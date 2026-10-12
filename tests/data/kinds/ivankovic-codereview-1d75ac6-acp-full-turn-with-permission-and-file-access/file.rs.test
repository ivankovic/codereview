//! A client for the Agent Client Protocol (ACP): codereview drives an agent process the way
//! an editor would. The agent runs as a child speaking JSON-RPC 2.0, one message per line,
//! over its stdin and stdout. This module spawns it, performs the `initialize` and
//! `session/new` handshake, sends prompts, streams the agent's updates back as [`Event`]s,
//! answers its file reads and writes (inside the repository only), and relays its permission
//! requests to whoever is driving the UI.
//!
//! Nothing here is async: a reader thread turns stdout lines into channel messages, and the
//! front ends call [`Agent::poll`] from their own loops. Protocol version 1.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc::TryRecvError;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::transport::{Raw, Transport};

pub const PROTOCOL_VERSION: u64 = 1;

/// How long the handshake may take: `npx` fetching an adapter on first use is slow.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(120);

/// Who said a piece of text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    User,
    Agent,
    Thought,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PermissionOption {
    pub option_id: String,
    pub name: String,
    /// `allow_once`, `allow_always`, `reject_once`, `reject_always`.
    pub kind: String,
}

/// What the agent sent, in a form the front ends can show without knowing the protocol.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event {
    /// A piece of streamed text.
    Text { role: Role, text: String },
    /// A tool call started or changed. Fields the update did not mention are `None`.
    ToolCall {
        id: String,
        title: Option<String>,
        kind: Option<String>,
        status: Option<String>,
        /// `path:line` for each location the call touches.
        locations: Vec<String>,
        /// Text the tool produced, if any.
        output: Option<String>,
    },
    /// The agent's plan: `(content, status)` per entry.
    Plan { entries: Vec<(String, String)> },
    /// The agent wants to do something and asks; answer with [`Agent::respond_permission`].
    Permission {
        request_id: Value,
        /// One line, for a status bar.
        title: String,
        /// Everything the answer would approve, in full. A title is shortened to fit, and
        /// approving what you cannot see is how a second command rides along with the one
        /// you meant to allow.
        details: Option<String>,
        options: Vec<PermissionOption>,
    },
    /// The prompt turn ended: `end_turn`, `max_tokens`, `refusal`, `cancelled`, ...
    TurnDone { stop_reason: String },
    /// The agent answered a prompt with a JSON-RPC error.
    Error { message: String },
    /// A line the agent wrote to stderr; diagnostics, not conversation.
    Stderr { text: String },
    /// Progress worth showing but not part of the conversation: what was started, what the
    /// session is, what a turn cost.
    Log { text: String },
    /// What the agent is doing right now, in a few words (`waiting for the model`).
    Status { text: String },
    /// Tokens the last model call used (to add up) and, when known, the session's total cost
    /// so far (to replace).
    Usage {
        input_tokens: u64,
        output_tokens: u64,
        cost_usd: Option<f64>,
    },
    /// The agent process ended.
    Exited { message: String },
}

/// A running agent process with one session.
pub struct Agent {
    io: Transport,
    next_id: u64,
    session_id: String,
    root: PathBuf,
    /// The id of the `session/prompt` request whose turn is running.
    pending_prompt: Option<u64>,
    /// Responses to requests other than the running prompt, keyed by id, waiting for
    /// `wait_response`.
    responses: HashMap<u64, Value>,
    /// Events that arrived while `wait_response` was blocking; drained by the next `poll`.
    queued: Vec<Event>,
    pub name: String,
    pub embedded_context: bool,
    exited: bool,
}

impl Agent {
    /// Starts `command args...` in `root`, initialises it and opens a session there.
    pub fn spawn(command: &str, args: &[String], root: &Path) -> Result<Self> {
        preflight(command, args)?;
        let mut process = Command::new(command);
        process.args(args).current_dir(root);
        crate::agent::strip_secrets(&mut process);
        let io = Transport::spawn(process, format!("{command} {}", args.join(" ")))?;
        Self::start(io, command, root)
    }

    /// Initialises an agent over `io` and opens a session in `root`; `name` is what to call
    /// it until it introduces itself.
    pub(crate) fn start(io: Transport, name: &str, root: &Path) -> Result<Self> {
        let mut agent = Self {
            io,
            next_id: 0,
            session_id: String::new(),
            root: root.to_path_buf(),
            pending_prompt: None,
            responses: HashMap::new(),
            queued: Vec::new(),
            name: name.to_string(),
            embedded_context: false,
            exited: false,
        };
        agent.queued.push(Event::Log {
            text: format!("started {} in {}", agent.io.description(), root.display()),
        });
        let started = Instant::now();
        if let Err(e) = agent.handshake() {
            agent.io.kill();
            // Whatever the agent said on stderr is the real explanation more often than not.
            let tail = agent.io.last_words(&agent.queued);
            if tail.is_empty() {
                return Err(e);
            }
            return Err(anyhow!("{e}\n{}", tail.join("\n")));
        }
        agent.queued.push(Event::Log {
            text: format!(
                "session {} open with {} after {:.1}s{}",
                agent.session_id,
                agent.name,
                started.elapsed().as_secs_f64(),
                if agent.embedded_context {
                    ", embedded context accepted"
                } else {
                    ""
                }
            ),
        });
        Ok(agent)
    }

    fn handshake(&mut self) -> Result<()> {
        let init = self.request(
            "initialize",
            json!({
                "protocolVersion": PROTOCOL_VERSION,
                "clientCapabilities": {
                    "fs": { "readTextFile": true, "writeTextFile": true },
                    "terminal": false
                },
                "clientInfo": { "name": "codereview", "version": env!("CARGO_PKG_VERSION") }
            }),
        )?;
        let init = self.wait_response(init, HANDSHAKE_TIMEOUT)?;
        if let Some(info) = init
            .get("agentInfo")
            .and_then(|i| i.get("name"))
            .and_then(Value::as_str)
        {
            self.name = info.to_string();
        }
        self.embedded_context = init
            .pointer("/agentCapabilities/promptCapabilities/embeddedContext")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let auth_methods: Vec<String> = init
            .get("authMethods")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|m| m.get("id")?.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        let new_session = json!({ "cwd": self.root, "mcpServers": [] });
        let id = self.request("session/new", new_session.clone())?;
        let session = match self.wait_response(id, HANDSHAKE_TIMEOUT) {
            Ok(v) => v,
            Err(e) if !auth_methods.is_empty() && e.to_string().to_lowercase().contains("auth") => {
                // Try the agent's first authentication method once, then open the session.
                let id = self.request("authenticate", json!({ "methodId": auth_methods[0] }))?;
                self.wait_response(id, HANDSHAKE_TIMEOUT)
                    .context("authenticate")?;
                let id = self.request("session/new", new_session)?;
                self.wait_response(id, HANDSHAKE_TIMEOUT)?
            }
            Err(e) => return Err(e),
        };
        self.session_id = session
            .get("sessionId")
            .and_then(Value::as_str)
            .context("session/new returned no sessionId")?
            .to_string();
        Ok(())
    }

    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// True while a prompt turn is running.
    pub fn busy(&self) -> bool {
        self.pending_prompt.is_some()
    }

    pub fn is_alive(&self) -> bool {
        !self.exited
    }

    fn send(&mut self, message: &Value) -> Result<()> {
        self.io.send_json(message)
    }

    fn request(&mut self, method: &str, params: Value) -> Result<u64> {
        self.next_id += 1;
        let id = self.next_id;
        self.send(&json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }))?;
        Ok(id)
    }

    fn notify(&mut self, method: &str, params: Value) -> Result<()> {
        self.send(&json!({ "jsonrpc": "2.0", "method": method, "params": params }))
    }

    /// Blocks until the response to `id` arrives, handling everything else meanwhile.
    fn wait_response(&mut self, id: u64, timeout: Duration) -> Result<Value> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(v) = self.responses.remove(&id) {
                if let Some(err) = v.get("__error").and_then(Value::as_str) {
                    bail!("{err}");
                }
                return Ok(v);
            }
            if self.exited {
                bail!("agent exited during the handshake");
            }
            match self.io.recv_until(deadline) {
                Some(raw) => {
                    if let Some(event) = self.handle_raw(raw) {
                        self.queued.push(event);
                    }
                }
                None => bail!("agent did not answer within {}s", timeout.as_secs()),
            }
        }
    }

    /// Sends a prompt: `text`, plus `context` as embedded resources (`(uri, text)`) when the
    /// agent accepts them, otherwise folded into the text.
    pub fn prompt(&mut self, text: &str, context: &[(String, String)]) -> Result<()> {
        if self.pending_prompt.is_some() {
            bail!("the agent is still working on the previous prompt");
        }
        let mut blocks = Vec::new();
        if self.embedded_context {
            blocks.push(json!({ "type": "text", "text": text }));
            for (uri, body) in context {
                blocks.push(json!({
                    "type": "resource",
                    "resource": { "uri": uri, "text": body, "mimeType": "text/plain" }
                }));
            }
        } else {
            let mut full = text.to_string();
            for (uri, body) in context {
                full.push_str(&format!("\n\n{uri}:\n```\n{body}\n```"));
            }
            blocks.push(json!({ "type": "text", "text": full }));
        }
        let id = self.request(
            "session/prompt",
            json!({ "sessionId": self.session_id, "prompt": blocks }),
        )?;
        self.pending_prompt = Some(id);
        Ok(())
    }

    /// Asks the agent to stop the running turn; it answers the prompt with `cancelled`.
    pub fn cancel(&mut self) -> Result<()> {
        let params = json!({ "sessionId": self.session_id });
        self.notify("session/cancel", params)
    }

    pub fn respond_permission(
        &mut self,
        request_id: &Value,
        option_id: Option<&str>,
    ) -> Result<()> {
        let outcome = match option_id {
            Some(id) => json!({ "outcome": "selected", "optionId": id }),
            None => json!({ "outcome": "cancelled" }),
        };
        self.send(&json!({ "jsonrpc": "2.0", "id": request_id, "result": { "outcome": outcome } }))
    }

    /// Everything that arrived since the last call. Never blocks.
    pub fn poll(&mut self) -> Vec<Event> {
        let mut events = std::mem::take(&mut self.queued);
        loop {
            match self.io.rx.try_recv() {
                Ok(raw) => {
                    if let Some(e) = self.handle_raw(raw) {
                        events.push(e);
                    }
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    if !self.exited {
                        self.exited = true;
                        self.pending_prompt = None;
                        events.push(Event::Exited {
                            message: "agent closed its output".into(),
                        });
                    }
                    break;
                }
            }
        }
        events
    }

    fn handle_raw(&mut self, raw: Raw) -> Option<Event> {
        match raw {
            Raw::Stderr(text) => Some(Event::Stderr { text }),
            Raw::Eof => {
                self.exited = true;
                let message = self.io.exit_message("agent");
                self.pending_prompt = None;
                Some(Event::Exited { message })
            }
            Raw::Message(msg) => self.handle_message(msg),
        }
    }

    fn handle_message(&mut self, msg: Value) -> Option<Event> {
        let method = msg
            .get("method")
            .and_then(Value::as_str)
            .map(str::to_string);
        let id = msg.get("id").cloned().filter(|v| !v.is_null());
        match (method, id) {
            (Some(method), Some(id)) => self.handle_request(
                &method,
                id,
                msg.get("params").cloned().unwrap_or(Value::Null),
            ),
            (Some(method), None) => {
                self.handle_notification(&method, msg.get("params").cloned().unwrap_or(Value::Null))
            }
            (None, Some(id)) => {
                let id = id.as_u64()?;
                if self.pending_prompt == Some(id) {
                    self.pending_prompt = None;
                    if let Some(err) = msg.get("error") {
                        return Some(Event::Error {
                            message: err
                                .get("message")
                                .and_then(Value::as_str)
                                .unwrap_or("prompt failed")
                                .to_string(),
                        });
                    }
                    let stop_reason = msg
                        .pointer("/result/stopReason")
                        .and_then(Value::as_str)
                        .unwrap_or("end_turn")
                        .to_string();
                    return Some(Event::TurnDone { stop_reason });
                }
                let value = match msg.get("error") {
                    Some(err) => {
                        let text = err
                            .get("message")
                            .and_then(Value::as_str)
                            .unwrap_or("error");
                        // Handed to wait_response as an error through a marker; simplest is
                        // to store the error and let the waiter see it.
                        self.responses.insert(id, json!({ "__error": text }));
                        return None;
                    }
                    None => msg.get("result").cloned().unwrap_or(Value::Null),
                };
                self.responses.insert(id, value);
                None
            }
            (None, None) => None,
        }
    }

    fn handle_notification(&mut self, method: &str, params: Value) -> Option<Event> {
        if method != "session/update" {
            return None;
        }
        let update = params.get("update")?;
        let kind = update.get("sessionUpdate").and_then(Value::as_str)?;
        match kind {
            "agent_message_chunk" | "agent_thought_chunk" | "user_message_chunk" => {
                let role = match kind {
                    "agent_message_chunk" => Role::Agent,
                    "agent_thought_chunk" => Role::Thought,
                    _ => Role::User,
                };
                let text = content_text(update.get("content")?);
                Some(Event::Text { role, text })
            }
            "tool_call" | "tool_call_update" => Some(tool_call_event(update)),
            "plan" => {
                let entries = update
                    .get("entries")
                    .and_then(Value::as_array)
                    .map(|a| {
                        a.iter()
                            .map(|e| {
                                (
                                    e.get("content")
                                        .and_then(Value::as_str)
                                        .unwrap_or("")
                                        .to_string(),
                                    e.get("status")
                                        .and_then(Value::as_str)
                                        .unwrap_or("pending")
                                        .to_string(),
                                )
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                Some(Event::Plan { entries })
            }
            "available_commands_update" | "current_mode_update" => None,
            other => Some(Event::Log {
                text: format!("session update {other} (not shown)"),
            }),
        }
    }

    fn handle_request(&mut self, method: &str, id: Value, params: Value) -> Option<Event> {
        match method {
            "session/request_permission" => {
                let title = params
                    .pointer("/toolCall/title")
                    .and_then(Value::as_str)
                    .unwrap_or("a tool call")
                    .to_string();
                let options = params
                    .get("options")
                    .and_then(Value::as_array)
                    .map(|a| {
                        a.iter()
                            .filter_map(|o| {
                                Some(PermissionOption {
                                    option_id: o.get("optionId")?.as_str()?.to_string(),
                                    name: o
                                        .get("name")
                                        .and_then(Value::as_str)
                                        .unwrap_or("")
                                        .to_string(),
                                    kind: o
                                        .get("kind")
                                        .and_then(Value::as_str)
                                        .unwrap_or("")
                                        .to_string(),
                                })
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                Some(Event::Permission {
                    request_id: id,
                    title,
                    details: None,
                    options,
                })
            }
            "fs/read_text_file" => {
                let result = self.read_text_file(&params);
                self.reply(id, result);
                None
            }
            "fs/write_text_file" => {
                let result = self.write_text_file(&params);
                self.reply(id, result);
                None
            }
            _ => {
                self.reply(id, Err(anyhow!("method not found: {method}")));
                None
            }
        }
    }

    fn reply(&mut self, id: Value, result: Result<Value>) {
        let msg = match result {
            Ok(v) => json!({ "jsonrpc": "2.0", "id": id, "result": v }),
            Err(e) => {
                json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32000, "message": e.to_string() } })
            }
        };
        let _ = self.send(&msg);
    }

    fn resolve(&self, path: &str) -> Result<PathBuf> {
        resolve_in(&self.root, path)
    }

    fn read_text_file(&self, params: &Value) -> Result<Value> {
        let path = params
            .get("path")
            .and_then(Value::as_str)
            .context("path required")?;
        let full = self.resolve(path)?;
        let text = std::fs::read_to_string(&full).with_context(|| format!("cannot read {path}"))?;
        let line = params
            .get("line")
            .and_then(Value::as_u64)
            .map(|l| l.saturating_sub(1) as usize);
        let limit = params
            .get("limit")
            .and_then(Value::as_u64)
            .map(|l| l as usize);
        let content = match (line, limit) {
            (None, None) => text,
            (line, limit) => text
                .lines()
                .skip(line.unwrap_or(0))
                .take(limit.unwrap_or(usize::MAX))
                .collect::<Vec<_>>()
                .join("\n"),
        };
        Ok(json!({ "content": content }))
    }

    fn write_text_file(&self, params: &Value) -> Result<Value> {
        let path = params
            .get("path")
            .and_then(Value::as_str)
            .context("path required")?;
        let content = params
            .get("content")
            .and_then(Value::as_str)
            .context("content required")?;
        let full = self.resolve(path)?;
        if let Some(dir) = full.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(&full, content).with_context(|| format!("cannot write {path}"))?;
        Ok(json!({}))
    }
}

/// The text of a content block; non-text blocks become a short marker.
fn content_text(block: &Value) -> String {
    match block.get("type").and_then(Value::as_str) {
        Some("text") => block
            .get("text")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        Some("image") => "[image]".into(),
        Some("audio") => "[audio]".into(),
        Some("resource_link") => format!(
            "[{}]",
            block
                .get("uri")
                .and_then(Value::as_str)
                .unwrap_or("resource")
        ),
        Some("resource") => block
            .pointer("/resource/text")
            .and_then(Value::as_str)
            .unwrap_or("[resource]")
            .to_string(),
        _ => String::new(),
    }
}

fn tool_call_event(update: &Value) -> Event {
    let id = update
        .get("toolCallId")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let field = |k: &str| update.get(k).and_then(Value::as_str).map(str::to_string);
    let locations = update
        .get("locations")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|l| {
                    let path = l.get("path")?.as_str()?;
                    Some(match l.get("line").and_then(Value::as_u64) {
                        Some(line) => format!("{path}:{line}"),
                        None => path.to_string(),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    let output = update
        .get("content")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|c| match c.get("type").and_then(Value::as_str) {
                    Some("content") => c.get("content").map(content_text),
                    Some("diff") => Some(format!(
                        "diff {}",
                        c.get("path").and_then(Value::as_str).unwrap_or("")
                    )),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n")
        });
    Event::ToolCall {
        id,
        title: field("title"),
        kind: field("kind"),
        status: field("status"),
        locations,
        output: output.filter(|o| !o.is_empty()),
    }
}

/// The oldest Node the npm-distributed adapters (Claude Code's, Gemini's) run on.
const MIN_NODE_MAJOR: u32 = 20;

/// Catches the one failure that otherwise shows up as a stack trace from the adapter: an
/// `npx`/`node`-based agent on a Node too old to load it.
fn preflight(command: &str, args: &[String]) -> Result<()> {
    let base = Path::new(command)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(command);
    if !matches!(base, "npx" | "node" | "npm") {
        return Ok(());
    }
    let Ok(output) = Command::new("node").arg("--version").output() else {
        bail!(
            "the agent `{command} {}` needs Node, and `node` is not on PATH; install Node {MIN_NODE_MAJOR} or newer, \
             or set [agent] in the config file to another ACP agent",
            args.join(" ")
        );
    };
    let version = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let major: u32 = version
        .trim_start_matches('v')
        .split('.')
        .next()
        .and_then(|m| m.parse().ok())
        .unwrap_or(0);
    if major < MIN_NODE_MAJOR {
        let path = Command::new("which")
            .arg("node")
            .output()
            .ok()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .filter(|p| !p.is_empty())
            .unwrap_or_else(|| "node".into());
        bail!(
            "the agent `{command} {}` needs Node {MIN_NODE_MAJOR} or newer, but `{path}` is {version}. \
             Install a newer Node (for example `nvm install 22`, or your distribution's nodejs 22 \
             package) and make sure it is first on PATH, or point [agent] in the config file at \
             another ACP agent",
            args.join(" ")
        );
    }
    Ok(())
}

/// The absolute path for an agent-supplied one, refused when it leaves `root`. Symlinks are
/// followed on the nearest existing ancestor, so a new file's directory is checked too.
fn resolve_in(root: &Path, path: &str) -> Result<PathBuf> {
    let raw = if Path::new(path).is_absolute() {
        PathBuf::from(path)
    } else {
        root.join(path)
    };
    // Lexical normalisation first: `.` vanishes, `..` pops, so the probe below never has to
    // reason about parent components.
    let mut full = PathBuf::new();
    for component in raw.components() {
        use std::path::Component;
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !full.pop() {
                    bail!("{path} is outside the repository");
                }
            }
            other => full.push(other.as_os_str()),
        }
    }
    let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let mut probe = full.clone();
    let mut tail: Vec<std::ffi::OsString> = Vec::new();
    while !probe.exists() {
        let Some(name) = probe.file_name() else {
            bail!("{path} is outside the repository");
        };
        tail.push(name.to_os_string());
        probe = probe.parent().map(Path::to_path_buf).unwrap_or_default();
    }
    let mut canonical = probe.canonicalize()?;
    for name in tail.into_iter().rev() {
        canonical.push(name);
    }
    if !canonical.starts_with(&root) {
        bail!("{path} is outside the repository");
    }
    // A link that points nowhere yet looks like a file that does not exist, and writing
    // through it would create the file wherever it points, outside the repository.
    if std::fs::symlink_metadata(&canonical).is_ok_and(|m| m.file_type().is_symlink()) {
        bail!("{path} is a symlink; codereview will not follow it");
    }
    // The repository's own machinery is not a file to edit: a hook or a config written
    // there runs the next time anybody uses git here.
    if canonical
        .strip_prefix(&root)
        .is_ok_and(|rest| rest.components().any(|c| c.as_os_str() == ".git"))
    {
        bail!("{path} is inside .git");
    }
    Ok(canonical)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Polls until `done` says so or `timeout` passes, collecting events.
    fn drain(
        agent: &mut Agent,
        timeout: Duration,
        mut done: impl FnMut(&[Event]) -> bool,
    ) -> Vec<Event> {
        let deadline = Instant::now() + timeout;
        let mut events = Vec::new();
        while Instant::now() < deadline {
            events.extend(agent.poll());
            if done(&events) {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        events
    }

    #[test]
    fn full_turn_with_permission_and_file_access() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.rs"), "fn a() {}\n").unwrap();
        let mut agent = Agent::start(crate::fakes::acp(), "fake", dir.path()).unwrap();
        assert_eq!(agent.name, "fake-agent");
        assert!(agent.embedded_context);
        assert_eq!(agent.session_id(), "sess-1");

        agent
            .prompt("hi", &[("file:///x".into(), "ctx".into())])
            .unwrap();
        assert!(agent.busy());
        let events = drain(&mut agent, Duration::from_secs(10), |e| {
            e.iter().any(|e| matches!(e, Event::Permission { .. }))
        });
        let permission = events
            .iter()
            .find_map(|e| match e {
                Event::Permission {
                    request_id,
                    title,
                    options,
                    ..
                } => Some((request_id.clone(), title.clone(), options.clone())),
                _ => None,
            })
            .expect("permission request");
        assert_eq!(permission.1, "Read a.rs");
        assert_eq!(permission.2[0].kind, "allow_once");
        assert!(
            events.iter().any(
                |e| matches!(e, Event::Text { role: Role::Thought, text } if text == "thinking")
            )
        );
        assert!(events.iter().any(|e| matches!(e, Event::ToolCall { id, status: Some(s), .. } if id == "call-1" && s == "pending")));

        agent
            .respond_permission(&permission.0, Some("allow"))
            .unwrap();
        let events = drain(&mut agent, Duration::from_secs(10), |e| {
            e.iter().any(|e| matches!(e, Event::TurnDone { .. }))
        });
        let text: String = events
            .iter()
            .filter_map(|e| match e {
                Event::Text {
                    role: Role::Agent,
                    text,
                } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(text, "; read 10 bytes; outside refused");
        assert!(events.iter().any(|e| matches!(e, Event::ToolCall { status: Some(s), output: Some(o), .. } if s == "completed" && o == "fn a() {}\n")));
        assert!(events.iter().any(|e| matches!(e, Event::Plan { entries } if entries[0] == ("step one".to_string(), "completed".to_string()))));
        assert!(
            matches!(events.last(), Some(Event::TurnDone { stop_reason }) if stop_reason == "end_turn")
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("written.txt")).unwrap(),
            "by agent\n"
        );
        assert!(!agent.busy());

        // A second turn, cancelled.
        agent.prompt("please cancel me", &[]).unwrap();
        let events = drain(&mut agent, Duration::from_secs(10), |e| !e.is_empty());
        assert!(
            events
                .iter()
                .any(|e| matches!(e, Event::Text { text, .. } if text == "working..."))
        );
        agent.cancel().unwrap();
        let events = drain(&mut agent, Duration::from_secs(10), |e| {
            e.iter().any(|e| matches!(e, Event::TurnDone { .. }))
        });
        assert!(
            matches!(events.last(), Some(Event::TurnDone { stop_reason }) if stop_reason == "cancelled")
        );
    }

    /// An error reply during the handshake is reported as the agent's own message.
    #[test]
    fn handshake_error_is_reported() {
        let io = Transport::in_process(|rx, tx| {
            for line in rx {
                let Ok(msg) = serde_json::from_str::<Value>(&line) else {
                    continue;
                };
                let _ = tx.send(Raw::Message(json!({ "jsonrpc": "2.0", "id": msg["id"],
                    "error": { "code": -32000, "message": "not logged in" } })));
            }
            let _ = tx.send(Raw::Eof);
        });
        let dir = tempfile::tempdir().unwrap();
        let err = match Agent::start(io, "fake", dir.path()) {
            Ok(_) => panic!("started against an agent that only errors"),
            Err(e) => format!("{e:#}"),
        };
        assert!(err.contains("not logged in"), "{err}");
    }

    #[test]
    fn missing_agent_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let err = match Agent::spawn("codereview-no-such-agent-binary", &[], dir.path()) {
            Ok(_) => panic!("spawned a binary that does not exist"),
            Err(e) => e,
        };
        assert!(err.to_string().contains("cannot start agent"));
    }

    #[test]
    fn paths_stay_inside_the_repository() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.rs"), "x").unwrap();
        let root = dir.path().canonicalize().unwrap();
        assert_eq!(resolve_in(dir.path(), "a.rs").unwrap(), root.join("a.rs"));
        assert_eq!(
            resolve_in(dir.path(), "src/new/file.rs").unwrap(),
            root.join("src/new/file.rs")
        );
        assert_eq!(
            resolve_in(dir.path(), root.join("a.rs").to_str().unwrap()).unwrap(),
            root.join("a.rs")
        );
        assert!(resolve_in(dir.path(), "/etc/hostname").is_err());
        assert!(resolve_in(dir.path(), "../outside.txt").is_err());
        assert!(resolve_in(dir.path(), "src/../../outside.txt").is_err());
        // A link pointing out of the repository is refused whether or not its target is
        // there yet: a link to nothing looks like a file that does not exist, and writing
        // it would create the target wherever the link points.
        let outside = dir.path().parent().unwrap().join("codereview-escape.txt");
        std::os::unix::fs::symlink(&outside, dir.path().join("dangling")).unwrap();
        assert!(resolve_in(dir.path(), "dangling").is_err());
        std::fs::write(&outside, "x").unwrap();
        assert!(resolve_in(dir.path(), "dangling").is_err());
        let _ = std::fs::remove_file(&outside);
        // Nor may the agent write the repository's own machinery.
        assert!(resolve_in(dir.path(), ".git/hooks/pre-commit").is_err());
        assert!(resolve_in(dir.path(), "src/../.git/config").is_err());
    }
}
