/*  This file is part of codereview.
 *
 *  Copyright (C) 2026 Marko Ivankovic
 *
 *  This program is free software: you can redistribute it and/or modify
 *  it under the terms of the GNU Affero General Public License as published
 *  by the Free Software Foundation, either version 3 of the License, or
 *  (at your option) any later version.
 *
 *  This program is distributed in the hope that it will be useful,
 *  but WITHOUT ANY WARRANTY; without even the implied warranty of
 *  MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
 *  GNU Affero General Public License for more details.
 *
 *  You should have received a copy of the GNU Affero General Public License
 *  along with this program.  If not, see <https://www.gnu.org/licenses/>.
 */

use std::path::PathBuf;

use anyhow::{Result, bail};
use clap::{Parser, Subcommand};
use codereview::anchor::AnchorState;
use codereview::session::Session;

#[derive(Parser)]
#[command(
    name = "codereview",
    version,
    about = "Explore and review a repository; comments go to REVIEW.md, notes to NOTES.md"
)]
struct Cli {
    /// Repository, or any directory inside one. Defaults to the current directory.
    #[arg(global = true, short = 'C', long = "repo", value_name = "DIR")]
    repo: Option<PathBuf>,

    /// Repositories to open, or files to open inside their repositories; several may be
    /// named. Defaults to the repository around the current directory.
    #[arg(value_name = "PATH")]
    paths: Vec<String>,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Open the terminal UI (the default).
    #[cfg(feature = "tui")]
    Tui {
        /// Repositories, or files inside them, to open.
        paths: Vec<String>,
    },
    /// Serve the browser UI on localhost and open it.
    #[cfg(feature = "web")]
    Web {
        #[arg(long, default_value = "127.0.0.1")]
        host: String,
        /// 0 picks a free port.
        #[arg(long, default_value_t = 0)]
        port: u16,
        /// Do not open a browser.
        #[arg(long)]
        no_open: bool,
        /// Repositories to serve; the page switches between them.
        paths: Vec<String>,
    },
    /// Print comments from REVIEW.md.
    List {
        /// Include completed comments.
        #[arg(long)]
        all: bool,
        /// Only this file.
        #[arg(long)]
        path: Option<String>,
        /// Machine-readable output, with each comment's current anchor state.
        #[arg(long)]
        json: bool,
    },
    /// Add a pending comment: `codereview add src/main.rs 12 "text"`, or `12-15`, or leave the
    /// lines out (`codereview add src "text"`) to comment on a whole file or directory.
    Add {
        path: String,
        /// `N` or `N-M`. With no `text` after it this is the comment, on the whole path.
        #[arg(value_name = "LINES_OR_TEXT")]
        lines: String,
        text: Option<String>,
        /// Override the author (defaults to git user.name).
        #[arg(long)]
        author: Option<String>,
        /// Leave out the timestamp.
        #[arg(long)]
        no_timestamp: bool,
    },
    /// Add a note to NOTES.md, under a file or under General.
    Note {
        /// File path, or omit for General.
        #[arg(long)]
        path: Option<String>,
        text: String,
    },
    /// Move a comment between Pending and Completed, by its number in `list --all`.
    Toggle { number: usize },
    /// Rewrite REVIEW.md line numbers for comments whose lines have moved.
    Reanchor,
    /// Where a symbol is defined.
    Def { name: String },
    /// Every place an identifier occurs.
    Refs { name: String },
    /// Definitions whose name contains the query.
    Symbols { query: String },
    /// Send one prompt to the configured ACP agent and print its answer.
    Agent {
        prompt: String,
        /// Answer every permission request with the first allow option instead of asking.
        #[arg(long)]
        yes: bool,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let from = cli.repo.unwrap_or_else(|| PathBuf::from("."));
    match cli.command {
        #[cfg(feature = "tui")]
        None => codereview::tui::run(open_targets(&from, &cli.paths)?),
        #[cfg(not(feature = "tui"))]
        None => bail!("built without the tui feature; use a subcommand"),
        #[cfg(feature = "tui")]
        Some(Command::Tui { paths }) => {
            let all: Vec<String> = cli.paths.iter().chain(&paths).cloned().collect();
            codereview::tui::run(open_targets(&from, &all)?)
        }
        #[cfg(feature = "web")]
        Some(Command::Web {
            host,
            port,
            no_open,
            paths,
        }) => {
            let all: Vec<String> = cli.paths.iter().chain(&paths).cloned().collect();
            let sessions = open_targets(&from, &all)?
                .into_iter()
                .map(|(s, _)| s)
                .collect();
            codereview::web::run(sessions, &host, port, !no_open)
        }
        Some(Command::List { all, path, json }) => list(&from, all, path.as_deref(), json),
        Some(Command::Add {
            path,
            lines,
            text,
            author,
            no_timestamp,
        }) => {
            let mut session = Session::open(&from)?;
            if author.is_some() {
                session.author = author;
            }
            session.timestamps = !no_timestamp;
            // With no text the lines argument is the comment, on the path as a whole.
            let (lines, text) = match text {
                Some(text) => (Some(parse_lines(&lines)?), text),
                None => (None, lines),
            };
            let c = session.add_comment(&path, lines, &text, None)?;
            println!("added: {} {}", c.location(), c.text);
            Ok(())
        }
        Some(Command::Note { path, text }) => {
            let mut session = Session::open(&from)?;
            let n = session.add_note(path.as_deref(), &text)?;
            println!("noted under {}", n.target);
            Ok(())
        }
        Some(Command::Toggle { number }) => {
            let mut session = Session::open(&from)?;
            let comments = session.review.comments();
            let Some(target) = comments.get(number.checked_sub(1).unwrap_or(usize::MAX)) else {
                bail!("no comment number {number}; see `codereview list --all`");
            };
            let target = (*target).clone();
            session.toggle_comment(&target)?;
            println!(
                "{} is now {}",
                target.location(),
                if target.is_pending() {
                    "completed"
                } else {
                    "pending"
                }
            );
            Ok(())
        }
        Some(Command::Reanchor) => {
            let mut session = Session::open(&from)?;
            let moved = session.reanchor()?;
            println!("{moved} comment(s) re-anchored");
            Ok(())
        }
        Some(Command::Def { name }) => {
            let mut session = Session::open(&from)?;
            let defs = session.definitions(&name);
            if defs.is_empty() {
                println!("no definition of {name}");
            }
            for d in defs {
                let container = d
                    .container
                    .as_deref()
                    .map(|c| format!(" in {c}"))
                    .unwrap_or_default();
                println!(
                    "{}:{}: {} {}{}: {}",
                    d.path, d.line, d.kind, d.name, container, d.text
                );
            }
            Ok(())
        }
        Some(Command::Refs { name }) => {
            let mut session = Session::open(&from)?;
            let refs = session.references(&name);
            if refs.is_empty() {
                println!("no occurrence of {name}");
            }
            for r in refs {
                println!("{}:{}:{}: {}", r.path, r.line, r.column + 1, r.text);
            }
            Ok(())
        }
        Some(Command::Symbols { query }) => {
            let mut session = Session::open(&from)?;
            for s in session.search_symbols(&query) {
                println!("{}:{}: {} {}", s.path, s.line, s.kind, s.name);
            }
            Ok(())
        }
        Some(Command::Agent { prompt, yes }) => agent_once(&from, &prompt, yes),
    }
}

/// One prompt, answer streamed to stdout, permission requests answered on stdin (or
/// auto-allowed with `--yes`), tool calls and diagnostics on stderr.
#[cfg(any(feature = "tui", feature = "web"))]
fn open_targets(
    from: &std::path::Path,
    paths: &[String],
) -> Result<Vec<(Session, Option<String>)>> {
    codereview::session::open_targets(from, paths, None, |m| eprintln!("{m}"))
}

fn agent_once(from: &std::path::Path, prompt: &str, yes: bool) -> Result<()> {
    use codereview::agent::{Event, Role};
    use std::io::Write;
    let session = Session::open(from)?;
    eprintln!("starting the agent ...");
    let mut agent = session.spawn_agent()?;
    eprintln!("connected to {}", agent.name());
    agent.prompt(prompt, &[])?;
    let stdout = std::io::stdout();
    loop {
        let events = agent.poll();
        if events.is_empty() {
            if !agent.is_alive() {
                bail!("agent exited");
            }
            std::thread::sleep(std::time::Duration::from_millis(30));
            continue;
        }
        for event in events {
            match event {
                Event::Text {
                    role: Role::Agent,
                    text,
                } => {
                    let mut out = stdout.lock();
                    out.write_all(text.as_bytes())?;
                    out.flush()?;
                }
                Event::Text {
                    role: Role::Thought,
                    ..
                }
                | Event::Text {
                    role: Role::User, ..
                } => {}
                Event::ToolCall { title, status, .. } => {
                    if let Some(t) = title {
                        eprintln!(
                            "[tool] {t}{}",
                            status.map(|s| format!(" ({s})")).unwrap_or_default()
                        );
                    }
                }
                Event::Plan { entries } => {
                    for (content, status) in entries {
                        eprintln!("[plan] {status}: {content}");
                    }
                }
                Event::Permission {
                    request_id,
                    title,
                    options,
                } => {
                    let choice = if yes {
                        options
                            .iter()
                            .find(|o| o.kind.starts_with("allow"))
                            .or(options.first())
                    } else {
                        eprintln!("permission: {title}");
                        for (i, o) in options.iter().enumerate() {
                            eprintln!("  {}. {} ({})", i + 1, o.name, o.kind);
                        }
                        eprint!("choice [1]: ");
                        let mut line = String::new();
                        std::io::stdin().read_line(&mut line)?;
                        let n: usize = line.trim().parse().unwrap_or(1);
                        options.get(n.saturating_sub(1)).or(options.first())
                    };
                    agent.respond_permission(&request_id, choice.map(|o| o.option_id.as_str()))?;
                }
                Event::TurnDone { stop_reason } => {
                    println!();
                    if stop_reason != "end_turn" {
                        eprintln!("stopped: {stop_reason}");
                    }
                    return Ok(());
                }
                Event::Error { message } => bail!("agent error: {message}"),
                Event::Stderr { text } => eprintln!("[agent] {text}"),
                Event::Log { text } => eprintln!("[log] {text}"),
                Event::Status { .. } | Event::Usage { .. } => {}
                Event::Exited { message } => bail!("{message}"),
            }
        }
    }
}

fn parse_lines(spec: &str) -> Result<(usize, usize)> {
    let mut parts = spec.splitn(2, '-');
    let line: usize = parts
        .next()
        .and_then(|s| s.trim().parse().ok())
        .filter(|l| *l > 0)
        .ok_or_else(|| anyhow::anyhow!("lines must be N or N-M, got {spec:?}"))?;
    let end = match parts.next() {
        Some(s) => s
            .trim()
            .parse()
            .ok()
            .filter(|e| *e >= line)
            .ok_or_else(|| anyhow::anyhow!("lines must be N or N-M, got {spec:?}"))?,
        None => line,
    };
    Ok((line, end))
}

fn list(from: &std::path::Path, all: bool, path: Option<&str>, json: bool) -> Result<()> {
    let session = Session::open(from)?;
    let anchored = session.all_anchored();
    // Numbering follows the file's own order, the same one `toggle` uses.
    let order: Vec<_> = session.review.comments();
    let mut rows: Vec<(usize, &codereview::anchor::Anchored)> = anchored
        .iter()
        .filter(|a| all || a.comment.is_pending())
        .filter(|a| path.is_none_or(|p| a.comment.path == p))
        .map(|a| {
            let n = order
                .iter()
                .position(|c| **c == a.comment)
                .map_or(0, |i| i + 1);
            (n, a)
        })
        .collect();
    rows.sort_by_key(|(n, _)| *n);
    if json {
        let items: Vec<serde_json::Value> = rows
            .iter()
            .map(|(n, a)| {
                let mut v = serde_json::to_value(a).expect("serializable");
                v["number"] = serde_json::Value::from(*n);
                v
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&items)?);
        return Ok(());
    }
    if rows.is_empty() {
        println!("no {}comments", if all { "" } else { "pending " });
        return Ok(());
    }
    for (n, a) in rows {
        let state = match a.state {
            AnchorState::Exact => "",
            AnchorState::Moved => " (moved)",
            AnchorState::Stale => " (stale)",
        };
        let section = if a.comment.is_pending() {
            ""
        } else {
            " [done]"
        };
        println!(
            "{n:3}. {}{state}{section}: {}",
            a.location(),
            a.comment.text
        );
    }
    Ok(())
}
