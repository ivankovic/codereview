//! The five sections across the top, tests first: each is level 0 of a stack of its own, and
//! what a repository opens on is the first of them, never something pushed over the code.

use super::{App, Screen};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Section {
    /// The end-to-end tests: what the repository demonstrably does.
    Tests,
    /// Every test, by what kind of test it is.
    AllTests,
    /// The files, and the log, changes and diffs reached from them.
    Code,
    /// The comments in REVIEW.md, and the notes.
    Review,
    Agent,
}

impl Section {
    pub const ALL: [Section; 5] = [
        Section::Tests,
        Section::AllTests,
        Section::Code,
        Section::Review,
        Section::Agent,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Section::Tests => "Tests",
            Section::AllTests => "All tests",
            Section::Code => "Code",
            Section::Review => "Review",
            Section::Agent => "Agent",
        }
    }

    pub fn index(self) -> usize {
        Section::ALL
            .iter()
            .position(|s| *s == self)
            .expect("listed")
    }

    /// The digit that switches to it.
    pub fn key(self) -> char {
        char::from(b'1' + self.index() as u8)
    }

    pub fn from_key(c: char) -> Option<Section> {
        let at = c.to_digit(10)?.checked_sub(1)?;
        Section::ALL.get(at as usize).copied()
    }
}

impl App {
    /// Goes to `to` where it was left, making its first screen the first time. Asked for the
    /// section already on screen, it goes back to that section's first screen.
    pub fn switch_section(&mut self, to: Section) {
        if to == self.section {
            self.screens.truncate(1);
            return;
        }
        let left = std::mem::take(&mut self.screens);
        self.parked[self.section.index()] = left;
        self.screens = std::mem::take(&mut self.parked[to.index()]);
        self.section = to;
        if self.screens.is_empty() {
            self.open_section_root();
        } else {
            // It stood still while another section was on screen; the repository did not.
            self.refresh_section();
        }
        // A screen that could not be made, the specification when the tests cannot be read,
        // leaves the section empty; the tests are always there to fall back on.
        if self.screens.is_empty() {
            self.parked[to.index()] = Vec::new();
            self.section = Section::Tests;
            self.screens = std::mem::take(&mut self.parked[Section::Tests.index()]);
            if self.screens.is_empty() {
                self.push_end_to_end();
            }
        }
    }

    fn open_section_root(&mut self) {
        match self.section {
            Section::Tests => self.push_end_to_end(),
            Section::AllTests => self.push_spec(self.stretch.clone()),
            Section::Code => self.screens.push(Screen::Explorer),
            Section::Review => self.push_review(),
            Section::Agent => self.open_agent(),
        }
    }

    /// Re-reads what the top screen of a section shows, on coming back to it.
    fn refresh_section(&mut self) {
        match self.screen() {
            Screen::EndToEnd(_) | Screen::EndToEndTest(_) => self.refresh_end_to_end(false),
            Screen::Review(_) => self.refresh_review(),
            Screen::Notes(_) => self.refresh_notes(),
            _ => {}
        }
    }

    /// `q` on a section's first screen: back to the tests, and from the tests, out.
    pub(super) fn leave_section(&mut self) {
        if self.section == Section::Tests {
            self.quit = true;
        } else {
            self.switch_section(Section::Tests);
        }
    }

    /// Shows the explorer in the section on screen: the one already on its stack, dropping
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

    /// Every screen of every section, the one on screen first.
    pub(super) fn every_screen_mut(&mut self) -> impl Iterator<Item = &mut Screen> {
        self.screens
            .iter_mut()
            .chain(self.parked.iter_mut().flatten())
    }
}
