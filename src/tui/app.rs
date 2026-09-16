//! Application state and key handling. Drawing lives in `ui.rs`; nothing here touches the
//! terminal.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::Instant;

use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde_json::Value;

use crate::agent::{Agent, Event as AgentEvent, PermissionOption, Role, prompts};
use crate::anchor::Anchored;
use crate::notes::Note;
use crate::repo::{ChangedFile, Commit, FileStatus};
use crate::review::{self, Comment};
use crate::session::{DiffTarget, Session};
use crate::theme::Theme;
use crate::tui::prompt::{Prompt, PromptKind};
use crate::tui::tree::Tree;
use crate::tui::viewer::{DiffLayout, DiffView, Viewer};

/// Commits fetched per page of a log.
const LOG_PAGE: usize = 200;

/// What the panel says it is starting: `claude`, or the ACP command line.
pub fn agent_label(config: &crate::config::AgentConfig) -> String {
    match config.kind.as_str() {
        "acp" => format!("{} {}", config.command, config.args.join(" ")),
        "claude" => config.claude_command.clone(),
        _ => {
            if std::process::Command::new(&config.claude_command)
                .arg("--version")
                .output()
                .is_ok()
            {
                config.claude_command.clone()
            } else {
                format!("{} {}", config.command, config.args.join(" "))
            }
        }
    }
}

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    User,
    Agent,
    Thought,
    Tool,
    System,
    /// Progress and diagnostics from the backend: what started, stderr, what a turn cost.
    Log,
}

#[derive(Debug, Clone)]
pub struct Entry {
    pub kind: EntryKind,
    pub text: String,
}

/// What is known about one tool call, merged from its updates.
#[derive(Debug, Clone, Default)]
struct ToolInfo {
    title: String,
    kind: Option<String>,
    status: Option<String>,
    locations: Vec<String>,
    output: Option<String>,
}

impl ToolInfo {
    fn render(&self) -> String {
        let mut line = self.title.clone();
        if let Some(k) = &self.kind {
            line = format!("{line} ({k})");
        }
        if let Some(s) = &self.status {
            line = format!("{line} [{s}]");
        }
        if !self.locations.is_empty() {
            line = format!("{line} {}", self.locations.join(", "));
        }
        if let Some(out) = &self.output {
            let short: String = out.chars().take(600).collect();
            line.push('\n');
            line.push_str(&short);
            if out.len() > short.len() {
                line.push('…');
            }
        }
        line
    }
}

/// The agent conversation, kept on the app so it survives leaving the screen.
#[derive(Default)]
pub struct AgentState {
    pub entries: Vec<Entry>,
    pub scroll: usize,
    /// Keep the view at the bottom as text streams in.
    pub follow: bool,
    pub permission: Option<(Value, String, Vec<PermissionOption>)>,
    /// Tool call id to the entry showing it, for status updates.
    tools: HashMap<String, (usize, ToolInfo)>,
    pub status: String,
    pub show_thoughts: bool,
    pub show_log: bool,
    /// What the backend says it is doing right now.
    pub phase: String,
    /// When the agent started starting; cleared once it is connected.
    pub start_began: Option<Instant>,
    /// When the running turn was sent.
    pub turn_started: Option<Instant>,
    /// When the backend last said anything, to show a long silence for what it is.
    pub last_event: Option<Instant>,
    /// Tool calls seen in the running turn.
    pub turn_tools: usize,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost_usd: Option<f64>,
}

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
    pub agent: Option<Agent>,
    /// The agent is starting on another thread; the result arrives here.
    agent_starting: Option<Receiver<Result<Agent>>>,
    pub agent_state: AgentState,
    /// A prompt sent before the agent finished starting.
    queued_prompt: Option<(String, Vec<(String, String)>)>,
    /// What the configured agent is called, found out once: the `auto` kind probes PATH.
    agent_label: Option<String>,
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
            agent: None,
            agent_starting: None,
            agent_state: AgentState {
                follow: true,
                show_log: true,
                status: "not started".into(),
                ..AgentState::default()
            },
            queued_prompt: None,
            agent_label: None,
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
        let selected = self.tree.current().map(|n| n.path.clone());
        let filter = self.tree.filter.clone();
        self.tree = Tree::new(&self.session.files);
        self.tree.filter = filter;
        self.tree.rebuild();
        if let Some(path) = selected {
            self.tree.select(&path);
        }
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
        self.push_locations(format!("{n} occurrences of {name}"), items);
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

    fn push_locations(&mut self, title: String, items: Vec<Hit>) {
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

    fn handle_locations_key(&mut self, key: KeyEvent) {
        let height = self.measured.list_height.max(1) as isize;
        let Screen::Locations(state) = self.screens.last_mut().expect("screen") else {
            return;
        };
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => self.pop_screen(),
            KeyCode::Char('j') | KeyCode::Down => state.items.move_by(1),
            KeyCode::Char('k') | KeyCode::Up => state.items.move_by(-1),
            KeyCode::PageDown => state.items.move_by(height),
            KeyCode::PageUp => state.items.move_by(-height),
            KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                state.items.move_by(height / 2)
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                state.items.move_by(-height / 2)
            }
            KeyCode::Char('g') | KeyCode::Home => state.items.cursor = 0,
            KeyCode::Char('G') | KeyCode::End => {
                state.items.cursor = state.items.items.len().saturating_sub(1)
            }
            KeyCode::Enter | KeyCode::Char('o') | KeyCode::Char('l') => {
                if let Some(hit) = state.items.current().cloned() {
                    self.push_jump();
                    self.jump_to_col(&hit.path, hit.line, hit.column);
                }
            }
            _ => {}
        }
    }

    // ----- agent -----------------------------------------------------------------------------

    /// Starts the configured agent on another thread; `tick` collects the result.
    fn ensure_agent(&mut self) {
        if self.agent.is_some() || self.agent_starting.is_some() {
            return;
        }
        let config = self.session.config.agent.clone();
        let root = self.session.root().to_path_buf();
        let description = format!("starting {}", self.agent_label());
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(crate::agent::spawn(&config, &root));
        });
        self.agent_starting = Some(rx);
        self.agent_state.status = "starting".into();
        self.agent_state.start_began = Some(Instant::now());
        self.system_entry(description);
    }

    /// Bookkeeping for a prompt that was just sent.
    fn begin_turn(&mut self) {
        let state = &mut self.agent_state;
        state.status = "working".into();
        state.phase = "prompt sent".into();
        state.turn_started = Some(Instant::now());
        state.last_event = Some(Instant::now());
        state.turn_tools = 0;
    }

    /// One line of progress for the title and the status bar: the phase, how long the turn
    /// has run, how long the backend has been silent, tool calls so far. `None` when idle.
    pub fn agent_progress(&self) -> Option<String> {
        let state = &self.agent_state;
        if let Some(t) = state.start_began {
            return Some(format!("starting · {}s", t.elapsed().as_secs()));
        }
        let busy = self.agent.as_ref().is_some_and(|a| a.busy());
        if !busy && state.permission.is_none() {
            return None;
        }
        let mut parts = Vec::new();
        if state.permission.is_some() {
            parts.push("waiting for permission".to_string());
        } else if !state.phase.is_empty() {
            parts.push(state.phase.clone());
        }
        if let Some(t) = state.turn_started {
            parts.push(format!("{}s", t.elapsed().as_secs()));
        }
        if let Some(t) = state.last_event {
            let quiet = t.elapsed().as_secs();
            if quiet >= 5 && state.permission.is_none() {
                parts.push(format!("silent for {quiet}s"));
            }
        }
        if state.turn_tools > 0 {
            parts.push(format!(
                "{} tool call{}",
                state.turn_tools,
                if state.turn_tools == 1 { "" } else { "s" }
            ));
        }
        Some(parts.join(" · "))
    }

    /// `12.3k in / 1.1k out · $0.21` once the backend has reported any usage.
    pub fn agent_usage(&self) -> Option<String> {
        let state = &self.agent_state;
        if state.input_tokens == 0 && state.output_tokens == 0 && state.cost_usd.is_none() {
            return None;
        }
        let mut text = format!(
            "{} in / {} out",
            crate::claude::count(state.input_tokens),
            crate::claude::count(state.output_tokens)
        );
        if let Some(cost) = state.cost_usd {
            text.push_str(&format!(" · ${cost:.2}"));
        }
        Some(text)
    }

    /// The configured agent's name for titles and hints, computed on first use.
    pub fn agent_label(&mut self) -> String {
        if self.agent_label.is_none() {
            self.agent_label = Some(agent_label(&self.session.config.agent));
        }
        self.agent_label.clone().unwrap_or_default()
    }

    fn system_entry(&mut self, text: impl Into<String>) {
        self.agent_state.entries.push(Entry {
            kind: EntryKind::System,
            text: text.into(),
        });
    }

    pub fn open_agent(&mut self) {
        self.ensure_agent();
        if !matches!(self.screen(), Screen::Agent) {
            self.screens.push(Screen::Agent);
        }
        self.agent_state.follow = true;
    }

    /// Sends `text` (with embedded `context`) to the agent, starting it first if needed.
    pub fn ask_agent(&mut self, text: String, context: Vec<(String, String)>) {
        self.agent_state.entries.push(Entry {
            kind: EntryKind::User,
            text: text.clone(),
        });
        match &mut self.agent {
            Some(agent) => {
                if let Err(e) = agent.prompt(&text, &context) {
                    let msg = format!("{e:#}");
                    self.system_entry(msg);
                } else {
                    self.begin_turn();
                }
            }
            None => {
                self.queued_prompt = Some((text, context));
                self.ensure_agent();
            }
        }
        self.open_agent();
    }

    /// True while there is agent activity worth polling quickly for.
    pub fn agent_active(&self) -> bool {
        self.agent_starting.is_some() || self.agent.as_ref().is_some_and(|a| a.busy())
    }

    /// Collects agent start-up results and events. Returns whether anything changed.
    pub fn tick(&mut self) -> bool {
        let mut changed = false;
        if let Some(rx) = &self.agent_starting {
            match rx.try_recv() {
                Ok(Ok(agent)) => {
                    self.agent_starting = None;
                    let name = agent.name().to_string();
                    self.agent = Some(agent);
                    self.agent_state.status = "idle".into();
                    self.agent_state.start_began = None;
                    self.system_entry(format!("connected to {name}"));
                    if let Some((text, context)) = self.queued_prompt.take() {
                        let outcome = match &mut self.agent {
                            Some(agent) => agent.prompt(&text, &context),
                            None => Ok(()),
                        };
                        match outcome {
                            Ok(()) => self.begin_turn(),
                            Err(e) => self.system_entry(format!("{e:#}")),
                        }
                    }
                    changed = true;
                }
                Ok(Err(e)) => {
                    self.agent_starting = None;
                    self.queued_prompt = None;
                    self.agent_state.status = "failed to start".into();
                    self.agent_state.start_began = None;
                    self.system_entry(format!("could not start the agent: {e:#}"));
                    changed = true;
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => {
                    self.agent_starting = None;
                    self.agent_state.status = "failed to start".into();
                    self.agent_state.start_began = None;
                    changed = true;
                }
            }
        }
        let events = match &mut self.agent {
            Some(agent) => agent.poll(),
            None => Vec::new(),
        };
        for event in events {
            self.apply_agent_event(event);
            changed = true;
        }
        changed
    }

    fn apply_agent_event(&mut self, event: AgentEvent) {
        let state = &mut self.agent_state;
        state.last_event = Some(Instant::now());
        match event {
            AgentEvent::Text { role, text } => {
                let kind = match role {
                    Role::Agent => EntryKind::Agent,
                    Role::Thought => EntryKind::Thought,
                    Role::User => EntryKind::User,
                };
                match state.entries.last_mut() {
                    Some(last) if last.kind == kind && kind != EntryKind::User => {
                        last.text.push_str(&text)
                    }
                    _ => state.entries.push(Entry { kind, text }),
                }
            }
            AgentEvent::ToolCall {
                id,
                title,
                kind,
                status,
                locations,
                output,
            } => {
                if !state.tools.contains_key(&id) {
                    state.entries.push(Entry {
                        kind: EntryKind::Tool,
                        text: String::new(),
                    });
                    let i = state.entries.len() - 1;
                    state.tools.insert(id.clone(), (i, ToolInfo::default()));
                    state.turn_tools += 1;
                }
                let (idx, info) = state.tools.get_mut(&id).expect("present");
                if let Some(t) = title {
                    info.title = t;
                }
                if kind.is_some() {
                    info.kind = kind;
                }
                if status.is_some() {
                    info.status = status;
                }
                if !locations.is_empty() {
                    info.locations = locations;
                }
                if output.is_some() {
                    info.output = output;
                }
                state.entries[*idx].text = info.render();
            }
            AgentEvent::Plan { entries } => {
                let text = entries
                    .iter()
                    .map(|(c, s)| format!("[{s}] {c}"))
                    .collect::<Vec<_>>()
                    .join("\n");
                state.entries.push(Entry {
                    kind: EntryKind::System,
                    text: format!("plan:\n{text}"),
                });
            }
            AgentEvent::Permission {
                request_id,
                title,
                details,
                options,
            } => {
                let names: Vec<String> = options
                    .iter()
                    .enumerate()
                    .map(|(i, o)| format!("{} {}", i + 1, o.name))
                    .collect();
                // The whole of what is being approved goes in the transcript: the bar below
                // has room for one line, and a command's second line is where something
                // unwanted would hide.
                let full = match &details {
                    Some(d) if d.trim() != title.trim() => format!("permission: {title}\n{d}"),
                    _ => format!("permission: {title}"),
                };
                state.entries.push(Entry {
                    kind: EntryKind::System,
                    text: format!("{full}\n({})", names.join("  ")),
                });
                state.permission = Some((request_id, title, options));
                state.status = "waiting for permission".into();
            }
            AgentEvent::TurnDone { stop_reason } => {
                state.status = if stop_reason == "end_turn" {
                    "idle".into()
                } else {
                    format!("stopped: {stop_reason}")
                };
                state.tools.clear();
                state.phase.clear();
                state.turn_started = None;
            }
            AgentEvent::Error { message } => {
                state.status = "error".into();
                state.entries.push(Entry {
                    kind: EntryKind::System,
                    text: format!("error: {message}"),
                });
            }
            AgentEvent::Stderr { text } => state.entries.push(Entry {
                kind: EntryKind::Log,
                text: format!("stderr: {text}"),
            }),
            AgentEvent::Log { text } => state.entries.push(Entry {
                kind: EntryKind::Log,
                text,
            }),
            AgentEvent::Status { text } => state.phase = text,
            AgentEvent::Usage {
                input_tokens,
                output_tokens,
                cost_usd,
            } => {
                state.input_tokens += input_tokens;
                state.output_tokens += output_tokens;
                if cost_usd.is_some() {
                    state.cost_usd = cost_usd;
                }
            }
            AgentEvent::Exited { message } => {
                state.status = "exited".into();
                state.permission = None;
                state.phase.clear();
                state.turn_started = None;
                state.entries.push(Entry {
                    kind: EntryKind::System,
                    text: message,
                });
                self.agent = None;
            }
        }
    }

    fn answer_permission(&mut self, choice: Option<usize>) {
        let Some((id, _, options)) = self.agent_state.permission.take() else {
            return;
        };
        let option = choice.and_then(|i| options.get(i));
        let Some(agent) = &mut self.agent else {
            return;
        };
        let outcome = agent.respond_permission(&id, option.map(|o| o.option_id.as_str()));
        let label = option
            .map(|o| o.name.clone())
            .unwrap_or_else(|| "cancelled".into());
        self.agent_state.status = "working".into();
        match outcome {
            Ok(()) => self.system_entry(format!("answered: {label}")),
            Err(e) => self.system_entry(format!("{e:#}")),
        }
    }

    /// `a` outside the agent screen: a prompt about what is under the cursor.
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

    fn handle_agent_key(&mut self, key: KeyEvent) {
        let height = self.measured.main_height.max(1);
        match key.code {
            KeyCode::Char('q') => self.pop_screen(),
            KeyCode::Esc => {
                if self.agent.as_ref().is_some_and(|a| a.busy()) {
                    if let Some(agent) = &mut self.agent {
                        let outcome = agent.cancel();
                        self.report(outcome);
                        self.info("cancelling");
                    }
                } else {
                    self.pop_screen();
                }
            }
            KeyCode::Char('i') | KeyCode::Char('a') | KeyCode::Enter => {
                if self.agent_state.permission.is_some() {
                    self.info("answer the permission request first (1-9, y, n)");
                    return;
                }
                self.prompt = Some(Prompt::new(
                    PromptKind::Agent {
                        context: Vec::new(),
                    },
                    "ask the agent",
                    "",
                ));
            }
            KeyCode::Char(c @ '1'..='9') if self.agent_state.permission.is_some() => {
                self.answer_permission(Some(c as usize - '1' as usize));
            }
            KeyCode::Char('y') if self.agent_state.permission.is_some() => {
                let i = self
                    .agent_state
                    .permission
                    .as_ref()
                    .and_then(|(_, _, o)| o.iter().position(|x| x.kind.starts_with("allow")));
                self.answer_permission(i);
            }
            KeyCode::Char('n') if self.agent_state.permission.is_some() => {
                let i = self
                    .agent_state
                    .permission
                    .as_ref()
                    .and_then(|(_, _, o)| o.iter().position(|x| x.kind.starts_with("reject")));
                self.answer_permission(i);
            }
            KeyCode::Char('j') | KeyCode::Down => {
                self.agent_state.scroll += 1;
                self.agent_state.follow = false;
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.agent_state.scroll = self.agent_state.scroll.saturating_sub(1);
                self.agent_state.follow = false;
            }
            KeyCode::PageDown | KeyCode::Char('d')
                if key.code == KeyCode::PageDown
                    || key.modifiers.contains(KeyModifiers::CONTROL) =>
            {
                self.agent_state.scroll += height / 2;
                self.agent_state.follow = false;
            }
            KeyCode::PageUp | KeyCode::Char('u')
                if key.code == KeyCode::PageUp || key.modifiers.contains(KeyModifiers::CONTROL) =>
            {
                self.agent_state.scroll = self.agent_state.scroll.saturating_sub(height / 2);
                self.agent_state.follow = false;
            }
            KeyCode::Char('G') | KeyCode::End => self.agent_state.follow = true,
            KeyCode::Char('g') | KeyCode::Home => {
                self.agent_state.scroll = 0;
                self.agent_state.follow = false;
            }
            KeyCode::Char('t') => self.agent_state.show_thoughts = !self.agent_state.show_thoughts,
            KeyCode::Char('l') => self.agent_state.show_log = !self.agent_state.show_log,
            KeyCode::Char('C') => {
                self.agent_state.entries.clear();
                self.agent_state.tools.clear();
                self.agent_state.scroll = 0;
            }
            KeyCode::Char('R') => {
                self.agent = None;
                self.agent_state.permission = None;
                self.system_entry("restarting");
                self.ensure_agent();
            }
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
            _ => {}
        }
    }

    // ----- screens ---------------------------------------------------------------------------

    fn push_log(&mut self, path: Option<String>) {
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
        self.load_commit_files(&mut state);
        self.screens.push(Screen::Log(state));
    }

    fn load_commit_files(&mut self, state: &mut LogState) {
        let Some(commit) = state.commits.current() else {
            return;
        };
        if state.files_of.as_deref() == Some(commit.hash.as_str()) {
            return;
        }
        let hash = commit.hash.clone();
        match self.session.commit_files(&hash) {
            Ok(mut files) => {
                if let Some(path) = &state.path {
                    // Put the file whose history this is first.
                    files.sort_by_key(|f| f.path != *path);
                }
                state.files = ListState::new(files);
                state.files_of = Some(hash);
            }
            Err(e) => self.error(format!("{e:#}")),
        }
    }

    fn load_more_commits(&mut self, state: &mut LogState) {
        if state.exhausted {
            return;
        }
        let skip = state.commits.items.len();
        let more = match &state.path {
            Some(p) => self.session.file_log(p, skip, LOG_PAGE),
            None => self.session.log(skip, LOG_PAGE),
        };
        if let Some(more) = self.report(more) {
            if more.len() < LOG_PAGE {
                state.exhausted = true;
            }
            state.commits.items.extend(more);
        }
    }

    fn push_changes(&mut self, target: DiffTarget) {
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

    fn push_diff(&mut self, target: DiffTarget, siblings: Vec<ChangedFile>, index: usize) {
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

    fn replace_diff(&mut self, index: usize) {
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

    fn push_review(&mut self) {
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

    fn refresh_review(&mut self) {
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
            r.items.cursor = cursor.min(r.items.items.len().saturating_sub(1));
        }
    }

    fn push_notes(&mut self) {
        let items: Vec<Note> = self.session.notes.notes().into_iter().cloned().collect();
        self.screens.push(Screen::Notes(NotesState {
            items: ListState::new(items),
        }));
    }

    fn refresh_notes(&mut self) {
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

    // ----- theme -----------------------------------------------------------------------------

    fn rehighlight(&mut self) {
        let syntax = self.theme.syntax.clone();
        if let Some(v) = &mut self.viewer {
            v.rehighlight(&syntax);
        }
        for screen in &mut self.screens {
            if let Screen::Diff(d) = screen {
                d.view.rehighlight(&syntax);
            }
        }
    }

    fn apply_theme(&mut self, theme: Theme) {
        if theme.syntax != self.theme.syntax {
            self.theme = theme;
            self.rehighlight();
        } else {
            self.theme = theme;
        }
    }

    /// Takes over the theme another repository's app saved.
    pub fn adopt_theme(&mut self, theme: Theme) {
        self.session.adopt_theme(theme.clone());
        self.apply_theme(theme);
    }

    /// Takes over the diff layout another repository's app saved.
    pub fn adopt_layout(&mut self, layout: DiffLayout) {
        self.layout = layout;
        self.session.adopt_layout(layout.name());
    }

    fn open_theme_picker(&mut self) {
        let index = crate::theme::all()
            .iter()
            .position(|t| t.name == self.theme.name)
            .unwrap_or(0);
        self.theme_picker = Some((index, self.theme.clone()));
    }

    fn handle_theme_picker_key(&mut self, key: KeyEvent) {
        let Some((index, previous)) = self.theme_picker.clone() else {
            return;
        };
        let themes = crate::theme::all();
        let select = |app: &mut App, i: usize| {
            let i = i.min(themes.len() - 1);
            app.theme_picker = Some((i, previous.clone()));
            app.apply_theme(themes[i].clone());
        };
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => select(self, index + 1),
            KeyCode::Char('k') | KeyCode::Up => select(self, index.saturating_sub(1)),
            KeyCode::Char('g') | KeyCode::Home => select(self, 0),
            KeyCode::Char('G') | KeyCode::End => select(self, themes.len() - 1),
            KeyCode::Enter => {
                let name = self.theme.name.clone();
                self.theme_picker = None;
                let outcome = self.session.set_theme(&name);
                if self.report(outcome).is_some() {
                    self.info(format!("theme {name} saved to the config file"));
                }
            }
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('t') => {
                self.theme_picker = None;
                self.apply_theme(previous);
            }
            _ => {}
        }
    }

    // ----- prompt ----------------------------------------------------------------------------

    fn submit_prompt(&mut self) {
        let Some(prompt) = self.prompt.take() else {
            return;
        };
        let text = prompt.text.clone();
        match prompt.kind {
            PromptKind::Comment { path, lines } => {
                if text.trim().is_empty() {
                    self.info("empty comment discarded");
                    return;
                }
                let source = match (self.screen(), lines) {
                    (Screen::Diff(d), Some((line, _))) => d.view.after_lines.get(line - 1).cloned(),
                    _ => None,
                };
                let outcome = self
                    .session
                    .add_comment(&path, lines, &text, source.as_deref());
                if self.report(outcome).is_some() {
                    if let Some(v) = &mut self.viewer {
                        v.visual = None;
                    }
                    self.after_comment_change();
                    self.info("comment added to REVIEW.md");
                }
            }
            PromptKind::EditComment => {
                if let Some(EditTarget::Comment(c)) = self.edit_target.take() {
                    if text.trim().is_empty() {
                        self.info("empty comment; use d to delete");
                        return;
                    }
                    let outcome = self.session.edit_comment(&c, &text);
                    if self.report(outcome).is_some() {
                        self.after_comment_change();
                        self.info("comment updated");
                    }
                }
            }
            PromptKind::Note { path } => {
                if text.trim().is_empty() {
                    self.info("empty note discarded");
                    return;
                }
                let outcome = self.session.add_note(path.as_deref(), &text);
                if self.report(outcome).is_some() {
                    self.refresh_notes();
                    self.info("note added to NOTES.md");
                }
            }
            PromptKind::EditNote => {
                if let Some(EditTarget::Note(n)) = self.edit_target.take() {
                    if text.trim().is_empty() {
                        return;
                    }
                    let outcome = self.session.edit_note(&n, &text);
                    if self.report(outcome).is_some() {
                        self.refresh_notes();
                        self.info("note updated");
                    }
                }
            }
            PromptKind::Search => {
                let query = if text.is_empty() {
                    self.last_search.clone()
                } else {
                    text
                };
                self.last_search = query.clone();
                let n = match self.screen_mut() {
                    Screen::Diff(d) => {
                        let n = d.view.set_search(&query);
                        d.view.next_match(true);
                        n
                    }
                    _ => match &mut self.viewer {
                        Some(v) => {
                            let n = v.set_search(&query);
                            v.nearest_match();
                            n
                        }
                        None => 0,
                    },
                };
                self.info(format!("{n} match(es) for {query:?}"));
            }
            PromptKind::GoToLine => {
                if let Ok(n) = text.trim().parse::<usize>() {
                    match self.screen_mut() {
                        Screen::Diff(d) => d.view.go_to_after_line(n.saturating_sub(1)),
                        _ => {
                            if let Some(v) = &mut self.viewer {
                                v.go_to_line(n.saturating_sub(1));
                            }
                        }
                    }
                }
            }
            PromptKind::Filter => {
                self.tree.filter = text;
                self.tree.rebuild();
            }
            PromptKind::ConfirmDelete => {}
            PromptKind::Agent { context } => {
                if text.trim().is_empty() {
                    return;
                }
                self.ask_agent(text, context);
            }
            PromptKind::SymbolSearch => {
                if !text.trim().is_empty() {
                    self.symbol_search(text.trim());
                }
            }
        }
    }

    fn handle_prompt_key(&mut self, key: KeyEvent) {
        let Some(p) = &mut self.prompt else {
            return;
        };
        match (key.code, key.modifiers) {
            (KeyCode::Esc, _) => {
                let was_filter = matches!(p.kind, PromptKind::Filter);
                self.prompt = None;
                self.edit_target = None;
                if was_filter {
                    self.tree.rebuild();
                }
            }
            (KeyCode::Enter, _) => self.submit_prompt(),
            (KeyCode::Char('c'), KeyModifiers::CONTROL) => {
                self.prompt = None;
                self.edit_target = None;
            }
            (KeyCode::Char('u'), KeyModifiers::CONTROL) => {
                p.text.clear();
                p.cursor = 0;
            }
            (KeyCode::Char('w'), KeyModifiers::CONTROL) => p.delete_word(),
            (KeyCode::Char('a'), KeyModifiers::CONTROL) | (KeyCode::Home, _) => p.home(),
            (KeyCode::Char('e'), KeyModifiers::CONTROL) | (KeyCode::End, _) => p.end(),
            (KeyCode::Left, _) => p.left(),
            (KeyCode::Right, _) => p.right(),
            (KeyCode::Backspace, _) => {
                p.backspace();
                if matches!(p.kind, PromptKind::Filter) {
                    self.tree.filter = p.text.clone();
                    self.tree.rebuild();
                }
            }
            (KeyCode::Delete, _) => p.delete(),
            (KeyCode::Char(c), m) if !m.contains(KeyModifiers::CONTROL) => {
                p.insert(c);
                if matches!(p.kind, PromptKind::Filter) {
                    self.tree.filter = p.text.clone();
                    self.tree.rebuild();
                }
            }
            _ => {}
        }
    }

    // ----- keys ------------------------------------------------------------------------------

    pub fn handle_key(&mut self, key: KeyEvent) {
        self.message = None;
        if self.prompt.is_some() {
            self.handle_prompt_key(key);
            return;
        }
        if self.theme_picker.is_some() {
            self.handle_theme_picker_key(key);
            return;
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
            return;
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.quit = true;
            return;
        }
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
            return;
        }
        // `g` arms the chord everywhere, so `gt`/`gT` switch repositories from any screen.
        if key.code == KeyCode::Char('g') && key.modifiers.is_empty() {
            self.pending_g = true;
            return;
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            match key.code {
                KeyCode::Char('a') => {
                    self.open_agent();
                    return;
                }
                KeyCode::Char('o') => {
                    self.jump_back();
                    return;
                }
                _ => {}
            }
        }
        // Keys that work everywhere.
        match key.code {
            KeyCode::Char('?') => {
                self.show_help = true;
                return;
            }
            // In the agent panel `t` toggles thoughts; the picker is a key away elsewhere.
            KeyCode::Char('t') if !matches!(self.screen(), Screen::Agent) => {
                self.open_theme_picker();
                return;
            }
            KeyCode::Char('W') => {
                self.workspace_request = Some(WorkspaceRequest::PickRepo);
                return;
            }
            // The agent panel from anywhere, for terminals (tmux with a Ctrl-a prefix) where
            // Ctrl-a never arrives. The review list keeps `A` for "address all".
            KeyCode::Char('A') if !matches!(self.screen(), Screen::Agent | Screen::Review(_)) => {
                self.open_agent();
                return;
            }
            KeyCode::Char('i') if !matches!(self.screen(), Screen::Agent) => {
                self.open_agent();
                if self.agent_state.permission.is_none() {
                    self.prompt = Some(Prompt::new(
                        PromptKind::Agent {
                            context: Vec::new(),
                        },
                        "ask the agent",
                        "",
                    ));
                }
                return;
            }
            KeyCode::Char('L') => {
                self.push_log(None);
                return;
            }
            KeyCode::Char('R') if !matches!(self.screen(), Screen::Agent) => {
                self.push_review();
                return;
            }
            KeyCode::Char('T') => {
                self.push_notes();
                return;
            }
            KeyCode::Char('S') => {
                self.push_changes(DiffTarget::Working);
                return;
            }
            KeyCode::Char('N') => {
                self.start_note(true);
                return;
            }
            KeyCode::Char('r') if key.modifiers.is_empty() => {
                self.refresh_all();
                return;
            }
            KeyCode::Char('h') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.highlight = !self.highlight;
                return;
            }
            KeyCode::Char('u')
                if key.modifiers.is_empty()
                    && !matches!(self.screen(), Screen::Explorer | Screen::Diff(_)) =>
            {
                self.undo_delete();
                return;
            }
            _ => {}
        }
        match self.screen() {
            Screen::Explorer => self.handle_explorer_key(key),
            Screen::Log(_) => self.handle_log_key(key),
            Screen::Changes(_) => self.handle_changes_key(key),
            Screen::Diff(_) => self.handle_diff_key(key),
            Screen::Review(_) => self.handle_review_key(key),
            Screen::Notes(_) => self.handle_notes_key(key),
            Screen::Locations(_) => self.handle_locations_key(key),
            Screen::Agent => self.handle_agent_key(key),
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
                self.prompt = Some(Prompt::new(
                    PromptKind::Filter,
                    "filter",
                    &self.tree.filter.clone(),
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
                self.prompt = Some(Prompt::new(
                    PromptKind::Filter,
                    "filter",
                    &self.tree.filter.clone(),
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

    fn handle_log_key(&mut self, key: KeyEvent) {
        let height = self.measured.list_height.max(1) as isize;
        let Screen::Log(state) = self.screens.last_mut().expect("screen") else {
            return;
        };
        let mut state_owned = None;
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
            KeyCode::Char('j') | KeyCode::Down => {
                if state.focus_files {
                    state.files.move_by(1);
                } else {
                    let at_end = state.commits.cursor + 1 >= state.commits.items.len();
                    state.commits.move_by(1);
                    if at_end {
                        state_owned = Some(self.screens.pop());
                    }
                }
            }
            KeyCode::Char('k') | KeyCode::Up => {
                if state.focus_files {
                    state.files.move_by(-1);
                } else {
                    state.commits.move_by(-1);
                }
            }
            KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                if state.focus_files {
                    state.files.move_by(height / 2)
                } else {
                    state.commits.move_by(height / 2)
                }
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                if state.focus_files {
                    state.files.move_by(-height / 2)
                } else {
                    state.commits.move_by(-height / 2)
                }
            }
            KeyCode::PageDown => {
                if state.focus_files {
                    state.files.move_by(height)
                } else {
                    state.commits.move_by(height)
                }
            }
            KeyCode::PageUp => {
                if state.focus_files {
                    state.files.move_by(-height)
                } else {
                    state.commits.move_by(-height)
                }
            }
            KeyCode::Char('g') | KeyCode::Home => {
                if state.focus_files {
                    state.files.cursor = 0
                } else {
                    state.commits.cursor = 0
                }
            }
            KeyCode::Char('G') | KeyCode::End => {
                if state.focus_files {
                    state.files.cursor = state.files.items.len().saturating_sub(1);
                } else {
                    state.commits.cursor = state.commits.items.len().saturating_sub(1);
                    state_owned = Some(self.screens.pop());
                }
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
        // Selection moved: reload the file list and, at the end of the list, the next page.
        match state_owned {
            Some(Some(Screen::Log(mut st))) => {
                self.load_more_commits(&mut st);
                self.load_commit_files(&mut st);
                self.screens.push(Screen::Log(st));
            }
            Some(other) => {
                if let Some(s) = other {
                    self.screens.push(s);
                }
            }
            None => {
                if let Some(Screen::Log(mut st)) = self.screens.pop() {
                    self.load_commit_files(&mut st);
                    self.screens.push(Screen::Log(st));
                }
            }
        }
    }

    fn handle_changes_key(&mut self, key: KeyEvent) {
        let height = self.measured.list_height.max(1) as isize;
        let Screen::Changes(state) = self.screens.last_mut().expect("screen") else {
            return;
        };
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => self.pop_screen(),
            KeyCode::Char('j') | KeyCode::Down => state.files.move_by(1),
            KeyCode::Char('k') | KeyCode::Up => state.files.move_by(-1),
            KeyCode::PageDown => state.files.move_by(height),
            KeyCode::PageUp => state.files.move_by(-height),
            KeyCode::Char('g') | KeyCode::Home => state.files.cursor = 0,
            KeyCode::Char('G') | KeyCode::End => {
                state.files.cursor = state.files.items.len().saturating_sub(1)
            }
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

    fn handle_diff_key(&mut self, key: KeyEvent) {
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

    fn handle_review_key(&mut self, key: KeyEvent) {
        let height = self.measured.list_height.max(1) as isize;
        let Screen::Review(state) = self.screens.last_mut().expect("screen") else {
            return;
        };
        let visible = Self::review_visible(state).len();
        let clamp = |c: isize| c.clamp(0, visible.saturating_sub(1) as isize) as usize;
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => self.pop_screen(),
            KeyCode::Char('j') | KeyCode::Down => {
                state.items.cursor = clamp(state.items.cursor as isize + 1)
            }
            KeyCode::Char('k') | KeyCode::Up => {
                state.items.cursor = clamp(state.items.cursor as isize - 1)
            }
            KeyCode::PageDown => state.items.cursor = clamp(state.items.cursor as isize + height),
            KeyCode::PageUp => state.items.cursor = clamp(state.items.cursor as isize - height),
            KeyCode::Char('g') | KeyCode::Home => state.items.cursor = 0,
            KeyCode::Char('G') | KeyCode::End => state.items.cursor = clamp(isize::MAX / 2),
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

    fn handle_notes_key(&mut self, key: KeyEvent) {
        let height = self.measured.list_height.max(1) as isize;
        let Screen::Notes(state) = self.screens.last_mut().expect("screen") else {
            return;
        };
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => self.pop_screen(),
            KeyCode::Char('j') | KeyCode::Down => state.items.move_by(1),
            KeyCode::Char('k') | KeyCode::Up => state.items.move_by(-1),
            KeyCode::PageDown => state.items.move_by(height),
            KeyCode::PageUp => state.items.move_by(-height),
            KeyCode::Char('g') | KeyCode::Home => state.items.cursor = 0,
            KeyCode::Char('G') | KeyCode::End => {
                state.items.cursor = state.items.items.len().saturating_sub(1)
            }
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
                self.agent_state.scroll =
                    (self.agent_state.scroll as isize + delta).max(0) as usize;
                self.agent_state.follow = false;
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
