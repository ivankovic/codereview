//! From where the reader is to where they could go next. `Enter` on a name takes the obvious
//! step — a use to its definition, a definition to its usages — and `J` lists every step there
//! is, in groups (`nav::here`): the definition, the usages, the tests that ran the line or name
//! what it is in, the documents that mention it. Every jump is remembered: `Ctrl-o` goes back
//! along the trail and `Tab` forward again.
//!
//! A child module of `app`, so it goes on using the app's own methods.

use crate::nav::{Here, Kind, Place, Step};

use super::*;

/// One row of the jump menu: a group's heading, or one of its places.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JumpRow {
    Heading(Kind),
    /// A group, and the index of a place in it.
    Place(Kind, usize),
}

impl JumpRow {
    fn selectable(&self) -> bool {
        matches!(self, JumpRow::Place(..))
    }
}

/// The jump menu over the screen: where the cursor is, and the places to go from there.
pub struct JumpMenu {
    pub here: Here,
    pub rows: ListState<JumpRow>,
}

impl JumpMenu {
    fn new(here: Here, at: Option<Kind>) -> Self {
        let mut rows = Vec::new();
        for g in &here.groups {
            rows.push(JumpRow::Heading(g.kind));
            rows.extend((0..g.places.len()).map(|i| JumpRow::Place(g.kind, i)));
        }
        let mut rows = ListState::new(rows);
        if let Some(kind) = at
            && let Some(i) = rows
                .items
                .iter()
                .position(|r| *r == JumpRow::Place(kind, 0))
        {
            rows.cursor = i;
        }
        super::end_to_end::settle(&mut rows, 1, JumpRow::selectable);
        Self { here, rows }
    }

    pub fn place(&self, row: JumpRow) -> Option<&Place> {
        let JumpRow::Place(kind, i) = row else {
            return None;
        };
        self.here.group(kind)?.places.get(i)
    }
}

impl App {
    /// Where the cursor is in the open file, as `nav::here` sees it; `None` without a file.
    fn here_at_cursor(&mut self) -> Option<Here> {
        let v = self.viewer.as_ref()?;
        let line = v.current_line()?;
        let (path, column) = (v.path.clone(), v.column());
        Some(crate::nav::here(&mut self.session, &path, line, column))
    }

    /// Runs `then` with where the cursor is, at once when the symbols are read, otherwise
    /// behind a loading bar while they are.
    fn with_here(&mut self, then: impl FnOnce(&mut App, Here) + 'static) {
        let work = move |app: &mut App| match app.here_at_cursor() {
            Some(here) => then(app, here),
            None => app.info("open a file, and put the cursor on what to go from"),
        };
        if self.session.has_index() {
            work(self);
        } else {
            self.later("reading the symbols…", work);
        }
    }

    /// `J`: every place to go from the cursor, in groups, to pick from.
    pub(super) fn open_jump_menu(&mut self) {
        self.with_here(|app, here| app.show_jump_menu(here, None));
    }

    fn show_jump_menu(&mut self, here: Here, at: Option<Kind>) {
        if here.groups.is_empty() {
            self.info("nowhere to go from here: no definition, usage, test or document");
            return;
        }
        self.jump_menu = Some(JumpMenu::new(here, at));
    }

    /// `Enter` on a name: its definition, from a use; its usages, from its definition.
    pub(super) fn default_jump(&mut self) {
        self.with_here(|app, here| {
            let from = app
                .viewer
                .as_ref()
                .map(|v| v.path.clone())
                .unwrap_or_default();
            match here.default_step(&from) {
                Some(Step::Go(place)) => app.go_to_place(&place),
                Some(Step::Choose(kind)) => app.show_jump_menu(here, Some(kind)),
                None if here.groups.is_empty() => app.info("nowhere to go from here"),
                None => app.show_jump_menu(here, None),
            }
        });
    }

    /// Goes to `place`, leaving where the cursor was on the trail.
    pub(super) fn go_to_place(&mut self, place: &Place) {
        self.push_jump();
        self.forward.clear();
        self.jump_to(&place.path, place.line);
    }

    /// `Tab`: forward along the trail, to where `Ctrl-o` came back from.
    pub(super) fn jump_forward(&mut self) {
        match self.forward.pop() {
            Some((path, line, col)) => {
                self.push_jump();
                self.jump_to_col(&path, line + 1, col);
            }
            None => self.info("no later position"),
        }
    }

    /// How many jumps back the trail goes, for the breadcrumb.
    pub fn trail_depth(&self) -> usize {
        self.jumps.len()
    }

    pub(super) fn handle_jump_menu_key(&mut self, key: KeyEvent) {
        let height = self.measured.main_height.max(1) as isize;
        let Some(menu) = &mut self.jump_menu else {
            return;
        };
        let before = menu.rows.cursor;
        if menu.rows.handle_nav(key, height) {
            let direction = menu.rows.cursor as isize - before as isize;
            super::end_to_end::settle(
                &mut menu.rows,
                if direction < 0 { -1 } else { 1 },
                JumpRow::selectable,
            );
            return;
        }
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('J') => self.jump_menu = None,
            KeyCode::Enter | KeyCode::Char('l') | KeyCode::Right => {
                let place = menu.rows.current().and_then(|r| menu.place(*r)).cloned();
                self.jump_menu = None;
                if let Some(place) = place {
                    self.go_to_place(&place);
                }
            }
            // A group's letter: to its first place.
            KeyCode::Char(c) => {
                let at = menu
                    .rows
                    .items
                    .iter()
                    .position(|r| matches!(r, JumpRow::Place(kind, 0) if kind.key() == c));
                match at {
                    Some(at) => menu.rows.cursor = at,
                    None => self.info(format!("no group under {c} here")),
                }
            }
            _ => {}
        }
    }
}
