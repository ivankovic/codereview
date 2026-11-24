//! What the reader is looking at: the tests, the code, the documents or the data, picked at
//! the top right. Each focus keeps a stack of screens of its own, its first screen at the
//! bottom, and a file explorer of its own — the documents or the datasets listed where no
//! file is open, the file open — so switching away and back finds both where they were left.

use super::{App, ListState, Picture, Screen};
use crate::tui::viewer::Viewer;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Focus {
    /// The end-to-end tests, and every test by its kind: what the repository demonstrably does.
    Tests,
    /// The files, and the log, changes, diffs and review reached from them.
    Code,
    /// The repository's Markdown documents.
    Docs,
    /// Its datasets: CSV, JSON and the like.
    Data,
}

impl Focus {
    pub const ALL: [Focus; 4] = [Focus::Tests, Focus::Code, Focus::Docs, Focus::Data];

    pub fn name(self) -> &'static str {
        match self {
            Focus::Tests => "Tests",
            Focus::Code => "Code",
            Focus::Docs => "Docs",
            Focus::Data => "Data",
        }
    }

    pub fn index(self) -> usize {
        Focus::ALL.iter().position(|f| *f == self).expect("listed")
    }

    /// The digit that switches to it.
    pub fn key(self) -> char {
        char::from(b'1' + self.index() as u8)
    }

    pub fn from_key(c: char) -> Option<Focus> {
        let at = c.to_digit(10)?.checked_sub(1)?;
        Focus::ALL.get(at as usize).copied()
    }

    /// Whether `path` is what it is about: anything for the tests and the code.
    pub fn holds(self, path: &str) -> bool {
        match self {
            Focus::Tests | Focus::Code => true,
            Focus::Docs => crate::docs::is_document(path),
            Focus::Data => crate::media::is_dataset(path),
        }
    }

    /// The files of `all` it lists where none is open: the documents, the datasets, and
    /// for the tests and the code none, which are too many to list.
    pub fn files(self, all: &[String]) -> Vec<String> {
        match self {
            Focus::Tests | Focus::Code => Vec::new(),
            Focus::Docs | Focus::Data => all.iter().filter(|f| self.holds(f)).cloned().collect(),
        }
    }
}

/// One focus's file explorer: the files it lists, and the file open in it.
pub struct Explorer {
    pub files: ListState<String>,
    pub viewer: Option<Viewer>,
    pub picture: Option<Picture>,
}

impl Explorer {
    pub fn new(files: &[String]) -> Self {
        Self {
            files: ListState::new(files.to_vec()),
            viewer: None,
            picture: None,
        }
    }
}

impl App {
    /// Takes the explorer on screen out of the app, leaving an empty one in its place.
    fn take_explorer(&mut self) -> Explorer {
        Explorer {
            files: std::mem::replace(&mut self.files, ListState::new(Vec::new())),
            viewer: self.viewer.take(),
            picture: self.picture.take(),
        }
    }

    fn put_explorer(&mut self, e: Explorer) {
        self.files = e.files;
        self.viewer = e.viewer;
        self.picture = e.picture;
    }

    /// Goes to `to` where it was left, making its first screen the first time. Asked for the
    /// focus already on screen, it goes back to that focus's first screen.
    pub fn switch_focus(&mut self, to: Focus) {
        if to == self.focus {
            self.screens.truncate(1);
            // The tests' first screen is their overview, under the list.
            if let Some(Screen::EndToEnd(state)) = self.screens.last_mut() {
                state.to_overview();
            }
            return;
        }
        let left = std::mem::take(&mut self.screens);
        self.parked[self.focus.index()] = left;
        let explorer = self.take_explorer();
        self.parked_explorers[self.focus.index()] = Some(explorer);
        let explorer = self.parked_explorers[to.index()]
            .take()
            .unwrap_or_else(|| Explorer::new(&to.files(&self.session.files)));
        self.put_explorer(explorer);
        self.screens = std::mem::take(&mut self.parked[to.index()]);
        self.focus = to;
        if self.screens.is_empty() {
            self.open_focus_root();
        } else {
            // It stood still while another focus was on screen; the repository did not.
            self.refresh_focus();
        }
        // A screen that could not be made leaves the focus empty; the tests are always there
        // to fall back on.
        if self.screens.is_empty() {
            self.screens.push(Screen::Explorer);
        }
    }

    fn open_focus_root(&mut self) {
        match self.focus {
            Focus::Tests => self.push_end_to_end(),
            Focus::Code | Focus::Docs | Focus::Data => self.screens.push(Screen::Explorer),
        }
    }

    /// Re-reads what the top screen of a focus shows, on coming back to it.
    fn refresh_focus(&mut self) {
        let files = self.focus.files(&self.session.files);
        relist(&mut self.files, files);
        match self.screen() {
            Screen::EndToEnd(_) => self.refresh_end_to_end(false),
            Screen::Review(_) => self.refresh_review(),
            Screen::Notes(_) => self.refresh_notes(),
            _ => {}
        }
    }

    /// Gives every focus's list the files it holds now.
    pub(super) fn refresh_trees(&mut self) {
        let all = self.session.files.clone();
        relist(&mut self.files, self.focus.files(&all));
        for focus in Focus::ALL {
            if let Some(e) = &mut self.parked_explorers[focus.index()] {
                relist(&mut e.files, focus.files(&all));
            }
        }
    }

    /// `q` on a focus's first screen: back to the tests, and from the tests, out.
    pub(super) fn leave_focus(&mut self) {
        if self.focus == Focus::Tests {
            self.quit = true;
        } else {
            self.switch_focus(Focus::Tests);
        }
    }

    /// Shows the explorer in the focus on screen: the one already on its stack, dropping
    /// what was opened over it, or a new one over whatever the file was opened from, so that
    /// `q` goes back there. A list of places picked from is done with once one is picked.
    pub(super) fn show_explorer(&mut self) {
        match self
            .screens
            .iter()
            .rposition(|s| matches!(s, Screen::Explorer))
        {
            Some(at) => self.screens.truncate(at + 1),
            None => {
                while self.screens.len() > 1
                    && matches!(self.screens.last(), Some(Screen::Locations(_)))
                {
                    self.screens.pop();
                }
                self.screens.push(Screen::Explorer);
            }
        }
    }

    /// Every test, by its kind, in the tests: the one already on their stack, dropping what was
    /// opened over it, or a new one over the end-to-end tests.
    pub(super) fn show_spec(&mut self) {
        if self.focus != Focus::Tests {
            self.switch_focus(Focus::Tests);
        }
        match self
            .screens
            .iter()
            .rposition(|s| matches!(s, Screen::Spec(_)))
        {
            Some(at) => self.screens.truncate(at + 1),
            None => {
                self.screens.truncate(1);
                self.push_spec(self.stretch.clone());
            }
        }
    }

    /// Every screen of every focus, the one on screen first.
    pub(super) fn every_screen_mut(&mut self) -> impl Iterator<Item = &mut Screen> {
        self.screens
            .iter_mut()
            .chain(self.parked.iter_mut().flatten())
    }
}

/// Puts `files` in `list`, the cursor staying on the file it was on while that is there.
fn relist(list: &mut ListState<String>, files: Vec<String>) {
    let was = list.current().cloned();
    *list = ListState::new(files);
    if let Some(at) = was.and_then(|w| list.items.iter().position(|f| *f == w)) {
        list.cursor = at;
    }
}
