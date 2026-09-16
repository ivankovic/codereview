//! The screens beyond the explorer: the log, the changes list, a diff, the review list, the
//! notes list, and a list of places to jump to. Each is a piece of state on the app's screen
//! stack, what puts it there, and the keys that drive it while it is on top.
//!
//! A child module of `app`, so these go on using the app's own methods without everything
//! having to be made public for them.

use super::*;

pub struct LogState {
    /// `None` for the whole repository, or the file whose history this is.
    pub path: Option<String>,
    pub commits: ListState<Commit>,
    /// Files of the selected commit, loaded on demand.
    pub files: ListState<ChangedFile>,
    pub files_of: Option<String>,
    pub focus_files: bool,
    pub exhausted: bool,
}

pub struct ChangesState {
    pub target: DiffTarget,
    pub files: ListState<ChangedFile>,
}

pub struct ReviewState {
    pub items: ListState<Anchored>,
    pub show_completed: bool,
}

pub struct NotesState {
    pub items: ListState<Note>,
}

pub struct DiffState {
    pub view: DiffView,
    /// The list the file was picked from, for `]` / `[`.
    pub siblings: Vec<ChangedFile>,
    pub index: usize,
}

/// One place to jump to: a definition, an occurrence, or a symbol.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hit {
    pub path: String,
    /// 1-based.
    pub line: usize,
    /// 0-based byte column.
    pub column: usize,
    /// `function total`, or empty for a plain occurrence.
    pub label: String,
    pub text: String,
}

pub struct LocationsState {
    pub title: String,
    pub items: ListState<Hit>,
}

impl App {
    pub(super) fn push_log(&mut self, path: Option<String>) {
        let commits = match &path {
            Some(p) => self.session.file_log(p, 0, LOG_PAGE),
            None => self.session.log(0, LOG_PAGE),
        };
        let Some(commits) = self.report(commits) else {
            return;
        };
        if commits.is_empty() {
            self.info(match &path {
                Some(p) => format!("{p} has no commits"),
                None => "no commits yet".to_string(),
            });
            return;
        }
        let exhausted = commits.len() < LOG_PAGE;
        let mut state = LogState {
            path,
            commits: ListState::new(commits),
            files: ListState::new(Vec::new()),
            files_of: None,
            focus_files: false,
            exhausted,
        };
        let outcome = Self::load_commit_files(&mut self.session, &mut state);
        self.report(outcome);
        self.screens.push(Screen::Log(state));
    }

    /// The files of the commit under the cursor. Takes the session rather than the whole
    /// app, so a caller holding the screen it belongs to can still call it: the two are
    /// different fields.
    fn load_commit_files(session: &mut Session, state: &mut LogState) -> Result<()> {
        let Some(commit) = state.commits.current() else {
            return Ok(());
        };
        if state.files_of.as_deref() == Some(commit.hash.as_str()) {
            return Ok(());
        }
        let hash = commit.hash.clone();
        let mut files = session.commit_files(&hash)?;
        if let Some(path) = &state.path {
            // Put the file whose history this is first.
            files.sort_by_key(|f| f.path != *path);
        }
        state.files = ListState::new(files);
        state.files_of = Some(hash);
        Ok(())
    }

    /// The next page of commits, when the cursor has reached the end of what is loaded.
    fn load_more_commits(session: &mut Session, state: &mut LogState) -> Result<()> {
        if state.exhausted {
            return Ok(());
        }
        let skip = state.commits.items.len();
        let more = match &state.path {
            Some(p) => session.file_log(p, skip, LOG_PAGE)?,
            None => session.log(skip, LOG_PAGE)?,
        };
        if more.len() < LOG_PAGE {
            state.exhausted = true;
        }
        state.commits.items.extend(more);
        Ok(())
    }

    pub(super) fn push_changes(&mut self, target: DiffTarget) {
        let Some(files) = self.report(self.session.changed_files(&target)) else {
            return;
        };
        if files.is_empty() {
            self.info(format!(
                "nothing {}",
                if target == DiffTarget::Staged {
                    "staged"
                } else {
                    "changed in the working tree"
                }
            ));
            return;
        }
        self.screens.push(Screen::Changes(ChangesState {
            target,
            files: ListState::new(files),
        }));
    }

    pub(super) fn push_diff(
        &mut self,
        target: DiffTarget,
        siblings: Vec<ChangedFile>,
        index: usize,
    ) {
        let Some(file) = siblings.get(index).cloned() else {
            return;
        };
        let Some(diff) = self.report(self.session.diff(&target, &file)) else {
            return;
        };
        let comments = self.session.review.comments_for(&file.path);
        let comments = crate::anchor::anchor_all(&comments, &diff.after.text);
        let view = DiffView::new(target, file, diff, comments, &self.theme.syntax);
        self.screens.push(Screen::Diff(Box::new(DiffState {
            view,
            siblings,
            index,
        })));
    }

    pub(super) fn replace_diff(&mut self, index: usize) {
        let Screen::Diff(d) = self.screen() else {
            return;
        };
        if index >= d.siblings.len() {
            return;
        }
        let target = d.view.target.clone();
        let siblings = d.siblings.clone();
        self.screens.pop();
        self.push_diff(target, siblings, index);
    }

    pub(super) fn push_review(&mut self) {
        let mut items = self.session.all_anchored();
        items.sort_by(|a, b| {
            b.comment
                .is_pending()
                .cmp(&a.comment.is_pending())
                .then_with(|| a.comment.path.cmp(&b.comment.path))
                .then_with(|| a.line.cmp(&b.line))
        });
        self.screens.push(Screen::Review(ReviewState {
            items: ListState::new(items),
            show_completed: false,
        }));
    }

    pub(super) fn refresh_review(&mut self) {
        let all = self.session.all_anchored();
        if let Screen::Review(r) = self.screen_mut() {
            let cursor = r.items.cursor;
            let mut items = all;
            items.sort_by(|a, b| {
                b.comment
                    .is_pending()
                    .cmp(&a.comment.is_pending())
                    .then_with(|| a.comment.path.cmp(&b.comment.path))
                    .then_with(|| a.line.cmp(&b.line))
            });
            r.items = ListState::new(items);
            // The cursor indexes the rows on screen, which is fewer than the items when
            // completed comments are hidden.
            let visible = Self::review_visible(r).len();
            r.items.cursor = cursor.min(visible.saturating_sub(1));
        }
    }

    pub(super) fn push_notes(&mut self) {
        let items: Vec<Note> = self.session.notes.notes().into_iter().cloned().collect();
        self.screens.push(Screen::Notes(NotesState {
            items: ListState::new(items),
        }));
    }

    pub(super) fn refresh_notes(&mut self) {
        let items: Vec<Note> = self.session.notes.notes().into_iter().cloned().collect();
        if let Screen::Notes(n) = self.screen_mut() {
            let cursor = n.items.cursor;
            n.items = ListState::new(items);
            n.items.cursor = cursor.min(n.items.items.len().saturating_sub(1));
        }
    }

    /// Visible review items, honouring the completed filter.
    pub fn review_visible(r: &ReviewState) -> Vec<&Anchored> {
        r.items
            .items
            .iter()
            .filter(|a| r.show_completed || a.comment.is_pending())
            .collect()
    }

    pub(super) fn push_locations(&mut self, title: String, items: Vec<Hit>) {
        // Start on the hit nearest the cursor when the list is for the current file.
        let mut list = ListState::new(items);
        if let Some(v) = &self.viewer {
            let line = v.current_line().unwrap_or(0) + 1;
            if let Some(i) = list
                .items
                .iter()
                .enumerate()
                .filter(|(_, h)| h.path == v.path)
                .min_by_key(|(_, h)| h.line.abs_diff(line))
                .map(|(i, _)| i)
            {
                list.cursor = i;
            }
        }
        self.screens
            .push(Screen::Locations(LocationsState { title, items: list }));
    }

    pub(super) fn handle_locations_key(&mut self, key: KeyEvent) {
        let height = self.measured.list_height.max(1) as isize;
        let Screen::Locations(state) = self.screens.last_mut().expect("screen") else {
            return;
        };
        if state.items.handle_nav(key, height) {
            return;
        }
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => self.pop_screen(),
            KeyCode::Enter | KeyCode::Char('o') | KeyCode::Char('l') => {
                if let Some(hit) = state.items.current().cloned() {
                    self.push_jump();
                    self.jump_to_col(&hit.path, hit.line, hit.column);
                }
            }
            _ => {}
        }
    }

    pub(super) fn handle_log_key(&mut self, key: KeyEvent) {
        let height = self.measured.list_height.max(1) as isize;
        let mut load_more = false;
        let Screen::Log(state) = self.screens.last_mut().expect("screen") else {
            return;
        };
        // One of the two lists has the keys; which one is what Tab switches.
        let moved = if state.focus_files {
            state.files.handle_nav(key, height)
        } else {
            let was_at_end = state.commits.cursor + 1 >= state.commits.items.len();
            let moved = state.commits.handle_nav(key, height);
            // Only a key that goes down can reach past the end and want another page.
            load_more = moved
                && was_at_end
                && matches!(
                    key.code,
                    KeyCode::Char('j')
                        | KeyCode::Down
                        | KeyCode::Char('G')
                        | KeyCode::End
                        | KeyCode::PageDown
                );
            moved
        };
        if !moved {
            match key.code {
                KeyCode::Char('q') | KeyCode::Esc => {
                    if state.focus_files {
                        state.focus_files = false;
                    } else {
                        self.pop_screen();
                    }
                    return;
                }
                KeyCode::Tab => {
                    if !state.files.items.is_empty() {
                        state.focus_files = !state.focus_files;
                    }
                    return;
                }
                KeyCode::Enter | KeyCode::Char('l') | KeyCode::Right => {
                    if state.focus_files || state.path.is_some() {
                        let Some(commit) = state.commits.current() else {
                            return;
                        };
                        let hash = commit.hash.clone();
                        let siblings = state.files.items.clone();
                        let index = if state.path.is_some() && !state.focus_files {
                            0
                        } else {
                            state.files.cursor
                        };
                        self.push_diff(DiffTarget::Commit { hash }, siblings, index);
                    } else if !state.files.items.is_empty() {
                        state.focus_files = true;
                    }
                    return;
                }
                KeyCode::Char('w') => {
                    // The file at this commit against the working tree.
                    let Some(commit) = state.commits.current() else {
                        return;
                    };
                    let hash = commit.hash.clone();
                    let file = if state.focus_files {
                        state.files.current().cloned()
                    } else {
                        state.path.as_ref().map(|p| ChangedFile {
                            status: FileStatus::Modified,
                            path: p.clone(),
                            old_path: None,
                        })
                    };
                    let Some(file) = file else {
                        self.info("pick a file first");
                        return;
                    };
                    self.push_diff(
                        DiffTarget::Revisions {
                            from: hash,
                            to: String::new(),
                        },
                        vec![file],
                        0,
                    );
                    return;
                }
                KeyCode::Char('H') => {
                    if state.focus_files {
                        if let Some(f) = state.files.current() {
                            let path = f.path.clone();
                            self.push_log(Some(path));
                        }
                    }
                    return;
                }
                _ => return,
            }
        }
        // What is under the cursor decides which files are shown, and reaching the end of
        // the list asks git for the next page. Both want the session, which is a different
        // field from the screen stack, so neither has to take the screen apart to get it.
        let mut failed = None;
        if let Some(Screen::Log(state)) = self.screens.last_mut() {
            if load_more {
                if let Err(e) = Self::load_more_commits(&mut self.session, state) {
                    failed = Some(e);
                }
            }
            if let Err(e) = Self::load_commit_files(&mut self.session, state) {
                failed = failed.or(Some(e));
            }
        }
        if let Some(e) = failed {
            self.error(format!("{e:#}"));
        }
    }

    pub(super) fn handle_changes_key(&mut self, key: KeyEvent) {
        let height = self.measured.list_height.max(1) as isize;
        let Screen::Changes(state) = self.screens.last_mut().expect("screen") else {
            return;
        };
        if state.files.handle_nav(key, height) {
            return;
        }
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => self.pop_screen(),
            KeyCode::Char('s') => {
                let target = if state.target == DiffTarget::Staged {
                    DiffTarget::Working
                } else {
                    DiffTarget::Staged
                };
                self.screens.pop();
                self.push_changes(target);
            }
            KeyCode::Enter | KeyCode::Char('l') | KeyCode::Right => {
                let target = state.target.clone();
                let siblings = state.files.items.clone();
                let index = state.files.cursor;
                self.push_diff(target, siblings, index);
            }
            KeyCode::Char('H') => {
                if let Some(f) = state.files.current() {
                    let path = f.path.clone();
                    self.push_log(Some(path));
                }
            }
            KeyCode::Char('o') => {
                if let Some(f) = state.files.current() {
                    let path = f.path.clone();
                    self.jump_to(&path, 1);
                }
            }
            _ => {}
        }
    }

    pub(super) fn handle_diff_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let height = self.measured.main_height.max(1);
        let half = (height / 2).max(1) as isize;
        let Screen::Diff(d) = self.screens.last_mut().expect("screen") else {
            return;
        };
        let v = &mut d.view;
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => {
                if v.search.is_some() && key.code == KeyCode::Esc {
                    v.search = None;
                } else {
                    self.pop_screen();
                }
            }
            KeyCode::Char('j') | KeyCode::Down => v.move_by(1),
            KeyCode::Char('k') | KeyCode::Up => v.move_by(-1),
            KeyCode::Char('d') if ctrl => v.move_by(half),
            KeyCode::Char('u') if ctrl => v.move_by(-half),
            KeyCode::Char('e') if ctrl => v.scroll_by(1, height),
            KeyCode::Char('y') if ctrl => v.scroll_by(-1, height),
            KeyCode::PageDown => v.move_by(height as isize),
            KeyCode::PageUp => v.move_by(-(height as isize)),
            KeyCode::Char('g') | KeyCode::Home => v.go_top(),
            KeyCode::Char('G') | KeyCode::End => v.go_bottom(),
            KeyCode::Char('h') | KeyCode::Left => v.hscroll = v.hscroll.saturating_sub(8),
            KeyCode::Char('l') | KeyCode::Right => v.hscroll += 8,
            KeyCode::Char('0') => v.hscroll = 0,
            KeyCode::Char('n') | KeyCode::Char('}') => v.next_change(true, false),
            KeyCode::Char('p') | KeyCode::Char('{') => v.next_change(false, false),
            KeyCode::Char('v') => {
                self.layout = self.layout.next();
                let outcome = self.session.set_layout(self.layout.name());
                self.report(outcome);
            }
            KeyCode::Char(']') => {
                let i = d.index + 1;
                if i < d.siblings.len() {
                    self.replace_diff(i);
                } else {
                    self.info("last file");
                }
            }
            KeyCode::Char('[') => {
                if d.index > 0 {
                    let i = d.index - 1;
                    self.replace_diff(i);
                } else {
                    self.info("first file");
                }
            }
            KeyCode::Char('c') => self.start_comment(),
            KeyCode::Char('a') => self.ask_about_context(),
            KeyCode::Char('x') => self.toggle_comment(),
            KeyCode::Char('e') => self.edit_comment(),
            KeyCode::Char('D') => self.delete_comment(),
            KeyCode::Char('u') => self.undo_delete(),
            KeyCode::Char('/') => {
                self.prompt = Some(Prompt::new(PromptKind::Search, "search (after side)", ""));
            }
            KeyCode::Char('>') => v.next_match(true),
            KeyCode::Char('<') => v.next_match(false),
            KeyCode::Char(':') => {
                self.prompt = Some(Prompt::new(
                    PromptKind::GoToLine,
                    "go to line (after side)",
                    "",
                ));
            }
            KeyCode::Char('H') => {
                let path = v.file.path.clone();
                self.push_log(Some(path));
            }
            KeyCode::Char('o') => {
                let line = v.after_line().unwrap_or(0) + 1;
                let path = v.file.path.clone();
                self.jump_to(&path, line);
            }
            _ => {}
        }
    }

    pub(super) fn handle_review_key(&mut self, key: KeyEvent) {
        let height = self.measured.list_height.max(1) as isize;
        let Screen::Review(state) = self.screens.last_mut().expect("screen") else {
            return;
        };
        let visible = Self::review_visible(state).len();
        if state.items.handle_nav_within(key, height, visible) {
            return;
        }
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => self.pop_screen(),
            KeyCode::Char('s') => {
                state.show_completed = !state.show_completed;
                state.items.cursor = 0;
            }
            KeyCode::Char('x') => self.toggle_comment(),
            KeyCode::Char('d') => self.delete_comment(),
            KeyCode::Char('e') => self.edit_comment(),
            KeyCode::Char('a') => self.ask_about_context(),
            KeyCode::Char('A') => {
                let text = prompts::address_all().to_string();
                self.prompt = Some(Prompt::new(
                    PromptKind::Agent {
                        context: Vec::new(),
                    },
                    "ask the agent",
                    &text,
                ));
            }
            KeyCode::Enter | KeyCode::Char('o') => {
                if let Some(a) = Self::review_visible(state).get(state.items.cursor) {
                    let (path, line) = (a.comment.path.clone(), a.line);
                    match line {
                        Some(line) => self.jump_to(&path, line),
                        // A comment on a whole path: show the file, or the directory in the tree.
                        None => self.jump_to_path(&path),
                    }
                }
            }
            KeyCode::Char('Z') => {
                let moved = self.session.reanchor();
                if let Some(n) = self.report(moved) {
                    self.refresh_review();
                    self.info(format!("{n} comment(s) re-anchored in REVIEW.md"));
                }
            }
            _ => {}
        }
    }

    pub(super) fn handle_notes_key(&mut self, key: KeyEvent) {
        let height = self.measured.list_height.max(1) as isize;
        let Screen::Notes(state) = self.screens.last_mut().expect("screen") else {
            return;
        };
        if state.items.handle_nav(key, height) {
            return;
        }
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => self.pop_screen(),
            KeyCode::Char('d') => self.delete_note(),
            KeyCode::Char('e') => self.edit_note(),
            KeyCode::Char('n') => {
                let path = state
                    .items
                    .current()
                    .filter(|n| !n.is_general())
                    .map(|n| n.target.clone());
                let label = match &path {
                    Some(p) => format!("note on {p}"),
                    None => "general note".to_string(),
                };
                self.prompt = Some(Prompt::new(PromptKind::Note { path }, label, ""));
            }
            KeyCode::Enter | KeyCode::Char('o') => {
                if let Some(n) = state.items.current().filter(|n| !n.is_general()) {
                    let path = n.target.clone();
                    self.jump_to(&path, 1);
                }
            }
            _ => {}
        }
    }
}
