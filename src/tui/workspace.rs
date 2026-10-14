//! Several repositories in one terminal: an [`App`] per repository, one of them active, a
//! strip naming them all when there is more than one, and a picker. Started on a directory of
//! checkouts, the picker is also the screen it opens on and the one `q` leads back to from a
//! repository, since no repository there is the one the reader came for. Apps do not know about
//! each other; they leave a [`WorkspaceRequest`] and the workspace acts on it. Settings the
//! active app saves (theme, diff layout) are handed to the others so nothing writes a stale
//! copy of the config file back later.

use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::session::{Checkouts, Session};
use crate::tui::app::{App, WorkspaceRequest};

/// How often the directories of checkouts are read again for new repositories.
const SCAN_EVERY: Duration = Duration::from_secs(5);

pub struct Workspace {
    pub apps: Vec<App>,
    pub active: usize,
    /// The repository picker, with the highlighted index.
    pub picker: Option<usize>,
    /// The picker is the "Select repository" screen rather than a box over the active
    /// repository: there is nothing behind it to close it onto.
    pub selecting: bool,
    /// Started on a directory of checkouts: leaving a repository's last screen comes back to
    /// the "Select repository" screen instead of quitting.
    home: bool,
    /// `q` on the "Select repository" screen, or Ctrl-C over the picker.
    quitting: bool,
    /// Each repository's branch, read when the picker opens rather than on every draw: it is
    /// a `git` call per repository.
    pub branches: Vec<String>,
    /// The directories of checkouts this was started on, read again every few seconds so a
    /// repository cloned or created in one joins the others.
    checkouts: Checkouts,
    scanned: Instant,
}

impl Workspace {
    /// One app per `(session, file to open)`.
    pub fn new(targets: Vec<(Session, Option<String>)>) -> Self {
        let apps = targets
            .into_iter()
            .map(|(session, open)| App::new(session, open))
            .collect();
        Self {
            apps,
            active: 0,
            picker: None,
            selecting: false,
            home: false,
            quitting: false,
            branches: Vec::new(),
            checkouts: Checkouts::default(),
            scanned: Instant::now(),
        }
    }

    /// Opens on the "Select repository" screen, and comes back to it from a repository's
    /// last screen: what a directory of checkouts starts on.
    pub fn open_home(&mut self) {
        self.home = true;
        self.select_repository();
    }

    fn select_repository(&mut self) {
        self.selecting = true;
        self.open_picker();
    }

    fn open_picker(&mut self) {
        self.picker = Some(self.active);
        self.branches = self
            .apps
            .iter()
            .map(|a| a.session.repo.head_label())
            .collect();
    }

    /// Reads `checkouts` again from now on, for repositories that appear in them.
    pub fn watch(&mut self, checkouts: Checkouts) {
        self.checkouts = checkouts;
    }

    /// Opens every repository that has appeared in the directories of checkouts, after the
    /// others, and says so. True when there was one.
    pub fn find_new(&mut self) -> bool {
        self.scanned = Instant::now();
        let mut problems = Vec::new();
        let found = self.checkouts.scan(|m| problems.push(m));
        let graphics_known = self.app().graphics_known;
        let graphics = self.app().graphics.clone();
        let mut names = Vec::new();
        for session in found {
            names.push(session.name());
            let mut app = App::new(session, None);
            app.graphics = graphics.clone();
            app.graphics_known = graphics_known;
            if self.picker.is_some() {
                self.branches.push(app.session.repo.head_label());
            }
            self.apps.push(app);
        }
        // The theme, layout and side panel the active repository has: the config file the
        // new one read may be older than what the others hold.
        self.propagate_settings();
        if !names.is_empty() {
            let first = self.apps.len() - names.len() + 1;
            self.app_mut()
                .info(format!("new repository {first}: {}", names.join(", ")));
        } else if let Some(problem) = problems.pop() {
            self.app_mut().info(problem);
        }
        !names.is_empty()
    }

    /// Tells every app how this terminal draws pictures, once it has been asked, and drops
    /// any picture described before the answer came.
    pub fn set_graphics(&mut self, graphics: Option<ratatui_image::picker::Picker>) {
        for app in &mut self.apps {
            app.graphics = graphics.clone();
            app.graphics_known = true;
            app.picture = None;
        }
    }

    pub fn app(&self) -> &App {
        &self.apps[self.active]
    }

    pub fn app_mut(&mut self) -> &mut App {
        &mut self.apps[self.active]
    }

    pub fn quit(&self) -> bool {
        self.quitting || self.apps.iter().any(|a| a.quit)
    }

    /// True while the repository on screen is playing a recording.
    pub fn animating(&self) -> bool {
        self.app().animating()
    }

    /// True while any repository's agent is starting or working.
    pub fn agent_active(&self) -> bool {
        self.apps.iter().any(App::agent_active)
    }

    /// Ticks every app, so agents in the other repositories keep going, and every few seconds
    /// looks for new repositories.
    pub fn tick(&mut self) -> bool {
        let mut changed = false;
        if !self.checkouts.is_empty() && self.scanned.elapsed() >= SCAN_EVERY {
            changed |= self.find_new();
        }
        for (i, app) in self.apps.iter_mut().enumerate() {
            changed |= app.tick(i == self.active);
        }
        changed
    }

    pub fn handle_key(&mut self, key: KeyEvent) {
        if let Some(index) = self.picker {
            self.handle_picker_key(index, key);
            return;
        }
        let ctrl_c =
            key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c');
        self.app_mut().handle_key(key);
        if self.home && self.app().quit && !ctrl_c {
            self.app_mut().quit = false;
            self.select_repository();
        }
        if let Some(request) = self.app_mut().workspace_request.take() {
            match request {
                WorkspaceRequest::NextRepo => self.switch(self.active + 1),
                WorkspaceRequest::PrevRepo => self.switch(self.active + self.apps.len() - 1),
                WorkspaceRequest::PickRepo => {
                    if self.apps.len() > 1 {
                        self.open_picker();
                    } else {
                        self.app_mut()
                            .info("one repository open; name more on the command line");
                    }
                }
            }
        }
        self.propagate_settings();
    }

    fn handle_picker_key(&mut self, index: usize, key: KeyEvent) {
        let last = self.apps.len() - 1;
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.quitting = true;
            return;
        }
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => self.picker = Some((index + 1).min(last)),
            KeyCode::Char('k') | KeyCode::Up => self.picker = Some(index.saturating_sub(1)),
            KeyCode::Char('g') | KeyCode::Home => self.picker = Some(0),
            KeyCode::Char('G') | KeyCode::End => self.picker = Some(last),
            KeyCode::Char(c @ '1'..='9') => {
                let i = c as usize - '1' as usize;
                if i <= last {
                    self.choose(i);
                }
            }
            KeyCode::Enter => self.choose(index),
            // Nothing behind the "Select repository" screen to go back to.
            KeyCode::Char('q') if self.selecting => self.quitting = true,
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('W') if !self.selecting => {
                self.picker = None
            }
            _ => {}
        }
    }

    /// Moves the picker's highlight by `by` rows, for the mouse wheel; false when the picker
    /// is not open.
    pub fn scroll_picker(&mut self, by: isize) -> bool {
        let Some(index) = self.picker else {
            return false;
        };
        let last = self.apps.len() - 1;
        self.picker = Some(index.saturating_add_signed(by).min(last));
        true
    }

    /// Closes the picker on repository `index`, which may be the one already open.
    fn choose(&mut self, index: usize) {
        self.picker = None;
        self.selecting = false;
        self.active = index;
        let (i, name) = (index + 1, self.app().session.name());
        self.app_mut().info(format!("repository {i}: {name}"));
    }

    fn switch(&mut self, index: usize) {
        if self.apps.len() < 2 {
            self.app_mut()
                .info("one repository open; name more on the command line");
            return;
        }
        self.active = index % self.apps.len();
        let (i, name) = (self.active + 1, self.app().session.name());
        self.app_mut().info(format!("repository {i}: {name}"));
    }

    /// Hands the active app's saved theme, layout and side panel settings to the others.
    /// Skipped while the theme picker previews, since a preview is not a decision yet.
    fn propagate_settings(&mut self) {
        let active = &self.apps[self.active];
        if active.theme_picker.is_some() {
            return;
        }
        let theme = active.theme.clone();
        let layout = active.layout;
        let sidebar = active.session.config.sidebar;
        for (i, app) in self.apps.iter_mut().enumerate() {
            if i == self.active {
                continue;
            }
            if app.theme.name != theme.name {
                app.adopt_theme(theme.clone());
            }
            if app.layout != layout {
                app.adopt_layout(layout);
            }
            if app.session.config.sidebar != sidebar {
                app.adopt_sidebar(sidebar);
            }
        }
    }
}
