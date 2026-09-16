//! The agent panel: the conversation, the agent behind it, and the keys and drawing that go
//! with it. All of it lives here rather than on [`App`](crate::tui::app::App), which has
//! enough to do; the app keeps one of these, hands it keys while its screen is up, and ticks
//! it so that an agent in a repository nobody is looking at still makes progress.

use std::path::Path;
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::Instant;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::agent::Agent;
use crate::config::AgentConfig;
use crate::theme::Theme;
use crate::transcript::{EntryKind, Transcript};
use crate::tui::style;

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
    /// What has been said, and what the turn is costing.
    pub transcript: Transcript,
    pub scroll: usize,
    /// Keep the view at the bottom as text streams in.
    pub follow: bool,
    pub show_thoughts: bool,
    pub show_log: bool,
    /// When the agent started starting; cleared once it is connected.
    start_began: Option<Instant>,
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
            transcript: Transcript::new(),
            ..Self::default()
        }
    }

    /// The question waiting for an answer, if there is one.
    pub fn permission(&self) -> Option<&crate::transcript::Pending> {
        self.transcript.permission.as_ref()
    }

    pub fn status(&self) -> &str {
        &self.transcript.status
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
        self.transcript.status = "starting".into();
        self.start_began = Some(Instant::now());
        self.transcript.system(description);
    }

    /// Sends `text` (with embedded `context`) to the agent, starting it first if needed.
    pub fn ask(
        &mut self,
        text: String,
        context: Vec<(String, String)>,
        config: &AgentConfig,
        root: &Path,
    ) {
        self.transcript.apply(crate::agent::Event::Text {
            role: crate::agent::Role::User,
            text: text.clone(),
        });
        self.follow = true;
        match &mut self.agent {
            Some(agent) => {
                if let Err(e) = agent.prompt(&text, &context) {
                    self.transcript.system(format!("{e:#}"));
                } else {
                    self.transcript.begin_turn();
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
                    self.transcript.status = "idle".into();
                    self.start_began = None;
                    self.transcript.system(format!("connected to {name}"));
                    if let Some((text, context)) = self.queued.take() {
                        let outcome = match &mut self.agent {
                            Some(agent) => agent.prompt(&text, &context),
                            None => Ok(()),
                        };
                        match outcome {
                            Ok(()) => self.transcript.begin_turn(),
                            Err(e) => self.transcript.system(format!("{e:#}")),
                        }
                    }
                    changed = true;
                }
                Ok(Err(e)) => {
                    self.starting = None;
                    self.queued = None;
                    self.transcript.status = "failed to start".into();
                    self.start_began = None;
                    self.transcript
                        .system(format!("could not start the agent: {e:#}"));
                    changed = true;
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => {
                    self.starting = None;
                    self.queued = None;
                    self.transcript.status = "failed to start".into();
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
            self.transcript.apply(event);
            changed = true;
        }
        changed
    }

    /// The spinner's current frame, for a title or a strip.
    /// One line of progress for the title and the status bar.
    pub fn progress(&self) -> Option<String> {
        self.transcript.progress(self.busy(), self.start_began)
    }

    /// `12.3k in / 1.1k out · $0.21` once the backend has reported any usage.
    pub fn usage(&self) -> Option<String> {
        self.transcript.usage()
    }

    pub fn spinner(&self) -> &'static str {
        SPINNER[self.spinner_frame()]
    }

    /// Where in its turn the spinner is.
    fn spinner_frame(&self) -> usize {
        (self
            .transcript
            .turn_started
            .or(self.start_began)
            .map(|t| t.elapsed().as_millis() / 80)
            .unwrap_or(0)
            % SPINNER.len() as u128) as usize
    }

    fn answer_permission(&mut self, choice: Option<usize>) {
        let Some(pending) = self.transcript.permission.take() else {
            return;
        };
        let (id, options) = (pending.id, pending.options);
        let option = choice.and_then(|i| options.get(i));
        let Some(agent) = &mut self.agent else {
            return;
        };
        let outcome = agent.respond_permission(&id, option.map(|o| o.option_id.as_str()));
        let label = option
            .map(|o| o.name.clone())
            .unwrap_or_else(|| "cancelled".into());
        self.transcript.status = "working".into();
        match outcome {
            Ok(()) => self.transcript.system(format!("answered: {label}")),
            Err(e) => self.transcript.system(format!("{e:#}")),
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
                            Err(e) => self.transcript.system(format!("{e:#}")),
                        }
                    }
                } else {
                    return Action::Close;
                }
            }
            KeyCode::Char('i') | KeyCode::Char('a') | KeyCode::Enter => {
                if self.transcript.permission.is_some() {
                    return Action::Say("answer the permission request first (1-9, y, n)");
                }
                return Action::Ask(String::new());
            }
            KeyCode::Char('A') => {
                return Action::Ask(crate::agent::prompts::address_all().to_string());
            }
            KeyCode::Char(c @ '1'..='9') if self.transcript.permission.is_some() => {
                self.answer_permission(Some(c as usize - '1' as usize));
            }
            KeyCode::Char('y') if self.transcript.permission.is_some() => {
                let i = self
                    .transcript
                    .permission
                    .as_ref()
                    .and_then(|p| p.options.iter().position(|x| x.kind.starts_with("allow")));
                self.answer_permission(i);
            }
            KeyCode::Char('n') if self.transcript.permission.is_some() => {
                let i = self
                    .transcript
                    .permission
                    .as_ref()
                    .and_then(|p| p.options.iter().position(|x| x.kind.starts_with("reject")));
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
                self.transcript.clear();
                self.scroll = 0;
            }
            KeyCode::Char('R') => {
                self.agent = None;
                self.transcript.permission = None;
                self.transcript.system("restarting");
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
        format!(" {} ", panel.status()).into(),
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
    let has_permission = panel.permission().is_some();
    let [body, ask] = Layout::vertical([
        Constraint::Fill(1),
        Constraint::Length(if has_permission { 2 } else { 0 }),
    ])
    .areas(inner);
    let width = body.width.saturating_sub(1) as usize;
    let mut lines: Vec<Line> = Vec::new();
    for entry in panel.transcript.entries() {
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
    if panel.transcript.is_empty() {
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
    if let Some(pending) = panel.permission() {
        let spans: Vec<Span> = vec![
            Span::styled(" agent asks: ", style::fg(theme.moved).bold()),
            pending.title.clone().into(),
        ];
        let mut choices: Vec<Span> = vec![" ".into()];
        for (i, o) in pending.options.iter().enumerate() {
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
