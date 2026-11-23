//! One file of a review range, shown two ways. As a diff, in omnidiff's own view: the two sides
//! matched node by node, the minimap, change by change with `n` and `p`, omnidiff drawing it and
//! moving through it. Or as code, the file as it is at one end of the range, syntax highlighted
//! and nothing else, which is how an added or a deleted file opens: a diff of a whole file
//! against nothing is half a screen of nothing and the other half all one colour. `s` goes from
//! one to the next.
//!
//! Around either, what belongs to the review: the range's other files (`]`, `[`), a comment on
//! the after side's line under the cursor (`c`), a search (`/`), `$EDITOR` (`ge`) and the way
//! back (`q`).
//!
//! A child module of `app`, so it goes on using the app's own methods.

use std::path::Path;

use omnidiff::Side;
use omnidiff::tui::embed::{DiffView, Outcome, OverlayTheme};

use super::*;

/// What the screen shows of the file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shown {
    Diff,
    /// The file as it is on one side: the head's (`After`) or the base's (`Before`).
    Code(Side),
}

pub struct OmniState {
    pub view: DiffView,
    pub shown: Shown,
    /// Each side as code, made the first time it is shown: before, then after.
    pub code: [Option<Viewer>; 2],
    /// Each side's text, from which the code views are made.
    texts: [String; 2],
    pub target: DiffTarget,
    /// The range's files, of which this is `index`, and each one's size.
    pub files: Vec<ChangedFile>,
    pub sizes: Vec<Option<omnidiff::Stats>>,
    pub index: usize,
    /// The after side's lines, for the text a comment records.
    pub after_lines: Vec<String>,
}

impl OmniState {
    pub fn file(&self) -> &ChangedFile {
        &self.files[self.index]
    }

    /// What `s` goes through, in order: the diff, then each side that has any text.
    fn choices(&self) -> Vec<Shown> {
        let mut out = vec![Shown::Diff];
        if !self.texts[1].is_empty() {
            out.push(Shown::Code(Side::After));
        }
        if !self.texts[0].is_empty() {
            out.push(Shown::Code(Side::Before));
        }
        out
    }

    /// The code view on screen, when the screen shows code.
    pub fn code_view(&mut self) -> Option<&mut Viewer> {
        match self.shown {
            Shown::Code(side) => self.code[side_index(side)].as_mut(),
            Shown::Diff => None,
        }
    }
}

fn side_index(side: Side) -> usize {
    match side {
        Side::Before => 0,
        _ => 1,
    }
}

impl App {
    /// File `index` of `files`, behind a loading bar while it is diffed; at `row` of its after
    /// side, 0-based, when one is given. A file that is not text opens in the plain diff.
    pub(super) fn push_omni(
        &mut self,
        target: DiffTarget,
        files: Vec<ChangedFile>,
        sizes: Vec<Option<omnidiff::Stats>>,
        index: usize,
        row: Option<usize>,
    ) {
        let Some(path) = files.get(index).map(|f| f.path.clone()) else {
            return;
        };
        self.later(format!("diffing {path}…"), move |app| {
            app.push_omni_now(target, files, sizes, index);
            if let (Some(row), Some(Screen::Omni(o))) = (row, app.screens.last_mut()) {
                if o.shown == Shown::Diff {
                    o.view.go_to_after_row(row);
                } else if let Some(v) = o.code[1].as_mut() {
                    v.go_to_line(row);
                }
            }
        });
    }

    fn push_omni_now(
        &mut self,
        target: DiffTarget,
        files: Vec<ChangedFile>,
        sizes: Vec<Option<omnidiff::Stats>>,
        index: usize,
    ) {
        let Some(file) = files.get(index).cloned() else {
            return;
        };
        let (before, after) = match self.session.diff_texts(&target, &file) {
            Ok(Some(texts)) => texts,
            Ok(None) => {
                self.push_diff(target, files, index);
                return;
            }
            Err(e) => {
                self.error(format!("{e:#}"));
                return;
            }
        };
        let old_path = file.old_path.clone().unwrap_or_else(|| file.path.clone());
        let mut view = match DiffView::from_texts(
            Path::new(&old_path),
            &before,
            Path::new(&file.path),
            &after,
        ) {
            Ok(view) => view,
            Err(e) => {
                self.error(format!("{e:#}"));
                return;
            }
        };
        view.set_overlay_theme(if self.theme.dark {
            OverlayTheme::default()
        } else {
            OverlayTheme::SolarizedLight
        });
        view.set_syntax_theme(&self.theme.syntax);
        // What a review comments on, and the only side an added file has.
        view.focus_side(Side::After);
        // A whole file added or removed is read as code, not as a diff against nothing.
        let shown = match file.status {
            FileStatus::Added | FileStatus::Untracked => Shown::Code(Side::After),
            FileStatus::Deleted => Shown::Code(Side::Before),
            _ => Shown::Diff,
        };
        let mut state = OmniState {
            view,
            shown,
            code: [None, None],
            texts: [before, after.clone()],
            target,
            files,
            sizes,
            index,
            after_lines: after.lines().map(str::to_string).collect(),
        };
        self.make_code_view(&mut state);
        self.screens.push(Screen::Omni(Box::new(state)));
    }

    /// The code view of the side `state` shows, made if it is not yet: highlighted as the
    /// file's name says, the after side with the review's comments under their lines.
    fn make_code_view(&self, state: &mut OmniState) {
        let Shown::Code(side) = state.shown else {
            return;
        };
        let at = side_index(side);
        if state.code[at].is_some() {
            return;
        }
        let file = state.file().clone();
        let path = match side {
            Side::Before => file.old_path.clone().unwrap_or(file.path.clone()),
            _ => file.path.clone(),
        };
        let text = state.texts[at].clone();
        let comments = if at == 1 {
            self.session.anchor_in(&file.path, &text)
        } else {
            Vec::new()
        };
        let view = crate::session::FileView {
            path,
            total_lines: text.lines().count(),
            text,
            binary: false,
            comments,
            notes: Vec::new(),
        };
        state.code[at] = Some(Viewer::new(view, &self.theme.syntax));
    }

    /// Another file of the same range, in place of this one.
    fn replace_omni(&mut self, index: usize) {
        let Some(Screen::Omni(state)) = self.screens.last() else {
            return;
        };
        if index >= state.files.len() {
            self.info(if index == 0 {
                "first file"
            } else {
                "last file"
            });
            return;
        }
        let (target, files, sizes) = (
            state.target.clone(),
            state.files.clone(),
            state.sizes.clone(),
        );
        self.screens.pop();
        self.push_omni(target, files, sizes, index, None);
    }

    pub(super) fn handle_omni_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let height = self.measured.main_height.max(1) as isize;
        let Some(Screen::Omni(mut state)) = self.screens.pop() else {
            return;
        };
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => {
                self.screens.push(Screen::Omni(state));
                self.pop_screen();
                return;
            }
            KeyCode::Char(']') | KeyCode::Char('[') => {
                let next = if key.code == KeyCode::Char(']') {
                    Some(state.index + 1)
                } else {
                    state.index.checked_sub(1)
                };
                self.screens.push(Screen::Omni(state));
                match next {
                    Some(i) => self.replace_omni(i),
                    None => self.info("first file"),
                }
                return;
            }
            // The diff, the code at the head, the code at the base, and round again.
            KeyCode::Char('s') => {
                let choices = state.choices();
                let at = choices.iter().position(|c| *c == state.shown).unwrap_or(0);
                state.shown = choices[(at + 1) % choices.len()];
                self.make_code_view(&mut state);
            }
            KeyCode::Char('c') | KeyCode::Char('/') => {
                self.screens.push(Screen::Omni(state));
                if key.code == KeyCode::Char('c') {
                    self.start_comment();
                } else {
                    self.prompt = Some(Prompt::new(PromptKind::Search, "search", ""));
                }
                return;
            }
            // `ge`: the line in `$EDITOR`, the after side's.
            KeyCode::Char('o') => {
                let line = match state.shown {
                    Shown::Code(Side::After) => {
                        state
                            .code_view()
                            .and_then(|v| v.current_line())
                            .unwrap_or(0)
                            + 1
                    }
                    Shown::Diff => match state.view.cursor() {
                        Some((Side::After, row)) => row + 1,
                        _ => 1,
                    },
                    _ => 1,
                };
                self.editor_request = Some((self.session.root().join(&state.file().path), line));
            }
            code_key if state.shown != Shown::Diff => {
                let v = state.code_view().expect("made when shown");
                match code_key {
                    KeyCode::Char('j') | KeyCode::Down => v.move_by(1),
                    KeyCode::Char('k') | KeyCode::Up => v.move_by(-1),
                    KeyCode::Char('d') if ctrl => v.move_by(height / 2),
                    KeyCode::Char('u') if ctrl => v.move_by(-height / 2),
                    KeyCode::PageDown => v.move_by(height),
                    KeyCode::PageUp => v.move_by(-height),
                    KeyCode::Home => v.go_top(),
                    KeyCode::Char('G') | KeyCode::End => v.go_bottom(),
                    KeyCode::Char('>') => v.next_match(true),
                    KeyCode::Char('<') => v.next_match(false),
                    _ => self.info("q back  s diff or code  ] [ files  c comment  / search"),
                }
            }
            KeyCode::Char('>') => state.view.next_match(true),
            KeyCode::Char('<') => state.view.next_match(false),
            _ => {
                if state.view.handle_key(key) == Outcome::Ignored {
                    self.info(
                        "q back  s code  ] [ files  c comment  / search  n p changes  v layout",
                    );
                }
            }
        }
        self.screens.push(Screen::Omni(state));
    }
}
