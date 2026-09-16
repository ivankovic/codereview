//! How codereview talks to an agent: a child process with pipes, or, in tests, a thread
//! standing in for one. Both backends speak newline-delimited JSON over it, and neither knows
//! which it has.
//!
//! The reading happens on threads of its own so that a backend's `poll` never blocks: every
//! line arrives as a [`Raw`] on a channel, whether it parsed or not.

use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{Receiver, Sender};
use std::time::Instant;

use anyhow::{Context, Result};
use serde_json::Value;

use crate::agent::Event;

/// A line from an agent process, shared with the Claude Code backend.
pub(crate) enum Raw {
    Message(Value),
    Stderr(String),
    Eof,
}

/// Where an agent's lines come from and go to: a child process with its pipes, or, in
/// tests, a thread standing in for one. Both hand lines back through the same channel, so
/// the backends never know which they talk to.
pub(crate) struct Transport {
    writer: Box<dyn Write + Send>,
    pub(crate) rx: Receiver<Raw>,
    child: Option<Child>,
    description: String,
}

impl Transport {
    /// Spawns `command` with piped stdio. Stdout lines that parse as JSON become messages,
    /// the rest and stderr become diagnostics; `description` names the command for logs.
    pub(crate) fn spawn(mut command: Command, description: String) -> Result<Self> {
        // The agent leads a process group of its own, so that stopping it stops whatever it
        // started: an `npx` wrapper otherwise leaves the node process behind, holding the
        // pipes and whatever it inherited.
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| format!("cannot start agent {description}"))?;
        let stdin = child.stdin.take().context("agent stdin")?;
        let stdout = child.stdout.take().context("agent stdout")?;
        let stderr = child.stderr.take().context("agent stderr")?;
        let (tx, rx) = std::sync::mpsc::channel::<Raw>();
        spawn_reader(stdout, tx.clone());
        spawn_stderr_reader(stderr, tx);
        let description = format!("`{description}` (pid {})", child.id());
        Ok(Self {
            writer: Box::new(stdin),
            rx,
            child: Some(child),
            description,
        })
    }

    /// A stand-in on a thread: `serve` receives the lines the client writes and answers on
    /// the channel; it sends [`Raw::Eof`] when it stops.
    #[cfg(test)]
    pub(crate) fn in_process(
        serve: impl FnOnce(Receiver<String>, Sender<Raw>) + Send + 'static,
    ) -> Self {
        let (line_tx, line_rx) = std::sync::mpsc::channel::<String>();
        let (tx, rx) = std::sync::mpsc::channel::<Raw>();
        std::thread::spawn(move || serve(line_rx, tx));
        Self {
            writer: Box::new(LineSender {
                tx: line_tx,
                pending: String::new(),
            }),
            rx,
            child: None,
            description: "in-process fake".into(),
        }
    }

    /// `\`claude -p\` (pid 12)`, or `in-process fake`.
    pub(crate) fn description(&self) -> &str {
        &self.description
    }

    /// Sends one JSON message: the line protocol both backends speak.
    pub(crate) fn send_json(&mut self, message: &Value) -> Result<()> {
        let mut line = serde_json::to_string(message)?;
        line.push('\n');
        self.send_line(&line)
    }

    /// Waits for the next line until `deadline`, or `None` when it passes or the agent is
    /// gone. Both handshakes wait like this, for an answer that may be slow to come.
    pub(crate) fn recv_until(&self, deadline: Instant) -> Option<Raw> {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return None;
        }
        self.rx.recv_timeout(remaining).ok()
    }

    /// The last few things the agent said on stderr, which is usually the real explanation
    /// when a handshake fails. `queued` holds what was collected before the failure.
    pub(crate) fn last_words(&self, queued: &[Event]) -> Vec<String> {
        let mut said: Vec<String> = queued
            .iter()
            .filter_map(|ev| match ev {
                Event::Stderr { text } => Some(text.clone()),
                _ => None,
            })
            .collect();
        while let Ok(Raw::Stderr(text)) = self.rx.try_recv() {
            said.push(text);
        }
        let from = said.len().saturating_sub(6);
        said.split_off(from)
    }

    pub(crate) fn send_line(&mut self, line: &str) -> Result<()> {
        self.writer
            .write_all(line.as_bytes())
            .context("agent stdin closed")?;
        self.writer.flush().context("agent stdin closed")
    }

    pub(crate) fn kill(&mut self) {
        if let Some(child) = &mut self.child {
            kill_group(child);
            let _ = child.kill();
        }
    }

    /// What to say when the output closed: the exit status once the process has ended.
    pub(crate) fn exit_message(&mut self, who: &str) -> String {
        match self.child.as_mut().map(Child::try_wait) {
            Some(Ok(Some(status))) => format!("{who} exited with {status}"),
            _ => format!("{who} closed its output"),
        }
    }
}

impl Drop for Transport {
    fn drop(&mut self) {
        if let Some(child) = &mut self.child {
            kill_group(child);
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// Kills everything the agent started along with the agent. Safe while the `Child` is held:
/// the process is not reaped until then, so its identifier cannot have been reused.
#[cfg(unix)]
fn kill_group(child: &std::process::Child) {
    // SAFETY: a signal to a process group; the group is the agent's own, made by
    // `process_group(0)` above.
    unsafe {
        libc::killpg(child.id() as i32, libc::SIGKILL);
    }
}

#[cfg(not(unix))]
fn kill_group(_child: &std::process::Child) {}

/// Hands complete lines to the in-process fake.
#[cfg(test)]
struct LineSender {
    tx: Sender<String>,
    pending: String,
}

#[cfg(test)]
impl Write for LineSender {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.pending.push_str(&String::from_utf8_lossy(bytes));
        while let Some(i) = self.pending.find('\n') {
            let line = self.pending[..i].to_string();
            self.pending.drain(..=i);
            if self.tx.send(line).is_err() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::BrokenPipe,
                    "the fake agent is gone",
                ));
            }
        }
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Forwards an agent's stderr lines into the channel.
pub(crate) fn spawn_stderr_reader(stderr: std::process::ChildStderr, tx: Sender<Raw>) {
    std::thread::spawn(move || {
        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
            if tx.send(Raw::Stderr(line)).is_err() {
                break;
            }
        }
    });
}

/// The longest line an agent may send. Its output is newline-delimited JSON; a line longer
/// than this is a runaway, not a message, and reading it would grow until memory ran out.
const MAX_LINE: u64 = 8 * 1024 * 1024;

pub(crate) fn spawn_reader(stdout: std::process::ChildStdout, tx: Sender<Raw>) {
    std::thread::spawn(move || {
        let reader = BufReader::new(stdout).take(MAX_LINE);
        for line in reader.lines() {
            let Ok(line) = line else { break };
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            match serde_json::from_str::<Value>(line) {
                Ok(v) => {
                    if tx.send(Raw::Message(v)).is_err() {
                        return;
                    }
                }
                Err(_) => {
                    // Not JSON: an adapter printing a banner. Show it as diagnostics.
                    if tx.send(Raw::Stderr(line.to_string())).is_err() {
                        return;
                    }
                }
            }
        }
        let _ = tx.send(Raw::Eof);
    });
}
