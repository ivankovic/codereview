//! Application state and key handling. Drawing lives in `ui.rs`; nothing here touches the
//! terminal.

use std::path::PathBuf;

use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::agent::prompts;
use crate::anchor::Anchored;
use crate::notes::Note;
use crate::repo::{ChangedFile, Commit, FileStatus};
use crate::review::{self, Comment};
use crate::session::{DiffTarget, Session};
use crate::theme::Theme;
mod overlays;
mod screens;

use crate::tui::agent_panel;
use crate::tui::diff_view::{DiffLayout, DiffView};
use crate::tui::prompt::{Prompt, PromptKind};
use crate::tui::tree::Tree;
use crate::tui::viewer::Viewer;

/// Commits fetched per page of a log.
const LOG_PAGE: usize = 200;

/// What the panel says it is starting: `claude`, or the ACP command line.
fn hit_from_symbol(s: crate::symbols::Symbol) -> Hit {
    let label = match &s.container {
        Some(c) => format!("{} {} in {c}", s.kind, s.name),
        None => format!("{} {}", s.kind, s.name),
    };
    Hit {
        path: s.path,
        line: s.line,
        column: s.column,
        label,
        text: s.text,
    }
}

pub struct ListState<T> {
    pub items: Vec<T>,
    pub cursor: usize,
    pub scroll: usize,
}

impl<T> ListState<T> {
    pub fn new(items: Vec<T>) -> Self {
        Self {
            items,
            cursor: 0,
            scroll: 0,
        }
    }

    pub fn current(&self) -> Option<&T> {
        self.items.get(self.cursor)
    }

    pub fn move_by(&mut self, delta: isize) {
        if self.items.is_empty() {
            self.cursor = 0;
            return;
        }
        let max = self.items.len() as isize - 1;
        self.cursor = (self.cursor as isize + delta).clamp(0, max) as usize;
    }

    /// The keys that move a cursor through a list, the same on every screen that has one.
    /// Returns whether the key was one of them.
    pub fn handle_nav(&mut self, key: KeyEvent, page: isize) -> bool {
        self.handle_nav_within(key, page, self.items.len())
    }

    /// The same, for a list showing only `rows` of its items: the cursor indexes what is on
    /// screen, so it must stop at the end of that rather than at the end of the items.
    pub fn handle_nav_within(&mut self, key: KeyEvent, page: isize, rows: usize) -> bool {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let last = rows.saturating_sub(1) as isize;
        let mut to = |delta: isize| {
            self.cursor = (self.cursor as isize + delta).clamp(0, last.max(0)) as usize;
        };
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => to(1),
            KeyCode::Char('k') | KeyCode::Up => to(-1),
            KeyCode::PageDown => to(page),
            KeyCode::PageUp => to(-page),
            KeyCode::Char('d') if ctrl => to(page / 2),
            KeyCode::Char('u') if ctrl => to(-page / 2),
            KeyCode::Char('g') | KeyCode::Home => self.cursor = 0,
            KeyCode::Char('G') | KeyCode::End => self.cursor = last.max(0) as usize,
            _ => return false,
        }
        true
    }

    pub fn ensure_visible(&mut self, height: usize) {
        if height == 0 {
            return;
        }
        if self.cursor < self.scroll {
            self.scroll = self.cursor;
        } else if self.cursor >= self.scroll + height {
            self.scroll = self.cursor + 1 - height;
        }
    }
}

pub use screens::{
    ChangesState, DiffState, Hit, LocationsState, LogState, NotesState, ReviewState,
};

pub enum Screen {
    Explorer,
    Log(LogState),
    Changes(ChangesState),
    Diff(Box<DiffState>),
    Review(ReviewState),
    Notes(NotesState),
    Locations(LocationsState),
    Agent,
}

impl Screen {
    pub fn name(&self) -> &'static str {
        match self {
            Screen::Explorer => "explore",
            Screen::Log(l) if l.path.is_some() => "history",
            Screen::Log(_) => "log",
            Screen::Changes(_) => "changes",
            Screen::Diff(_) => "diff",
            Screen::Review(_) => "review",
            Screen::Notes(_) => "notes",
            Screen::Locations(_) => "locations",
            Screen::Agent => "agent",
        }
    }
}

/// What the draw pass measured last time, so key handling can page by the right amount.
#[derive(Debug, Clone, Copy, Default)]
pub struct Measured {
    pub tree_height: usize,
    pub main_height: usize,
    pub main_width: usize,
    pub list_height: usize,
}

#[derive(Debug, Clone)]
enum EditTarget {
    Comment(Comment),
    Note(Note),
}

/// Something the app wants the workspace (the layer holding every open repository) to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceRequest {
    NextRepo,
    PrevRepo,
    PickRepo,
}

pub struct App {
    pub session: Session,
    pub tree: Tree,
    pub viewer: Option<Viewer>,
    pub focus_tree: bool,
    pub screens: Vec<Screen>,
    pub prompt: Option<Prompt>,
    pub message: Option<(String, bool)>,
    pub show_help: bool,
    pub help_scroll: usize,
    pub highlight: bool,
    pub theme: Theme,
    /// Open theme picker: the highlighted index and the theme to restore on cancel.
    pub theme_picker: Option<(usize, Theme)>,
    pub layout: DiffLayout,
    pub quit: bool,
    pub measured: Measured,
    pub last_search: String,
    /// Set when the user asked for `$EDITOR`; the run loop suspends the terminal for it.
    pub editor_request: Option<(PathBuf, usize)>,
    /// Set when the user asked to switch repositories; the workspace takes it.
    pub workspace_request: Option<WorkspaceRequest>,
    edit_target: Option<EditTarget>,
    last_deleted: Option<Comment>,
    /// Positions left by a symbol jump, for `Ctrl-o`: (path, 0-based line, column).
    jumps: Vec<(String, usize, usize)>,
    /// A `g` was pressed and the next key completes the chord.
    pending_g: bool,
    /// The agent panel: the conversation, and the agent having it.
    pub agent: agent_panel::Panel,
}

impl App {
    pub fn new(session: Session, open: Option<String>) -> Self {
        let tree = Tree::new(&session.files);
        let theme = session.theme.clone();
        let layout = DiffLayout::from_name(&session.config.layout);
        let mut app = Self {
            session,
            tree,
            viewer: None,
            focus_tree: true,
            screens: vec![Screen::Explorer],
            prompt: None,
            message: None,
            show_help: false,
            help_scroll: 0,
            highlight: true,
            theme,
            theme_picker: None,
            layout,
            quit: false,
            measured: Measured::default(),
            last_search: String::new(),
            editor_request: None,
            workspace_request: None,
            edit_target: None,
            last_deleted: None,
            jumps: Vec::new(),
            pending_g: false,
            agent: agent_panel::Panel::new(),
        };
        if let Some(path) = open {
            app.tree.select(&path);
            app.open_file(&path);
            app.focus_tree = false;
        }
        app
    }

    pub fn screen(&self) -> &Screen {
        self.screens.last().expect("at least the explorer")
    }

    fn screen_mut(&mut self) -> &mut Screen {
        self.screens.last_mut().expect("at least the explorer")
    }

    pub fn info(&mut self, text: impl Into<String>) {
        self.message = Some((text.into(), false));
    }

    pub fn error(&mut self, text: impl Into<String>) {
        self.message = Some((text.into(), true));
    }

    fn report<T>(&mut self, result: Result<T>) -> Option<T> {
        match result {
            Ok(v) => Some(v),
            Err(e) => {
                self.error(format!("{e:#}"));
                None
            }
        }
    }

    // ----- explorer --------------------------------------------------------------------------

    pub fn open_file(&mut self, path: &str) {
        let view = match self.session.file_view(path) {
            Ok(v) => v,
            Err(e) => {
                self.error(format!("{e:#}"));
                return;
            }
        };
        let mut viewer = Viewer::new(view, &self.theme.syntax);
        if let Some(entry) = self.session.status.get(path) {
            if let Some(status) = entry.unstaged.or(entry.staged) {
                let file = ChangedFile {
                    status,
                    path: path.to_string(),
                    old_path: entry.old_path.clone(),
                };
                let target = if entry.unstaged.is_some() {
                    DiffTarget::Working
                } else {
                    DiffTarget::Staged
                };
                if let Ok(diff) = self.session.diff(&target, &file) {
                    if diff.after.line_count() == viewer.line_count() {
                        viewer.line_ops = Some(diff.after.line_ops);
                    }
                }
            }
        }
        self.viewer = Some(viewer);
    }

    /// Re-reads the current file's comments after a change to REVIEW.md.
    fn refresh_viewer_comments(&mut self) {
        let Some(path) = self.viewer.as_ref().map(|v| v.path.clone()) else {
            return;
        };
        if let Ok(view) = self.session.file_view(&path) {
            if let Some(viewer) = &mut self.viewer {
                viewer.set_comments(view.comments);
            }
        }
        self.refresh_diff_comments();
    }

    fn refresh_diff_comments(&mut self) {
        for screen in &mut self.screens {
            if let Screen::Diff(d) = screen {
                let comments = self.session.review.comments_for(&d.view.file.path);
                d.view.comments = crate::anchor::anchor_all(&comments, &d.view.diff.after.text);
            }
        }
    }

    pub fn refresh_all(&mut self) {
        if let Err(e) = self.session.refresh() {
            self.error(format!("{e:#}"));
            return;
        }
        self.tree.replace_files(&self.session.files);
        if let Some(path) = self.viewer.as_ref().map(|v| v.path.clone()) {
            let (cursor, scroll) = self
                .viewer
                .as_ref()
                .map(|v| (v.cursor, v.scroll))
                .unwrap_or_default();
            self.open_file(&path);
            if let Some(v) = &mut self.viewer {
                v.cursor = cursor.min(v.rows.len().saturating_sub(1));
                v.scroll = scroll;
            }
        }
        self.refresh_diff_comments();
        self.info("refreshed");
    }

    /// Opens `path` in the explorer at `line` (1-based), popping every other screen.
    pub fn jump_to(&mut self, path: &str, line: usize) {
        self.jump_to_col(path, line, 0);
    }

    /// Shows a whole path: the file at its first line, or a directory in the tree.
    pub fn jump_to_path(&mut self, path: &str) {
        if self.tree.node(path).is_some_and(|n| n.is_dir) {
            self.screens.truncate(1);
            self.tree.select(path);
            self.focus_tree = true;
            return;
        }
        self.jump_to(path, 1);
    }

    /// Like `jump_to`, landing on byte `column` of the line.
    pub fn jump_to_col(&mut self, path: &str, line: usize, column: usize) {
        self.screens.truncate(1);
        self.tree.select(path);
        if self.viewer.as_ref().is_none_or(|v| v.path != path) {
            self.open_file(path);
        }
        self.focus_tree = false;
        if let Some(v) = &mut self.viewer {
            v.go_to_line(line.saturating_sub(1));
            v.col = column;
            v.keep_col_visible();
        }
    }

    /// Records where the cursor is, so `Ctrl-o` can come back.
    fn push_jump(&mut self) {
        if let Some(v) = &self.viewer {
            if let Some(line) = v.current_line() {
                self.jumps.push((v.path.clone(), line, v.column()));
                if self.jumps.len() > 100 {
                    self.jumps.remove(0);
                }
            }
        }
    }

    fn jump_back(&mut self) {
        match self.jumps.pop() {
            Some((path, line, col)) => self.jump_to_col(&path, line + 1, col),
            None => self.info("no earlier position"),
        }
    }

    // ----- symbols ---------------------------------------------------------------------------

    fn identifier_under_cursor(&self) -> Option<String> {
        let v = self.viewer.as_ref()?;
        let line = v.current_line()?;
        self.session
            .identifier_at(&v.path, line, v.column())
            .or_else(|| crate::symbols::word_at(v.lines.get(line)?, v.column()))
    }

    fn index_note(&mut self) {
        if !self.session.has_index() {
            let (files, symbols) = {
                let index = self.session.index();
                (index.file_count(), index.symbol_count())
            };
            self.info(format!("indexed {files} files, {symbols} symbols"));
        }
    }

    fn goto_definition(&mut self) {
        let Some(name) = self.identifier_under_cursor() else {
            self.info("no identifier under the cursor");
            return;
        };
        self.index_note();
        let defs = self.session.definitions(&name);
        match defs.len() {
            0 => self.info(format!("no definition of {name} in the repository")),
            1 => {
                let d = &defs[0];
                let (path, line, column) = (d.path.clone(), d.line, d.column);
                self.push_jump();
                self.jump_to_col(&path, line, column);
                self.info(format!("{} {name} in {path}:{line}", d.kind));
            }
            n => {
                let items = defs.into_iter().map(hit_from_symbol).collect();
                self.push_locations(format!("{n} definitions of {name}"), items);
            }
        }
    }

    fn find_references(&mut self) {
        let Some(name) = self.identifier_under_cursor() else {
            self.info("no identifier under the cursor");
            return;
        };
        self.index_note();
        let refs = self.session.references(&name);
        if refs.is_empty() {
            self.info(format!("no occurrence of {name}"));
            return;
        }
        let n = refs.len();
        let capped = n == crate::symbols::MAX_REFERENCES;
        let items = refs
            .into_iter()
            .map(|r| Hit {
                path: r.path,
                line: r.line,
                column: r.column,
                label: String::new(),
                text: r.text,
            })
            .collect();
        let title = if capped {
            format!("first {n} occurrences of {name}; there may be more")
        } else {
            format!("{n} occurrences of {name}")
        };
        self.push_locations(title, items);
    }

    fn file_symbols(&mut self) {
        let Some(path) = self.viewer.as_ref().map(|v| v.path.clone()) else {
            self.info("open a file first");
            return;
        };
        self.index_note();
        let symbols = self.session.symbols_in(&path);
        if symbols.is_empty() {
            self.info(format!("no symbols found in {path}"));
            return;
        }
        let items = symbols.into_iter().map(hit_from_symbol).collect();
        self.push_locations(format!("symbols in {path}"), items);
    }

    fn symbol_search(&mut self, query: &str) {
        self.index_note();
        let found = self.session.search_symbols(query);
        if found.is_empty() {
            self.info(format!("no symbol matches {query:?}"));
            return;
        }
        let n = found.len();
        let items = found.into_iter().map(hit_from_symbol).collect();
        self.push_locations(format!("{n} symbols matching {query:?}"), items);
    }

    fn ask_about_context(&mut self) {
        match self.screen() {
            Screen::Review(r) => {
                let Some(a) = Self::review_visible(r)
                    .get(r.items.cursor)
                    .map(|a| a.comment.clone())
                else {
                    return;
                };
                let text = prompts::address_comment(&a.path, a.line_label().as_deref(), &a.text);
                self.prompt = Some(Prompt::new(
                    PromptKind::Agent {
                        context: Vec::new(),
                    },
                    "ask the agent",
                    &text,
                ));
            }
            Screen::Diff(d) => {
                let text = prompts::review_diff(&d.view.file.path, &d.view.target.label());
                self.prompt = Some(Prompt::new(
                    PromptKind::Agent {
                        context: Vec::new(),
                    },
                    "ask the agent",
                    &text,
                ));
            }
            _ => {
                let Some(v) = &self.viewer else {
                    self.prompt = Some(Prompt::new(
                        PromptKind::Agent {
                            context: Vec::new(),
                        },
                        "ask the agent",
                        "",
                    ));
                    return;
                };
                if let Some(c) = v.current_comment().filter(|_| v.on_comment_row()) {
                    let c = &c.comment;
                    let text =
                        prompts::address_comment(&c.path, c.line_label().as_deref(), &c.text);
                    self.prompt = Some(Prompt::new(
                        PromptKind::Agent {
                            context: Vec::new(),
                        },
                        "ask the agent",
                        &text,
                    ));
                    return;
                }
                let Some((a, b)) = v.selection() else {
                    return;
                };
                let Some(lines) = v.lines.get(a..=b.min(v.lines.len().saturating_sub(1))) else {
                    self.info("the file is empty");
                    return;
                };
                let snippet = lines.join("\n");
                let uri = format!("file://{}", self.session.root().join(&v.path).display());
                let context = vec![(uri, snippet)];
                let label = format!(
                    "ask the agent about {}:{}",
                    v.path,
                    if b > a {
                        format!("{}-{}", a + 1, b + 1)
                    } else {
                        (a + 1).to_string()
                    }
                );
                self.prompt = Some(Prompt::new(PromptKind::Agent { context }, label, ""));
            }
        }
    }

    // ----- agent -----------------------------------------------------------------------------

    /// Brings up the agent panel, starting the agent if it is not running.
    pub fn open_agent(&mut self) {
        let config = self.session.config.agent.clone();
        let root = self.session.root().to_path_buf();
        self.agent.start(&config, &root);
        if !matches!(self.screen(), Screen::Agent) {
            self.screens.push(Screen::Agent);
        }
        self.agent.follow = true;
    }

    /// Sends `text` to the agent and shows the panel.
    pub fn ask_agent(&mut self, text: String, context: Vec<(String, String)>) {
        let config = self.session.config.agent.clone();
        let root = self.session.root().to_path_buf();
        self.agent.ask(text, context, &config, &root);
        self.open_agent();
    }

    /// True while there is agent activity worth polling quickly for.
    pub fn agent_active(&self) -> bool {
        self.agent.active()
    }

    /// Collects the agent's events. Returns whether anything changed.
    pub fn tick(&mut self) -> bool {
        self.agent.tick()
    }

    /// The configured agent's name, for a title or a hint.
    pub fn agent_label(&mut self) -> String {
        let config = self.session.config.agent.clone();
        self.agent.label(&config)
    }

    // ----- screens ---------------------------------------------------------------------------

    // ----- comments and notes ----------------------------------------------------------------

    fn start_comment(&mut self) {
        // The tree comments on whatever it has selected, file or directory, as a whole. Its
        // focus outlives the explorer, so only the explorer itself asks it.
        if self.focus_tree && matches!(self.screen(), Screen::Explorer) {
            if let Some(n) = self.tree.current().cloned() {
                self.start_path_comment(&n.path);
            }
            return;
        }
        let (path, lines) = match self.screen() {
            Screen::Diff(d) => {
                let Some(line) = d.view.after_line() else {
                    self.info("no line on the after side here");
                    return;
                };
                (d.view.file.path.clone(), Some((line + 1, line + 1)))
            }
            _ => {
                let Some(v) = &self.viewer else {
                    self.info("open a file first");
                    return;
                };
                // No line under the cursor means it sits on a comment on the whole file.
                (v.path.clone(), v.selection().map(|(a, b)| (a + 1, b + 1)))
            }
        };
        let Some((line, end)) = lines else {
            self.start_path_comment(&path.clone());
            return;
        };
        self.prompt = Some(Prompt::new(
            PromptKind::Comment {
                path: path.clone(),
                lines: Some((line, end)),
            },
            format!("comment on {path}:{}", review::line_label(line, Some(end))),
            "",
        ));
    }

    /// A comment on a whole file or directory, with no line range.
    fn start_path_comment(&mut self, path: &str) {
        self.prompt = Some(Prompt::new(
            PromptKind::Comment {
                path: path.to_string(),
                lines: None,
            },
            format!("comment on {path}"),
            "",
        ));
    }

    fn current_comment(&self) -> Option<Comment> {
        match self.screen() {
            Screen::Review(r) => Self::review_visible(r)
                .get(r.items.cursor)
                .map(|a| a.comment.clone()),
            Screen::Diff(d) => {
                let line = d.view.after_line()?;
                d.view
                    .comments
                    .iter()
                    .find(|c| c.covers_row(line))
                    .map(|a| a.comment.clone())
            }
            _ => self
                .viewer
                .as_ref()?
                .current_comment()
                .map(|a| a.comment.clone()),
        }
    }

    fn after_comment_change(&mut self) {
        self.refresh_viewer_comments();
        match self.screen() {
            Screen::Review(_) => self.refresh_review(),
            Screen::Notes(_) => self.refresh_notes(),
            _ => {}
        }
    }

    fn toggle_comment(&mut self) {
        let Some(c) = self.current_comment() else {
            self.info("no comment here");
            return;
        };
        let was_pending = c.is_pending();
        let outcome = self.session.toggle_comment(&c);
        if self.report(outcome).is_some() {
            self.after_comment_change();
            self.info(if was_pending { "completed" } else { "reopened" });
        }
    }

    fn delete_comment(&mut self) {
        let Some(c) = self.current_comment() else {
            self.info("no comment here");
            return;
        };
        let outcome = self.session.delete_comment(&c);
        if self.report(outcome).is_some() {
            self.last_deleted = Some(c);
            self.after_comment_change();
            self.info("deleted (u to undo)");
        }
    }

    fn undo_delete(&mut self) {
        let Some(c) = self.last_deleted.take() else {
            self.info("nothing to undo");
            return;
        };
        self.session.review.add(c);
        if self
            .report(self.session.review.save(self.session.root()))
            .is_some()
        {
            self.after_comment_change();
            self.info("restored");
        }
    }

    fn edit_comment(&mut self) {
        let Some(c) = self.current_comment() else {
            self.info("no comment here");
            return;
        };
        self.edit_target = Some(EditTarget::Comment(c.clone()));
        self.prompt = Some(Prompt::new(
            PromptKind::EditComment,
            format!("edit {}", c.location()),
            &c.text,
        ));
    }

    fn start_note(&mut self, general: bool) {
        let path = if general {
            None
        } else {
            match self.screen() {
                Screen::Diff(d) => Some(d.view.file.path.clone()),
                _ => self.viewer.as_ref().map(|v| v.path.clone()),
            }
        };
        if !general && path.is_none() {
            self.info("open a file first, or N for a general note");
            return;
        }
        let label = match &path {
            Some(p) => format!("note on {p}"),
            None => "general note".to_string(),
        };
        self.prompt = Some(Prompt::new(PromptKind::Note { path }, label, ""));
    }

    fn current_note(&self) -> Option<Note> {
        if let Screen::Notes(n) = self.screen() {
            return n.items.current().cloned();
        }
        None
    }

    fn edit_note(&mut self) {
        let Some(n) = self.current_note() else {
            return;
        };
        self.edit_target = Some(EditTarget::Note(n.clone()));
        self.prompt = Some(Prompt::new(
            PromptKind::EditNote,
            format!("edit note on {}", n.target),
            &n.text,
        ));
    }

    fn delete_note(&mut self) {
        let Some(n) = self.current_note() else {
            return;
        };
        let outcome = self.session.delete_note(&n);
        if self.report(outcome).is_some() {
            self.refresh_notes();
            self.info("note deleted");
        }
    }

    // ----- keys ------------------------------------------------------------------------------

    /// A key press, from the outermost layer inwards: whatever is over the screen, then the
    /// chord, then the keys that work everywhere, then the screen on top of the stack. Each
    /// layer that acts says so, and the ones below it do not see the key.
    pub fn handle_key(&mut self, key: KeyEvent) {
        self.message = None;
        if self.handle_overlay_key(key) {
            return;
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.quit = true;
            return;
        }
        if self.handle_chord_key(key) {
            return;
        }
        if self.handle_global_key(key) {
            return;
        }
        self.handle_screen_key(key);
    }

    /// A prompt, the colour scheme picker or the help page is over the screen and takes
    /// every key while it is there. True when one of them did.
    fn handle_overlay_key(&mut self, key: KeyEvent) -> bool {
        if self.prompt.is_some() {
            self.handle_prompt_key(key);
            return true;
        }
        if self.theme_picker.is_some() {
            self.handle_theme_picker_key(key);
            return true;
        }
        if self.show_help {
            match key.code {
                KeyCode::Char('j') | KeyCode::Down => self.help_scroll += 1,
                KeyCode::Char('k') | KeyCode::Up => {
                    self.help_scroll = self.help_scroll.saturating_sub(1)
                }
                _ => {
                    self.show_help = false;
                    self.help_scroll = 0;
                }
            }
            return true;
        }
        false
    }

    /// The `g` chord: `g` arms it anywhere, and the next key completes it. It is read before
    /// the keys that work everywhere, so `gr` does not refresh and `gS` does not open the
    /// changes list.
    fn handle_chord_key(&mut self, key: KeyEvent) -> bool {
        // A pending `g` claims the next key before the global bindings can (`gr` must not
        // refresh, `gS` must not open the changes list).
        if self.pending_g {
            self.pending_g = false;
            match key.code {
                // `gg` is "go to the top" on every screen: the same as Home.
                KeyCode::Char('g') => {
                    self.handle_key(KeyEvent::new(KeyCode::Home, KeyModifiers::NONE))
                }
                KeyCode::Char('d') => self.goto_definition(),
                KeyCode::Char('r') => self.find_references(),
                KeyCode::Char('s') => self.file_symbols(),
                KeyCode::Char('S') => {
                    self.prompt = Some(Prompt::new(PromptKind::SymbolSearch, "symbol", ""));
                }
                KeyCode::Char('t') => self.workspace_request = Some(WorkspaceRequest::NextRepo),
                KeyCode::Char('T') => self.workspace_request = Some(WorkspaceRequest::PrevRepo),
                _ => {}
            }
            return true;
        }
        // `g` arms the chord everywhere, so `gt`/`gT` switch repositories from any screen.
        if key.code == KeyCode::Char('g') && key.modifiers.is_empty() {
            self.pending_g = true;
            return true;
        }
        false
    }

    /// The keys that mean the same thing on every screen.
    fn handle_global_key(&mut self, key: KeyEvent) -> bool {
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            match key.code {
                KeyCode::Char('a') => {
                    self.open_agent();
                    return true;
                }
                KeyCode::Char('o') => {
                    self.jump_back();
                    return true;
                }
                _ => {}
            }
        }
        // Keys that work everywhere.
        match key.code {
            KeyCode::Char('?') => {
                self.show_help = true;
                return true;
            }
            // In the agent panel `t` toggles thoughts; the picker is a key away elsewhere.
            KeyCode::Char('t') if !matches!(self.screen(), Screen::Agent) => {
                self.open_theme_picker();
                return true;
            }
            KeyCode::Char('W') => {
                self.workspace_request = Some(WorkspaceRequest::PickRepo);
                return true;
            }
            // The agent panel from anywhere, for terminals (tmux with a Ctrl-a prefix) where
            // Ctrl-a never arrives. The review list keeps `A` for "address all".
            KeyCode::Char('A') if !matches!(self.screen(), Screen::Agent | Screen::Review(_)) => {
                self.open_agent();
                return true;
            }
            KeyCode::Char('i') if !matches!(self.screen(), Screen::Agent) => {
                self.open_agent();
                if self.agent.permission().is_none() {
                    self.prompt = Some(Prompt::new(
                        PromptKind::Agent {
                            context: Vec::new(),
                        },
                        "ask the agent",
                        "",
                    ));
                }
                return true;
            }
            KeyCode::Char('L') => {
                self.push_log(None);
                return true;
            }
            KeyCode::Char('R') if !matches!(self.screen(), Screen::Agent) => {
                self.push_review();
                return true;
            }
            KeyCode::Char('T') => {
                self.push_notes();
                return true;
            }
            KeyCode::Char('S') => {
                self.push_changes(DiffTarget::Working);
                return true;
            }
            KeyCode::Char('N') => {
                self.start_note(true);
                return true;
            }
            KeyCode::Char('r') if key.modifiers.is_empty() => {
                self.refresh_all();
                return true;
            }
            KeyCode::Char('h') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.highlight = !self.highlight;
                return true;
            }
            KeyCode::Char('u')
                if key.modifiers.is_empty()
                    && !matches!(self.screen(), Screen::Explorer | Screen::Diff(_)) =>
            {
                self.undo_delete();
                return true;
            }
            _ => {}
        }
        false
    }

    /// What is left goes to whichever screen is on top.
    fn handle_screen_key(&mut self, key: KeyEvent) {
        match self.screen() {
            Screen::Explorer => self.handle_explorer_key(key),
            Screen::Log(_) => self.handle_log_key(key),
            Screen::Changes(_) => self.handle_changes_key(key),
            Screen::Diff(_) => self.handle_diff_key(key),
            Screen::Review(_) => self.handle_review_key(key),
            Screen::Notes(_) => self.handle_notes_key(key),
            Screen::Locations(_) => self.handle_locations_key(key),
            Screen::Agent => {
                let height = self.measured.main_height;
                let config = self.session.config.agent.clone();
                let root = self.session.root().to_path_buf();
                match self.agent.handle_key(key, height, &config, &root) {
                    agent_panel::Action::Nothing => {}
                    agent_panel::Action::Close => self.pop_screen(),
                    agent_panel::Action::Say(text) => self.info(text),
                    agent_panel::Action::Ask(text) => {
                        self.prompt = Some(Prompt::new(
                            PromptKind::Agent {
                                context: Vec::new(),
                            },
                            "ask the agent",
                            &text,
                        ));
                    }
                }
            }
        }
    }

    fn pop_screen(&mut self) {
        if self.screens.len() > 1 {
            self.screens.pop();
        } else {
            self.quit = true;
        }
    }

    fn handle_explorer_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let half = (self.measured.main_height / 2).max(1) as isize;
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => {
                if self.viewer.as_ref().is_some_and(|v| v.visual.is_some()) {
                    if let Some(v) = &mut self.viewer {
                        v.visual = None;
                    }
                } else if self.viewer.as_ref().is_some_and(|v| v.search.is_some())
                    && key.code == KeyCode::Esc
                {
                    if let Some(v) = &mut self.viewer {
                        v.search = None;
                    }
                } else if !self.focus_tree {
                    self.focus_tree = true;
                } else if !self.tree.filter.is_empty() {
                    self.tree.filter.clear();
                    self.tree.rebuild();
                } else {
                    self.quit = true;
                }
            }
            KeyCode::Tab => {
                if self.viewer.is_some() {
                    self.focus_tree = !self.focus_tree;
                }
            }
            KeyCode::Char('f') if ctrl => {
                let was = self.tree.filter.clone();
                self.prompt = Some(Prompt::new(
                    PromptKind::Filter { was: was.clone() },
                    "filter",
                    &was,
                ));
            }
            _ if self.focus_tree => self.handle_tree_key(key),
            _ => self.handle_viewer_key(key, half),
        }
    }

    fn handle_tree_key(&mut self, key: KeyEvent) {
        let height = self.measured.tree_height.max(1) as isize;
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => self.tree.move_by(1),
            KeyCode::Char('k') | KeyCode::Up => self.tree.move_by(-1),
            KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.tree.move_by(height / 2)
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.tree.move_by(-height / 2)
            }
            KeyCode::PageDown => self.tree.move_by(height),
            KeyCode::PageUp => self.tree.move_by(-height),
            KeyCode::Char('g') | KeyCode::Home => self.tree.go_top(),
            KeyCode::Char('G') | KeyCode::End => self.tree.go_bottom(),
            KeyCode::Char('h') | KeyCode::Left => self.tree.collapse(),
            KeyCode::Char('l') | KeyCode::Right => {
                if self.tree.current().is_some_and(|n| n.is_dir) {
                    self.tree.expand();
                } else if self.viewer.is_some() {
                    self.focus_tree = false;
                }
            }
            KeyCode::Char('z') => self.tree.collapse_all(),
            KeyCode::Char('Z') => self.tree.expand_all(),
            KeyCode::Char('/') => {
                let was = self.tree.filter.clone();
                self.prompt = Some(Prompt::new(
                    PromptKind::Filter { was: was.clone() },
                    "filter",
                    &was,
                ));
            }
            KeyCode::Enter | KeyCode::Char(' ') => {
                if let Some(path) = self.tree.activate() {
                    self.open_file(&path);
                    if key.code == KeyCode::Enter {
                        self.focus_tree = false;
                    }
                }
            }
            KeyCode::Char('H') => {
                if let Some(n) = self.tree.current().filter(|n| !n.is_dir).cloned() {
                    self.push_log(Some(n.path));
                }
            }
            KeyCode::Char('D') => self.diff_current_file(),
            KeyCode::Char('c') => self.start_comment(),
            KeyCode::Char('n') => {
                if let Some(n) = self.tree.current().filter(|n| !n.is_dir).cloned() {
                    self.prompt = Some(Prompt::new(
                        PromptKind::Note {
                            path: Some(n.path.clone()),
                        },
                        format!("note on {}", n.path),
                        "",
                    ));
                }
            }
            KeyCode::Char('o') => {
                if let Some(n) = self.tree.current().filter(|n| !n.is_dir).cloned() {
                    self.editor_request = Some((self.session.root().join(&n.path), 1));
                }
            }
            _ => {}
        }
    }

    fn diff_current_file(&mut self) {
        let path = if self.focus_tree {
            self.tree
                .current()
                .filter(|n| !n.is_dir)
                .map(|n| n.path.clone())
        } else {
            self.viewer.as_ref().map(|v| v.path.clone())
        };
        let Some(path) = path else {
            return;
        };
        let Some(entry) = self.session.status.get(&path).cloned() else {
            self.info(format!("{path} is unchanged"));
            return;
        };
        let (target, status) = match (entry.unstaged, entry.staged) {
            (Some(s), _) => (DiffTarget::Working, s),
            (None, Some(s)) => (DiffTarget::Staged, s),
            _ => {
                self.info(format!("{path} is unchanged"));
                return;
            }
        };
        let file = ChangedFile {
            status,
            path,
            old_path: entry.old_path,
        };
        self.push_diff(target, vec![file], 0);
    }

    fn handle_viewer_key(&mut self, key: KeyEvent, half: isize) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let height = self.measured.main_height.max(1);
        let Some(v) = &mut self.viewer else {
            return;
        };
        v.code_width = self.measured.main_width.saturating_sub(v.gutter_width());
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => v.move_by(1),
            KeyCode::Char('k') | KeyCode::Up => v.move_by(-1),
            KeyCode::Char('d') if ctrl => v.move_lines(half),
            KeyCode::Char('u') if ctrl => v.move_lines(-half),
            KeyCode::Char('e') if ctrl => v.scroll_by(1, height),
            KeyCode::Char('y') if ctrl => v.scroll_by(-1, height),
            KeyCode::PageDown => v.move_lines(height as isize),
            KeyCode::PageUp => v.move_lines(-(height as isize)),
            KeyCode::Home => v.go_top(),
            KeyCode::Char('G') | KeyCode::End => v.go_bottom(),
            KeyCode::Char('h') | KeyCode::Left => v.move_col(-1),
            KeyCode::Char('l') | KeyCode::Right => v.move_col(1),
            KeyCode::Char('w') => v.word(true),
            KeyCode::Char('b') if !ctrl => v.word(false),
            KeyCode::Char('0') => v.col_home(),
            KeyCode::Char('^') => v.col_first_nonblank(),
            KeyCode::Char('$') => v.col_end(),
            // Ctrl-] reaches a terminal program as 0x1D, which crossterm reports as Ctrl-5.
            KeyCode::Char(']') | KeyCode::Char('5') if ctrl => self.goto_definition(),
            KeyCode::Char('*') => self.find_references(),
            KeyCode::Char('@') => self.file_symbols(),
            KeyCode::Char('#') => {
                self.prompt = Some(Prompt::new(PromptKind::SymbolSearch, "symbol", ""));
            }
            KeyCode::Char('a') => self.ask_about_context(),
            KeyCode::Char('V') | KeyCode::Char('v') => {
                v.visual = match v.visual {
                    Some(_) => None,
                    None => v.current_line(),
                };
            }
            KeyCode::Char('c') => self.start_comment(),
            KeyCode::Char('x') | KeyCode::Enter
                if v.on_comment_row() && key.code == KeyCode::Enter =>
            {
                self.edit_comment()
            }
            KeyCode::Char('x') => self.toggle_comment(),
            KeyCode::Char('d') => self.delete_comment(),
            KeyCode::Char('u') => self.undo_delete(),
            KeyCode::Char('e') => self.edit_comment(),
            KeyCode::Char('n') => self.start_note(false),
            KeyCode::Char(']') => v.next_comment(true),
            KeyCode::Char('[') => v.next_comment(false),
            KeyCode::Char('}') => v.next_change(true),
            KeyCode::Char('{') => v.next_change(false),
            KeyCode::Char('/') => {
                self.prompt = Some(Prompt::new(PromptKind::Search, "search", ""));
            }
            KeyCode::Char('>') => v.next_match(true),
            KeyCode::Char('<') => v.next_match(false),
            KeyCode::Char(':') => {
                self.prompt = Some(Prompt::new(PromptKind::GoToLine, "go to line", ""));
            }
            KeyCode::Char('H') => {
                let path = v.path.clone();
                self.push_log(Some(path));
            }
            KeyCode::Char('D') => self.diff_current_file(),
            KeyCode::Char('B') => {
                if v.blame.is_some() {
                    v.blame = None;
                } else {
                    let path = v.path.clone();
                    match self.session.repo.blame(&path) {
                        Ok(b) => {
                            if let Some(v) = &mut self.viewer {
                                v.blame = Some(b);
                            }
                        }
                        Err(e) => self.error(format!("{e:#}")),
                    }
                }
            }
            KeyCode::Char('o') => {
                let line = v.current_line().unwrap_or(0) + 1;
                self.editor_request = Some((self.session.root().join(&v.path), line));
            }
            _ => {}
        }
    }

    /// Mouse wheel over the main area.
    pub fn scroll_main(&mut self, delta: isize) {
        let height = self.measured.main_height.max(1);
        match self.screens.last_mut().expect("screen") {
            Screen::Explorer => {
                if self.focus_tree {
                    self.tree.move_by(delta);
                } else if let Some(v) = &mut self.viewer {
                    v.scroll_by(delta, height);
                }
            }
            Screen::Diff(d) => d.view.scroll_by(delta, height),
            Screen::Log(l) => {
                if l.focus_files {
                    l.files.move_by(delta);
                } else {
                    l.commits.move_by(delta);
                }
            }
            Screen::Changes(c) => c.files.move_by(delta),
            Screen::Locations(l) => l.items.move_by(delta),
            Screen::Agent => {
                self.agent.scroll = (self.agent.scroll as isize + delta).max(0) as usize;
                self.agent.follow = false;
            }
            Screen::Review(r) => {
                let n = Self::review_visible(r).len();
                r.items.cursor = (r.items.cursor as isize + delta)
                    .clamp(0, n.saturating_sub(1) as isize)
                    as usize;
            }
            Screen::Notes(n) => n.items.move_by(delta),
        }
    }
}
