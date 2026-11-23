//! What the reader is looking at: the tests, the code, the documents or the data, picked at
//! the top right. Each focus keeps a stack of screens of its own, its first screen at the
//! bottom, and a file explorer of its own whose tree holds only what the focus is about, so
//! switching away and back finds both where they were left.

use super::{App, Picture, Screen, Sidebar};
use crate::tui::outline::Outline;
use crate::tui::tree::Tree;
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

    /// Whether its file tree holds `path`.
    pub fn holds(self, path: &str) -> bool {
        match self {
            Focus::Tests | Focus::Code => true,
            Focus::Docs => crate::docs::is_document(path),
            Focus::Data => crate::media::is_dataset(path),
        }
    }

    /// The files of `all` its tree holds.
    pub fn files(self, all: &[String]) -> Vec<String> {
        all.iter().filter(|f| self.holds(f)).cloned().collect()
    }
}

/// One focus's file explorer: its tree, the file open in it, and how its side panel stands.
pub struct Explorer {
    pub tree: Tree,
    pub viewer: Option<Viewer>,
    pub outline: Option<Outline>,
    pub focus_tree: bool,
    pub sidebar: Sidebar,
    pub picture: Option<Picture>,
}

impl Explorer {
    pub fn new(files: &[String]) -> Self {
        Self {
            tree: Tree::new(files),
            viewer: None,
            outline: None,
            focus_tree: true,
            sidebar: Sidebar::Files,
            picture: None,
        }
    }
}

impl App {
    /// Takes the explorer on screen out of the app, leaving an empty one in its place.
    fn take_explorer(&mut self) -> Explorer {
        Explorer {
            tree: std::mem::replace(&mut self.tree, Tree::new(&[])),
            viewer: self.viewer.take(),
            outline: self.outline.take(),
            focus_tree: self.focus_tree,
            sidebar: self.sidebar,
            picture: self.picture.take(),
        }
    }

    fn put_explorer(&mut self, e: Explorer) {
        self.tree = e.tree;
        self.viewer = e.viewer;
        self.outline = e.outline;
        self.focus_tree = e.focus_tree;
        self.sidebar = e.sidebar;
        self.picture = e.picture;
    }

    /// Goes to `to` where it was left, making its first screen the first time. Asked for the
    /// focus already on screen, it goes back to that focus's first screen.
    pub fn switch_focus(&mut self, to: Focus) {
        if to == self.focus {
            self.screens.truncate(1);
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
        self.tree.replace_files(&files);
        match self.screen() {
            Screen::EndToEnd(_) => self.refresh_end_to_end(false),
            Screen::Review(_) => self.refresh_review(),
            Screen::Notes(_) => self.refresh_notes(),
            _ => {}
        }
    }

    /// Gives every focus's tree the files it holds now.
    pub(super) fn refresh_trees(&mut self) {
        let all = self.session.files.clone();
        self.tree.replace_files(&self.focus.files(&all));
        for focus in Focus::ALL {
            if let Some(e) = &mut self.parked_explorers[focus.index()] {
                e.tree.replace_files(&focus.files(&all));
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

    /// Whether the side panel is drawn: always, unless `\` hid it, and always with no file
    /// open, where there is nothing to give its width to.
    pub fn sidebar_shown(&self) -> bool {
        !self.sidebar_hidden || self.viewer.is_none()
    }
}
