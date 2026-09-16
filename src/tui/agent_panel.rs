//! The agent panel: the conversation, the agent behind it, and the keys and drawing that go
//! with it. All of it lives here rather than on [`App`](crate::tui::app::App), which has
//! enough to do; the app keeps one of these, hands it keys while its screen is up, and ticks
//! it so that an agent in a repository nobody is looking at still makes progress.

use std::collections::HashMap;
use std::path::Path;
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::Instant;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use serde_json::Value;

use crate::agent::{Agent, Event as AgentEvent, PermissionOption, Role};
use crate::config::AgentConfig;
use crate::theme::Theme;
use crate::tui::style;

/// Who said a line of the transcript.
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

/// What a key press wants the app to do, when it is not something the panel can do itself.
pub enum Action {
    Nothing,
    /// Leave the panel.
    Close,
    /// Open the ask prompt, with this text already in it.
    Ask(String),
    /// Say something on the status line.
    Say(&'static str),
}

/// The conversation and the agent having it. Kept by the app so that it survives leaving
/// the screen, and so that each repository has its own.
#[derive(Default)]
pub struct Panel {
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
    start_began: Option<Instant>,
    /// When the running turn was sent.
    turn_started: Option<Instant>,
    /// When the backend last said anything, to show a long silence for what it is.
    last_event: Option<Instant>,
    /// Tool calls seen in the running turn.
    turn_tools: usize,
    input_tokens: u64,
    output_tokens: u64,
    cost_usd: Option<f64>,
    /// The agent itself, once it has started.
    agent: Option<Agent>,
    /// It is starting on another thread; the result arrives here.
    starting: Option<Receiver<anyhow::Result<Agent>>>,
    /// A prompt sent before the agent finished starting.
    queued: Option<(String, Vec<(String, String)>)>,
    /// What the configured agent is called, found out once: `auto` probes PATH.
    label: Option<String>,
}

impl Panel {
    pub fn new() -> Self {
        Self {
            follow: true,
            show_log: true,
            status: "not started".into(),
            ..Self::default()
        }
    }

    /// The configured agent's name for titles and hints, worked out on first use.
    pub fn label(&mut self, config: &AgentConfig) -> String {
        self.label.get_or_insert_with(|| config.label()).clone()
    }

    /// What the agent calls itself, once it has said.
    pub fn name(&self) -> Option<String> {
        self.agent.as_ref().map(|a| a.name().to_string())
    }

    /// True once there is an agent to talk to.
    pub fn running(&self) -> bool {
        self.agent.is_some()
    }

    pub fn busy(&self) -> bool {
        self.agent.as_ref().is_some_and(Agent::busy)
    }

    /// True while there is activity worth polling quickly for.
    pub fn active(&self) -> bool {
        self.starting.is_some() || self.busy()
    }

    /// Starts the configured agent on another thread; [`Panel::tick`] collects the result.
    pub fn start(&mut self, config: &AgentConfig, root: &Path) {
        if self.agent.is_some() || self.starting.is_some() {
            return;
        }
        let description = format!("starting {}", self.label(config));
        let (config, root) = (config.clone(), root.to_path_buf());
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(crate::agent::spawn(&config, &root));
        });
        self.starting = Some(rx);
        self.status = "starting".into();
        self.start_began = Some(Instant::now());
        self.system(description);
    }

    /// Sends `text` (with embedded `context`) to the agent, starting it first if needed.
    pub fn ask(
        &mut self,
        text: String,
        context: Vec<(String, String)>,
        config: &AgentConfig,
        root: &Path,
    ) {
        self.entries.push(Entry {
            kind: EntryKind::User,
            text: text.clone(),
        });
        self.follow = true;
        match &mut self.agent {
            Some(agent) => {
                if let Err(e) = agent.prompt(&text, &context) {
                    self.system(format!("{e:#}"));
                } else {
                    self.begin_turn();
                }
            }
            None => {
                self.queued = Some((text, context));
                self.start(config, root);
            }
        }
    }

    /// Collects start-up results and events. Returns whether anything changed.
    pub fn tick(&mut self) -> bool {
        let mut changed = false;
        if let Some(rx) = &self.starting {
            match rx.try_recv() {
                Ok(Ok(agent)) => {
                    self.starting = None;
                    let name = agent.name().to_string();
                    self.agent = Some(agent);
                    self.status = "idle".into();
                    self.start_began = None;
                    self.system(format!("connected to {name}"));
                    if let Some((text, context)) = self.queued.take() {
                        let outcome = match &mut self.agent {
                            Some(agent) => agent.prompt(&text, &context),
                            None => Ok(()),
                        };
                        match outcome {
                            Ok(()) => self.begin_turn(),
                            Err(e) => self.system(format!("{e:#}")),
                        }
                    }
                    changed = true;
                }
                Ok(Err(e)) => {
                    self.starting = None;
                    self.queued = None;
                    self.status = "failed to start".into();
                    self.start_began = None;
                    self.system(format!("could not start the agent: {e:#}"));
                    changed = true;
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => {
                    self.starting = None;
                    self.queued = None;
                    self.status = "failed to start".into();
                    self.start_began = None;
                    changed = true;
                }
            }
        }
        let events = match &mut self.agent {
            Some(agent) => agent.poll(),
            None => Vec::new(),
        };
        for event in events {
            self.apply(event);
            changed = true;
        }
        changed
    }

    /// One line of progress for the title and the status bar: the phase, how long the turn
    /// has run, how long the backend has been silent, tool calls so far. `None` when idle.
    pub fn progress(&self) -> Option<String> {
        if let Some(t) = self.start_began {
            return Some(format!("starting · {}s", t.elapsed().as_secs()));
        }
        if !self.busy() && self.permission.is_none() {
            return None;
        }
        let mut parts = Vec::new();
        if self.permission.is_some() {
            parts.push("waiting for permission".to_string());
        } else if !self.phase.is_empty() {
            parts.push(self.phase.clone());
        }
        if let Some(t) = self.turn_started {
            parts.push(format!("{}s", t.elapsed().as_secs()));
        }
        if let Some(t) = self.last_event {
            let quiet = t.elapsed().as_secs();
            if quiet >= 5 && self.permission.is_none() {
                parts.push(format!("silent for {quiet}s"));
            }
        }
        if self.turn_tools > 0 {
            parts.push(format!(
                "{} tool call{}",
                self.turn_tools,
                if self.turn_tools == 1 { "" } else { "s" }
            ));
        }
        Some(parts.join(" · "))
    }

    /// The spinner's current frame, for a title or a strip.
    pub fn spinner(&self) -> &'static str {
        SPINNER[self.spinner_frame()]
    }

    /// Where in its turn the spinner is.
    fn spinner_frame(&self) -> usize {
        (self
            .turn_started
            .or(self.start_began)
            .map(|t| t.elapsed().as_millis() / 80)
            .unwrap_or(0)
            % SPINNER.len() as u128) as usize
    }

    /// `12.3k in / 1.1k out · $0.21` once the backend has reported any usage.
    pub fn usage(&self) -> Option<String> {
        if self.input_tokens == 0 && self.output_tokens == 0 && self.cost_usd.is_none() {
            return None;
        }
        let mut text = format!(
            "{} in / {} out",
            crate::claude::count(self.input_tokens),
            crate::claude::count(self.output_tokens)
        );
        if let Some(cost) = self.cost_usd {
            text.push_str(&format!(" · ${cost:.2}"));
        }
        Some(text)
    }

    fn system(&mut self, text: impl Into<String>) {
        self.entries.push(Entry {
            kind: EntryKind::System,
            text: text.into(),
        });
    }

    /// Bookkeeping for a prompt that was just sent.
    fn begin_turn(&mut self) {
        self.status = "working".into();
        self.phase = "prompt sent".into();
        self.turn_started = Some(Instant::now());
        self.last_event = Some(Instant::now());
        self.turn_tools = 0;
    }

    fn apply(&mut self, event: AgentEvent) {
        self.last_event = Some(Instant::now());
        match event {
            AgentEvent::Text { role, text } => {
                let kind = match role {
                    Role::Agent => EntryKind::Agent,
                    Role::Thought => EntryKind::Thought,
                    Role::User => EntryKind::User,
                };
                match self.entries.last_mut() {
                    Some(last) if last.kind == kind && kind != EntryKind::User => {
                        last.text.push_str(&text)
                    }
                    _ => self.entries.push(Entry { kind, text }),
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
                if !self.tools.contains_key(&id) {
                    self.entries.push(Entry {
                        kind: EntryKind::Tool,
                        text: String::new(),
                    });
                    let i = self.entries.len() - 1;
                    self.tools.insert(id.clone(), (i, ToolInfo::default()));
                    self.turn_tools += 1;
                }
                let (idx, info) = self.tools.get_mut(&id).expect("present");
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
                self.entries[*idx].text = info.render();
            }
            AgentEvent::Plan { entries } => {
                let text = entries
                    .iter()
                    .map(|(c, s)| format!("[{s}] {c}"))
                    .collect::<Vec<_>>()
                    .join("\n");
                self.system(format!("plan:\n{text}"));
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
                self.system(format!("{full}\n({})", names.join("  ")));
                self.permission = Some((request_id, title, options));
                self.status = "waiting for permission".into();
            }
            AgentEvent::TurnDone { stop_reason } => {
                self.status = if stop_reason == "end_turn" {
                    "idle".into()
                } else {
                    format!("stopped: {stop_reason}")
                };
                self.tools.clear();
                self.phase.clear();
                self.turn_started = None;
            }
            AgentEvent::Error { message } => {
                self.status = "error".into();
                self.system(format!("error: {message}"));
            }
            AgentEvent::Stderr { text } => self.entries.push(Entry {
                kind: EntryKind::Log,
                text: format!("stderr: {text}"),
            }),
            AgentEvent::Log { text } => self.entries.push(Entry {
                kind: EntryKind::Log,
                text,
            }),
            AgentEvent::Status { text } => self.phase = text,
            AgentEvent::Usage {
                input_tokens,
                output_tokens,
                cost_usd,
            } => {
                self.input_tokens += input_tokens;
                self.output_tokens += output_tokens;
                if cost_usd.is_some() {
                    self.cost_usd = cost_usd;
                }
            }
            AgentEvent::Exited { message } => {
                self.status = "exited".into();
                self.permission = None;
                self.phase.clear();
                self.turn_started = None;
                self.tools.clear();
                self.system(message);
                self.agent = None;
            }
        }
    }

    fn answer_permission(&mut self, choice: Option<usize>) {
        let Some((id, _, options)) = self.permission.take() else {
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
        self.status = "working".into();
        match outcome {
            Ok(()) => self.system(format!("answered: {label}")),
            Err(e) => self.system(format!("{e:#}")),
        }
    }

    /// The keys of the panel itself. What it cannot do alone comes back as an [`Action`].
    pub fn handle_key(
        &mut self,
        key: KeyEvent,
        height: usize,
        config: &AgentConfig,
        root: &Path,
    ) -> Action {
        let height = height.max(1);
        match key.code {
            KeyCode::Char('q') => return Action::Close,
            KeyCode::Esc => {
                if self.busy() {
                    if let Some(agent) = &mut self.agent {
                        match agent.cancel() {
                            Ok(()) => return Action::Say("cancelling"),
                            Err(e) => self.system(format!("{e:#}")),
                        }
                    }
                } else {
                    return Action::Close;
                }
            }
            KeyCode::Char('i') | KeyCode::Char('a') | KeyCode::Enter => {
                if self.permission.is_some() {
                    return Action::Say("answer the permission request first (1-9, y, n)");
                }
                return Action::Ask(String::new());
            }
            KeyCode::Char('A') => {
                return Action::Ask(crate::agent::prompts::address_all().to_string());
            }
            KeyCode::Char(c @ '1'..='9') if self.permission.is_some() => {
                self.answer_permission(Some(c as usize - '1' as usize));
            }
            KeyCode::Char('y') if self.permission.is_some() => {
                let i = self
                    .permission
                    .as_ref()
                    .and_then(|(_, _, o)| o.iter().position(|x| x.kind.starts_with("allow")));
                self.answer_permission(i);
            }
            KeyCode::Char('n') if self.permission.is_some() => {
                let i = self
                    .permission
                    .as_ref()
                    .and_then(|(_, _, o)| o.iter().position(|x| x.kind.starts_with("reject")));
                self.answer_permission(i);
            }
            KeyCode::Char('j') | KeyCode::Down => {
                self.scroll += 1;
                self.follow = false;
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.scroll = self.scroll.saturating_sub(1);
                self.follow = false;
            }
            KeyCode::PageDown | KeyCode::Char('d')
                if key.code == KeyCode::PageDown
                    || key.modifiers.contains(KeyModifiers::CONTROL) =>
            {
                self.scroll += height / 2;
                self.follow = false;
            }
            KeyCode::PageUp | KeyCode::Char('u')
                if key.code == KeyCode::PageUp || key.modifiers.contains(KeyModifiers::CONTROL) =>
            {
                self.scroll = self.scroll.saturating_sub(height / 2);
                self.follow = false;
            }
            KeyCode::Char('G') | KeyCode::End => self.follow = true,
            KeyCode::Char('g') | KeyCode::Home => {
                self.scroll = 0;
                self.follow = false;
            }
            KeyCode::Char('t') => self.show_thoughts = !self.show_thoughts,
            KeyCode::Char('l') => self.show_log = !self.show_log,
            KeyCode::Char('C') => {
                self.entries.clear();
                self.tools.clear();
                self.scroll = 0;
            }
            KeyCode::Char('R') => {
                self.agent = None;
                self.permission = None;
                self.system("restarting");
                self.start(config, root);
            }
            _ => {}
        }
        Action::Nothing
    }
}

pub(crate) const SPINNER: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// Draws the panel into `area`. Returns the body's height and width, which the app
/// remembers so that a page key moves by the right amount.
pub fn draw(
    frame: &mut Frame,
    panel: &mut Panel,
    theme: &Theme,
    label: &str,
    name: &str,
    area: Rect,
) -> (usize, usize) {
    let mut title: Vec<Span> = vec![
        " agent ".into(),
        name.to_string().bold(),
        format!(" {} ", panel.status).into(),
    ];
    if let Some(progress) = panel.progress() {
        title.push(Span::styled(
            format!("{} {progress} ", SPINNER[panel.spinner_frame()]),
            style::accent(theme),
        ));
    }
    if let Some(usage) = panel.usage() {
        title.push(Span::styled(format!("{usage} "), style::dim(theme)));
    }
    let flags = format!(
        "{}{}",
        if panel.show_thoughts {
            "thoughts shown "
        } else {
            ""
        },
        if panel.show_log { "" } else { "log hidden " }
    );
    if !flags.is_empty() {
        title.push(Span::styled(flags, style::dim(theme)));
    }
    let block = crate::tui::ui::title_block(theme, title.into(), true);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let has_permission = panel.permission.is_some();
    let [body, ask] = Layout::vertical([
        Constraint::Fill(1),
        Constraint::Length(if has_permission { 2 } else { 0 }),
    ])
    .areas(inner);
    let width = body.width.saturating_sub(1) as usize;
    let mut lines: Vec<Line> = Vec::new();
    for entry in &panel.entries {
        if entry.kind == EntryKind::Thought && !panel.show_thoughts {
            continue;
        }
        if entry.kind == EntryKind::Log && !panel.show_log {
            continue;
        }
        let (prefix, style_) = match entry.kind {
            EntryKind::User => ("you  ", style::accent(theme).bold()),
            EntryKind::Agent => ("agent", Style::new()),
            EntryKind::Thought => ("think", style::dim(theme)),
            EntryKind::Tool => ("tool ", style::fg(theme.update_fg)),
            EntryKind::System => ("     ", style::dim(theme)),
            EntryKind::Log => ("log  ", style::dim(theme)),
        };
        let text_style = match entry.kind {
            EntryKind::Thought | EntryKind::System | EntryKind::Log => style::dim(theme),
            EntryKind::Tool => style::fg(theme.update_fg),
            _ => Style::new(),
        };
        for (i, wrapped) in crate::tui::ui::wrap_text(&entry.text, width.saturating_sub(7))
            .into_iter()
            .enumerate()
        {
            let head = if i == 0 {
                format!("{prefix} │ ")
            } else {
                "      │ ".to_string()
            };
            lines.push(
                vec![
                    Span::styled(head, style_),
                    Span::styled(wrapped, text_style),
                ]
                .into(),
            );
        }
        if entry.kind != EntryKind::Log {
            lines.push("".into());
        }
    }
    if panel.entries.is_empty() {
        lines.push(
            "  Nothing yet. Press i to ask something, A to have every pending comment addressed."
                .dim()
                .into(),
        );
        lines.push("".into());
        lines.push(
            format!("  The agent is `{label}`; change [agent] in the config file to use another.")
                .dim()
                .into(),
        );
    }
    let total = lines.len();
    let height = body.height as usize;
    let bottom = total.saturating_sub(height);
    if panel.follow {
        panel.scroll = bottom;
    }
    panel.scroll = panel.scroll.min(bottom);
    // Scrolling back down to the end re-engages following, as `G` does.
    if panel.scroll == bottom {
        panel.follow = true;
    }
    let shown: Vec<Line> = lines.into_iter().skip(panel.scroll).take(height).collect();
    frame.render_widget(Paragraph::new(shown), body);
    if let Some((_, title, options)) = &panel.permission {
        let spans: Vec<Span> = vec![
            Span::styled(" agent asks: ", style::fg(theme.moved).bold()),
            title.clone().into(),
        ];
        let mut choices: Vec<Span> = vec![" ".into()];
        for (i, o) in options.iter().enumerate() {
            choices.push(Span::styled(
                format!("[{}] {}  ", i + 1, o.name),
                style::accent(theme),
            ));
        }
        choices.push("y first allow, n first reject".dim());
        frame.render_widget(
            Paragraph::new(vec![Line::from(spans), Line::from(choices)]),
            ask,
        );
    }
    (body.height as usize, body.width as usize)
}
