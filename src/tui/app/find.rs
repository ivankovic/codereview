//! `/`: one search for going somewhere new. What is typed is looked for as it is typed in the
//! open file, the definitions, the paths, the tests, the documents' headings and the review's
//! comments (`search::search`); once typing stops for a moment, every file's text is searched
//! too, on another thread, and its lines join the list at the end. `Tab` steps from group to
//! group, `Enter` goes, and the place gone from is on the trail `Ctrl-o` walks back.
//!
//! A child module of `app`, so it goes on using the app's own methods.

use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use crate::search::{Group, Hit, Near};
use crate::tui::prompt::Prompt;

use super::*;

/// How long typing has to stop before the text search runs.
const PAUSE: Duration = Duration::from_millis(250);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FindRow {
    Heading(Group),
    /// A group, and the index of a hit in it.
    Hit(Group, usize),
}

impl FindRow {
    fn selectable(&self) -> bool {
        matches!(self, FindRow::Hit(..))
    }
}

pub struct Finder {
    /// What is typed, edited as any prompt is.
    pub input: Prompt,
    pub groups: Vec<(Group, Vec<Hit>)>,
    pub rows: ListState<FindRow>,
    /// Where the reader was when it opened, for "In this file" and for ranking.
    near: Near,
    typed_at: Instant,
    /// The query the text search last ran, or is running, for.
    text_for: Option<String>,
    text: Option<Receiver<(String, Vec<Hit>)>>,
}

impl Finder {
    pub fn hit(&self, row: FindRow) -> Option<&Hit> {
        let FindRow::Hit(group, i) = row else {
            return None;
        };
        self.groups.iter().find(|(g, _)| *g == group)?.1.get(i)
    }

    /// Whether the text search is still running for what is typed.
    pub fn searching_text(&self) -> bool {
        self.text.is_some()
    }

    fn rebuild(&mut self) {
        let was = self.rows.current().copied();
        let mut rows = Vec::new();
        for (g, hits) in &self.groups {
            rows.push(FindRow::Heading(*g));
            rows.extend((0..hits.len()).map(|i| FindRow::Hit(*g, i)));
        }
        self.rows = ListState::new(rows);
        // The text search's lines arriving at the end leave the cursor where it was.
        if let Some(was) = was
            && let Some(at) = self.rows.items.iter().position(|r| *r == was)
        {
            self.rows.cursor = at;
        }
        super::end_to_end::settle(&mut self.rows, 1, FindRow::selectable);
    }
}

impl App {
    /// `/`: the search box, empty, over the screen.
    pub(super) fn open_finder(&mut self) {
        let file = self
            .viewer
            .as_ref()
            .filter(|_| matches!(self.screen(), Screen::Explorer))
            .map(|v| (v.path.clone(), v.lines.join("\n")));
        let changed = self.range_files();
        self.finder = Some(Finder {
            input: Prompt::new(crate::tui::prompt::PromptKind::Search, "search", ""),
            groups: Vec::new(),
            rows: ListState::new(Vec::new()),
            near: Near { file, changed },
            typed_at: Instant::now(),
            text_for: None,
            text: None,
        });
        if !self.session.has_index() {
            self.later("reading the symbols…", |_| {});
        }
    }

    /// Searches again for what is typed, all but the text, which waits for a pause.
    pub(super) fn refind(&mut self) {
        let Some(f) = &mut self.finder else {
            return;
        };
        let query = f.input.text.clone();
        let near = f.near.clone();
        let mut groups = crate::search::search(&mut self.session, &query, &near);
        let Some(f) = &mut self.finder else {
            return;
        };
        // What the text search found for this query, if it is what is typed still.
        if f.text_for.as_deref() == Some(query.as_str())
            && let Some(text) = f.groups.iter().find(|(g, _)| *g == Group::Text).cloned()
        {
            groups.push(text);
        }
        f.groups = groups;
        f.typed_at = Instant::now();
        f.rebuild();
    }

    /// Called every tick: starts the text search once typing has paused, and takes in what
    /// it found. Whether anything changed on screen.
    pub(super) fn tick_finder(&mut self) -> bool {
        let Some(f) = &mut self.finder else {
            return false;
        };
        let query = f.input.text.clone();
        let mut changed = false;
        if let Some(rx) = &f.text
            && let Ok((for_query, hits)) = rx.try_recv()
        {
            f.text = None;
            if for_query == query {
                f.groups.retain(|(g, _)| *g != Group::Text);
                if !hits.is_empty() {
                    f.groups.push((Group::Text, hits));
                }
                f.rebuild();
                changed = true;
            }
        }
        if f.text.is_none()
            && f.text_for.as_deref() != Some(query.as_str())
            && f.typed_at.elapsed() >= PAUSE
        {
            f.text_for = Some(query.clone());
            f.groups.retain(|(g, _)| *g != Group::Text);
            let (tx, rx) = std::sync::mpsc::channel();
            let repo = self.session.repo.clone();
            let near = f.near.clone();
            std::thread::spawn(move || {
                let hits = crate::search::text(&repo, &query, &near);
                let _ = tx.send((query, hits));
            });
            f.text = Some(rx);
            changed = true;
        }
        changed
    }

    pub(super) fn handle_finder_key(&mut self, key: KeyEvent) {
        let Some(f) = &mut self.finder else {
            return;
        };
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let step = |f: &mut Finder, by: isize| {
            f.rows.move_by(by);
            super::end_to_end::settle(&mut f.rows, by.signum(), FindRow::selectable);
        };
        match key.code {
            KeyCode::Esc => self.finder = None,
            KeyCode::Char('c') if ctrl => self.finder = None,
            KeyCode::Down => step(f, 1),
            KeyCode::Char('n') if ctrl => step(f, 1),
            KeyCode::Up => step(f, -1),
            KeyCode::Char('p') if ctrl => step(f, -1),
            KeyCode::PageDown => step(f, 10),
            KeyCode::PageUp => step(f, -10),
            // To the next group's first hit, round to the first.
            KeyCode::Tab | KeyCode::BackTab => {
                let back = key.code == KeyCode::BackTab;
                let at = f.rows.cursor;
                let starts: Vec<usize> = f
                    .rows
                    .items
                    .iter()
                    .enumerate()
                    .filter(|(_, r)| matches!(r, FindRow::Hit(_, 0)))
                    .map(|(i, _)| i)
                    .collect();
                let next = if back {
                    starts.iter().rev().find(|&&i| i < at).or(starts.last())
                } else {
                    starts.iter().find(|&&i| i > at).or(starts.first())
                };
                if let Some(&next) = next {
                    f.rows.cursor = next;
                }
            }
            KeyCode::Enter => {
                let hit = f.rows.current().and_then(|r| f.hit(*r)).cloned();
                let query = crate::search::Query::parse(&f.input.text).text;
                match hit {
                    Some(hit) => {
                        self.finder = None;
                        self.go_to_hit(&hit, &query);
                    }
                    None => self.info("nothing found to go to"),
                }
            }
            KeyCode::Backspace => {
                f.input.backspace();
                self.refind();
            }
            KeyCode::Char('u') if ctrl => {
                f.input.text.clear();
                f.input.cursor = 0;
                self.refind();
            }
            KeyCode::Char('w') if ctrl => {
                f.input.delete_word();
                self.refind();
            }
            KeyCode::Left => f.input.left(),
            KeyCode::Right => f.input.right(),
            KeyCode::Char(c) if !ctrl => {
                f.input.insert(c);
                self.refind();
            }
            _ => {}
        }
    }

    /// Opens a hit, leaving where the cursor was on the trail. A hit in the open file also
    /// leaves the query searched for there, so `>` and `<` step through the rest.
    fn go_to_hit(&mut self, hit: &Hit, query: &str) {
        self.push_jump();
        self.forward.clear();
        if self.focus == Focus::Tests || !self.focus.holds(&hit.path) {
            self.switch_focus(Focus::Code);
        }
        self.jump_to(&hit.path, hit.line.max(1));
        if hit.group == Group::InFile
            && let Some(v) = &mut self.viewer
        {
            v.set_search(query);
            // On the match itself, not only its line, so `Enter` and `J` go from there.
            let line = hit.line.saturating_sub(1);
            if let Some(&(_, start, _)) = v
                .search
                .as_ref()
                .and_then(|s| s.matches.iter().find(|m| m.0 == line))
            {
                v.col = start;
                v.keep_col_visible();
            }
            self.last_search = query.to_string();
        }
    }
}
