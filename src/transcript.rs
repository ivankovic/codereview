//! What an agent's turn looks like once it is written down: the lines of the conversation,
//! and the running account of what the turn is doing and costing.
//!
//! Both front ends show the same thing, so both build it the same way, here. The terminal
//! renders these entries directly; the browser is sent the ones that changed since it last
//! asked, which is what `version` is for.

use std::collections::HashMap;
use std::time::Instant;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::agent::{Event, PermissionOption, Role};

/// Who said a line of the transcript.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryKind {
    User,
    Agent,
    Thought,
    Tool,
    System,
    /// Progress and diagnostics from the backend: what started, stderr, what a turn cost.
    Log,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entry {
    pub kind: EntryKind,
    pub text: String,
    /// Bumped whenever this entry's text changes, so a reader that has seen version `n`
    /// knows which entries to take again. A tool call is written once and then filled in as
    /// it runs, so entries are not append-only.
    pub version: u64,
}

/// How much of a tool's output the transcript keeps. Enough to see what happened, little
/// enough that a command printing a whole file does not become the conversation.
const MAX_TOOL_OUTPUT: usize = 600;

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
            let short: String = out.chars().take(MAX_TOOL_OUTPUT).collect();
            line.push('\n');
            line.push_str(&short);
            if out.len() > short.len() {
                line.push('…');
            }
        }
        line
    }
}

/// A permission question waiting for an answer.
#[derive(Debug, Clone)]
pub struct Pending {
    pub id: Value,
    pub title: String,
    pub details: Option<String>,
    pub options: Vec<PermissionOption>,
}

/// The conversation so far, and what the running turn is doing.
#[derive(Default)]
pub struct Transcript {
    entries: Vec<Entry>,
    /// Tool call id to the entry showing it, for updates that arrive later.
    tools: HashMap<String, (usize, ToolInfo)>,
    version: u64,
    pub status: String,
    /// What the backend says it is doing right now.
    pub phase: String,
    pub permission: Option<Pending>,
    /// When the running turn was sent, and when the backend last said anything.
    pub turn_started: Option<Instant>,
    pub last_event: Option<Instant>,
    pub turn_tools: usize,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost_usd: Option<f64>,
}

impl Transcript {
    pub fn new() -> Self {
        Self {
            status: "not started".into(),
            ..Self::default()
        }
    }

    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The highest version stamped so far: what a reader passes back as `since`.
    pub fn version(&self) -> u64 {
        self.version
    }

    /// Entries written or changed since version `since`, with their positions, so a reader
    /// can put each one where it belongs.
    pub fn since(&self, since: u64) -> Vec<(usize, &Entry)> {
        self.entries
            .iter()
            .enumerate()
            .filter(|(_, e)| e.version > since)
            .collect()
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.tools.clear();
        // Not the version: a reader that has seen more must be told to start again, and a
        // version going backwards would leave it showing what is no longer there.
        self.next_version();
    }

    /// Adds a line nobody said: what codereview itself is doing.
    pub fn system(&mut self, text: impl Into<String>) {
        self.push(EntryKind::System, text.into());
    }

    /// The next stamp, for an entry about to be written or changed.
    fn next_version(&mut self) -> u64 {
        self.version += 1;
        self.version
    }

    fn push(&mut self, kind: EntryKind, text: String) {
        let version = self.next_version();
        self.entries.push(Entry {
            kind,
            text,
            version,
        });
    }

    /// Bookkeeping for a prompt just sent.
    pub fn begin_turn(&mut self) {
        self.status = "working".into();
        self.phase = "prompt sent".into();
        self.turn_started = Some(Instant::now());
        self.last_event = Some(Instant::now());
        self.turn_tools = 0;
    }

    /// Writes down what the agent said or did.
    pub fn apply(&mut self, event: Event) {
        self.last_event = Some(Instant::now());
        match event {
            Event::Text { role, text } => {
                let kind = match role {
                    Role::Agent => EntryKind::Agent,
                    Role::Thought => EntryKind::Thought,
                    Role::User => EntryKind::User,
                };
                // One speaker's text arrives in pieces and reads as one line.
                let joins = self
                    .entries
                    .last()
                    .is_some_and(|last| last.kind == kind && kind != EntryKind::User);
                if joins {
                    let version = self.next_version();
                    if let Some(last) = self.entries.last_mut() {
                        last.text.push_str(&text);
                        last.version = version;
                    }
                } else {
                    self.push(kind, text);
                }
            }
            Event::ToolCall {
                id,
                title,
                kind,
                status,
                locations,
                output,
            } => self.merge_tool_call(id, title, kind, status, locations, output),
            Event::Plan { entries } => {
                let text = entries
                    .iter()
                    .map(|(c, s)| format!("[{s}] {c}"))
                    .collect::<Vec<_>>()
                    .join("\n");
                self.system(format!("plan:\n{text}"));
            }
            Event::Permission {
                request_id,
                title,
                details,
                options,
            } => self.ask_permission(request_id, title, details, options),
            Event::TurnDone { stop_reason } => {
                self.status = if stop_reason == "end_turn" {
                    "idle".into()
                } else {
                    format!("stopped: {stop_reason}")
                };
                self.tools.clear();
                self.phase.clear();
                self.turn_started = None;
            }
            Event::Error { message } => {
                self.status = "error".into();
                self.system(format!("error: {message}"));
            }
            Event::Stderr { text } => self.push(EntryKind::Log, format!("stderr: {text}")),
            Event::Log { text } => self.push(EntryKind::Log, text),
            Event::Status { text } => self.phase = text,
            Event::Usage {
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
            Event::Exited { message } => {
                self.status = "exited".into();
                self.permission = None;
                self.phase.clear();
                self.turn_started = None;
                self.tools.clear();
                self.system(message);
            }
        }
    }

    /// A tool call is one entry, written when it starts and changed as it runs. Each update
    /// mentions only what it knows, so the rest of what was said before is kept.
    fn merge_tool_call(
        &mut self,
        id: String,
        title: Option<String>,
        kind: Option<String>,
        status: Option<String>,
        locations: Vec<String>,
        output: Option<String>,
    ) {
        if !self.tools.contains_key(&id) {
            self.push(EntryKind::Tool, String::new());
            let i = self.entries.len() - 1;
            self.tools.insert(id.clone(), (i, ToolInfo::default()));
            self.turn_tools += 1;
        }
        let version = self.next_version();
        let (at, info) = self.tools.get_mut(&id).expect("just inserted");
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
        let (text, at) = (info.render(), *at);
        if let Some(entry) = self.entries.get_mut(at) {
            entry.text = text;
            entry.version = version;
        }
    }

    /// A question the agent is waiting on. The whole of what would be approved goes in the
    /// transcript: a status bar has room for one line, and a command's second line is where
    /// something unwanted would hide.
    fn ask_permission(
        &mut self,
        request_id: Value,
        title: String,
        details: Option<String>,
        options: Vec<PermissionOption>,
    ) {
        let names: Vec<String> = options
            .iter()
            .enumerate()
            .map(|(i, o)| format!("{} {}", i + 1, o.name))
            .collect();
        let full = match &details {
            Some(d) if d.trim() != title.trim() => format!("permission: {title}\n{d}"),
            _ => format!("permission: {title}"),
        };
        self.system(format!("{full}\n({})", names.join("  ")));
        self.permission = Some(Pending {
            id: request_id,
            title,
            details,
            options,
        });
        self.status = "waiting for permission".into();
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

    /// One line of progress: the phase, how long the turn has run, how long the backend has
    /// been silent, and how many tools it has called. `starting` says a start is under way.
    pub fn progress(&self, busy: bool, starting: Option<Instant>) -> Option<String> {
        if let Some(t) = starting {
            return Some(format!("starting · {}s", t.elapsed().as_secs()));
        }
        if !busy && self.permission.is_none() {
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

    /// Roughly how much the entries hold, for a caller that has to bound it.
    pub fn bytes(&self) -> usize {
        self.entries.iter().map(|e| e.text.len()).sum()
    }

    /// Drops the oldest entries until the transcript holds less than `max` bytes. The tool
    /// call positions go with them, so updates to a dropped call start a new entry rather
    /// than landing on the wrong one. Everything left is stamped anew, because dropping
    /// from the front moves every position: a reader holding entries by position has to
    /// take them all again.
    pub fn trim_to(&mut self, max: usize) {
        let mut bytes = self.bytes();
        if bytes <= max {
            return;
        }
        let mut drop = 0;
        while bytes > max && drop + 1 < self.entries.len() {
            bytes -= self.entries[drop].text.len();
            drop += 1;
        }
        self.entries.drain(..drop);
        self.tools.clear();
        let version = self.next_version();
        for entry in &mut self.entries {
            entry.version = version;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(role: Role, text: &str) -> Event {
        Event::Text {
            role,
            text: text.into(),
        }
    }

    #[test]
    fn text_from_one_speaker_reads_as_one_line() {
        let mut t = Transcript::new();
        t.apply(text(Role::Agent, "Hello "));
        t.apply(text(Role::Agent, "world"));
        assert_eq!(t.entries().len(), 1);
        assert_eq!(t.entries()[0].text, "Hello world");
        // A different speaker starts a new line, and so does every prompt.
        t.apply(text(Role::Thought, "hmm"));
        t.apply(text(Role::User, "a"));
        t.apply(text(Role::User, "b"));
        assert_eq!(t.entries().len(), 4);
        let kinds: Vec<EntryKind> = t.entries().iter().map(|e| e.kind).collect();
        assert_eq!(
            kinds,
            vec![
                EntryKind::Agent,
                EntryKind::Thought,
                EntryKind::User,
                EntryKind::User
            ]
        );
    }

    #[test]
    fn a_tool_call_is_written_once_and_filled_in() {
        let mut t = Transcript::new();
        t.apply(Event::ToolCall {
            id: "1".into(),
            title: Some("Bash: ls".into()),
            kind: Some("execute".into()),
            status: Some("in_progress".into()),
            locations: vec![],
            output: None,
        });
        let first = t.version();
        assert_eq!(t.entries().len(), 1);
        assert_eq!(t.entries()[0].text, "Bash: ls (execute) [in_progress]");
        assert_eq!(t.turn_tools, 1);
        t.apply(Event::ToolCall {
            id: "1".into(),
            title: None,
            kind: None,
            status: Some("completed".into()),
            locations: vec![],
            output: Some("a\nb".into()),
        });
        assert_eq!(t.entries().len(), 1, "the same call, not a second one");
        assert!(t.entries()[0].text.contains("[completed]"));
        assert!(t.entries()[0].text.ends_with("a\nb"));
        // The change is visible to a reader that had already seen the first version.
        let changed = t.since(first);
        assert_eq!(changed.len(), 1);
        assert_eq!(changed[0].0, 0, "at the same position");
    }

    #[test]
    fn a_reader_is_told_only_what_changed() {
        let mut t = Transcript::new();
        t.apply(text(Role::Agent, "one"));
        let seen = t.version();
        assert!(t.since(seen).is_empty());
        t.system("two");
        let new = t.since(seen);
        assert_eq!(new.len(), 1);
        assert_eq!(new[0].1.text, "two");
        assert_eq!(new[0].0, 1);
    }

    #[test]
    fn usage_and_progress_add_up() {
        let mut t = Transcript::new();
        assert!(t.usage().is_none());
        assert!(t.progress(false, None).is_none());
        t.apply(Event::Usage {
            input_tokens: 1000,
            output_tokens: 20,
            cost_usd: None,
        });
        t.apply(Event::Usage {
            input_tokens: 500,
            output_tokens: 5,
            cost_usd: Some(0.5),
        });
        assert_eq!(t.usage().as_deref(), Some("1.5k in / 25 out · $0.50"));
        t.begin_turn();
        t.apply(Event::Status {
            text: "thinking".into(),
        });
        let progress = t.progress(true, None).unwrap();
        assert!(progress.starts_with("thinking · "), "{progress}");
        t.apply(Event::TurnDone {
            stop_reason: "end_turn".into(),
        });
        assert_eq!(t.status, "idle");
        assert!(t.progress(false, None).is_none());
    }

    #[test]
    fn trimming_keeps_the_newest_and_tells_readers_to_take_it_all_again() {
        let mut t = Transcript::new();
        for i in 0..10 {
            t.system(format!("{i}: {}", "x".repeat(100)));
        }
        let seen = t.version();
        t.trim_to(300);
        assert!(t.bytes() <= 400, "{}", t.bytes());
        assert!(t.entries().len() < 10);
        assert!(
            t.entries().last().unwrap().text.starts_with("9: "),
            "the newest is kept"
        );
        assert_eq!(
            t.since(seen).len(),
            t.entries().len(),
            "everything is offered again, since the positions moved"
        );
    }
}
