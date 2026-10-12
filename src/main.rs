/*  This file is part of codereview.
 *
 *  Copyright (C) 2026 Marko Ivankovic
 *
 *  This program is free software: you can redistribute it and/or modify
 *  it under the terms of the GNU Affero General Public License as published
 *  by the Free Software Foundation, version 3 of the License.
 *
 *  This program is distributed in the hope that it will be useful,
 *  but WITHOUT ANY WARRANTY; without even the implied warranty of
 *  MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
 *  GNU Affero General Public License for more details.
 *
 *  You should have received a copy of the GNU Affero General Public License
 *  along with this program.  If not, see <https://www.gnu.org/licenses/>.
 */

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
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
    ///
    /// Set CODEREVIEW_TOKEN to keep one URL across restarts; without it every run prints a
    /// new one. Behind a reverse proxy, see docs/deploy.md.
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
        /// The address a reverse proxy publishes, `https://review.example.com`: the URL to
        /// print, and over HTTPS the session cookie is marked `Secure`.
        #[arg(long, value_name = "URL")]
        public_url: Option<String>,
        /// Repositories to serve; the page switches between them.
        paths: Vec<String>,
    },
    /// Hash a password for the browser UI, for CODEREVIEW_PASSWORD_HASH.
    ///
    /// Asks for the password twice without echoing it, or reads one line from stdin when
    /// that is not a terminal. The hash goes to stdout, everything else to stderr.
    #[cfg(feature = "web")]
    HashPassword,
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
    /// Write OVERVIEW.md: what the repository is, where to start, what its tests check.
    Overview,
    /// The end-to-end tests, each with how its last recorded run went: what a repository
    /// opens on.
    E2e {
        /// One test, by its name or its phrase: what it ran, what documents it, and the
        /// smaller tests below it; with `--stretch`, how it ran before the stretch and now.
        #[arg(long)]
        test: Option<String>,
        /// What a stretch of history did: `week`, `last`, `uncommitted`, or a commit.
        #[arg(long)]
        stretch: Option<String>,
        /// With `--stretch`: record the tests at the stretch's base first, in a worktree.
        #[arg(long, requires = "stretch")]
        record: bool,
        /// Everything, as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Where a symbol is defined.
    Def { name: String },
    /// Every place an identifier occurs.
    Refs { name: String },
    /// Definitions whose name contains the query.
    Symbols { query: String },
    /// A sample of tests classified by hand, to measure the test classification against.
    Kinds {
        /// The dataset directory, with `kinds.csv` and a directory per sample.
        #[arg(long, default_value = codereview::kinds::DEFAULT_DATASET, global = true)]
        dataset: PathBuf,
        #[command(subcommand)]
        action: KindsAction,
    },
    /// Send one prompt to the configured ACP agent and print its answer.
    Agent {
        prompt: String,
        /// Answer every permission request with the first allow option instead of asking.
        #[arg(long)]
        yes: bool,
    },
}

#[derive(Subcommand)]
enum KindsAction {
    /// Draw tests at random from checkouts of GitHub repositories into the dataset: each with
    /// its repository, commit and file, its text, and where it came from.
    Draw {
        /// Checkouts, or directories of checkouts such as ~/src.
        #[arg(required = true)]
        sources: Vec<PathBuf>,
        /// Tests to add.
        #[arg(long, default_value_t = 20)]
        count: usize,
        /// The seed of the draw; the same seed over the same checkouts draws the same tests.
        /// Without it one is picked and printed.
        #[arg(long)]
        seed: Option<u64>,
        /// At most this many samples from any one repository.
        #[arg(long, default_value_t = 5)]
        per_repository: usize,
    },
    /// Classify the samples by hand, in the terminal; every answer is saved as it is given.
    Label {
        /// The sample to open on; the first one not finished otherwise.
        name: Option<String>,
    },
    /// How the classification's rules agree with the samples classified by hand.
    Agree {
        /// List every sample where they differ.
        #[arg(long)]
        verbose: bool,
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
            public_url,
            paths,
        }) => {
            let all: Vec<String> = cli.paths.iter().chain(&paths).cloned().collect();
            let opened = open_targets(&from, &all)?;
            let sessions = opened.targets.into_iter().map(|(s, _)| s).collect();
            codereview::web::run(
                sessions,
                opened.checkouts,
                codereview::web::Serve {
                    host,
                    port,
                    open_browser: !no_open,
                    public_url,
                },
            )
        }
        #[cfg(feature = "web")]
        Some(Command::HashPassword) => hash_password(),
        Some(Command::List { all, path, json }) => list(&from, all, path.as_deref(), json),
        Some(Command::Add {
            path,
            lines,
            text,
            author,
            no_timestamp,
        }) => add(&from, &path, lines, text, author, no_timestamp),
        Some(Command::Note { path, text }) => note(&from, path.as_deref(), &text),
        Some(Command::Toggle { number }) => toggle(&from, number),
        Some(Command::Reanchor) => reanchor(&from),
        Some(Command::Overview) => overview_file(&from),
        Some(Command::E2e {
            test,
            stretch,
            record,
            json,
        }) => match (stretch, test) {
            (Some(stretch), test) => {
                end_to_end_stretch(&from, &stretch, test.as_deref(), record, json)
            }
            (None, Some(name)) => end_to_end_test(&from, &name, json),
            (None, None) => end_to_end(&from, json),
        },
        Some(Command::Def { name }) => definitions(&from, &name),
        Some(Command::Refs { name }) => references(&from, &name),
        Some(Command::Symbols { query }) => symbols(&from, &query),
        Some(Command::Kinds { dataset, action }) => kinds(&from.join(dataset), action),
        Some(Command::Agent { prompt, yes }) => agent_once(&from, &prompt, yes),
    }
}

/// `add`: a comment on a line, a range, or a whole path.
fn add(
    from: &std::path::Path,
    path: &str,
    lines: String,
    text: Option<String>,
    author: Option<String>,
    no_timestamp: bool,
) -> Result<()> {
    let mut session = Session::open(from)?;
    if author.is_some() {
        session.author = author;
    }
    session.config.timestamps = !no_timestamp;
    // With no text the lines argument is the comment, on the path as a whole.
    let (lines, text) = match text {
        Some(text) => (Some(parse_lines(&lines)?), text),
        None => (None, lines),
    };
    let c = session.add_comment(path, lines, &text, None, None)?;
    println!("added: {} {}", c.location(), c.text);
    Ok(())
}

/// `note`: a note under a file, or under General.
fn note(from: &std::path::Path, path: Option<&str>, text: &str) -> Result<()> {
    let mut session = Session::open(from)?;
    let n = session.add_note(path, text)?;
    println!("noted under {}", n.target);
    Ok(())
}

/// `toggle`: moves a comment between Pending and Completed, by its number in `list --all`.
fn toggle(from: &std::path::Path, number: usize) -> Result<()> {
    let mut session = Session::open(from)?;
    let comments = session.review.comments();
    let Some(target) = comments.get(number.checked_sub(1).unwrap_or(usize::MAX)) else {
        bail!("no comment number {number}; see `codereview list --all`");
    };
    let target = (*target).clone();
    session.toggle_comment(&target)?;
    let now = if target.is_pending() {
        "completed"
    } else {
        "pending"
    };
    println!("{} is now {now}", target.location());
    Ok(())
}

/// `reanchor`: rewrites the line numbers of comments whose lines have moved.
fn reanchor(from: &std::path::Path) -> Result<()> {
    let mut session = Session::open(from)?;
    let moved = session.reanchor()?;
    println!("{moved} comment(s) re-anchored");
    Ok(())
}

/// `overview`: writes `OVERVIEW.md` now.
fn overview_file(from: &std::path::Path) -> Result<()> {
    let mut session = Session::open(from)?;
    let wrote = session.write_facts(None)?;
    println!(
        "{} {}",
        codereview::facts::FILE_NAME,
        if wrote { "written" } else { "unchanged" }
    );
    Ok(())
}

/// `e2e`: the end-to-end tests and their recordings, a line each.
fn end_to_end(from: &std::path::Path, json: bool) -> Result<()> {
    let mut session = Session::open(from)?;
    let opening = session.end_to_end();
    if json {
        println!("{}", serde_json::to_string_pretty(&opening)?);
        return Ok(());
    }
    if opening.missing() {
        println!(
            "No end-to-end tests: nothing drives {} the way a person does.",
            opening.kind.as_deref().unwrap_or("the software")
        );
        return Ok(());
    }
    for t in &opening.tests {
        let (mark, how) = match &t.recording {
            None => ("○", "no recording".to_string()),
            Some(r) => {
                let stale = match t.stale {
                    Some(s) if s.commits == 0 && s.files == 0 => ", matches the tree".into(),
                    Some(s) => format!(", {} commits and {} files since", s.commits, s.files),
                    None => String::new(),
                };
                let what = r.media().unwrap_or_else(|| "no recording".into());
                (if r.passed { "✓" } else { "✗" }, format!("{what}{stale}"))
            }
        };
        println!(
            "{mark} {}:{} {} ({how})",
            t.test.path, t.test.line, t.test.label
        );
        for f in t.recording.iter().flat_map(|r| r.failures()) {
            println!("    ✗ line {}: {}", f.line, f.text);
        }
    }
    println!(
        "{} end-to-end tests of {}, {} failed, {} without a recording",
        opening.tests.len(),
        opening.total,
        opening.failed,
        opening.unrecorded
    );
    Ok(())
}

/// `e2e --stretch`: what a stretch did, through the end-to-end tests; with `--test`, one of
/// them before and after.
fn end_to_end_stretch(
    from: &std::path::Path,
    stretch: &str,
    test: Option<&str>,
    record: bool,
    json: bool,
) -> Result<()> {
    use codereview::spec::Stretch;
    let stretch = match stretch {
        "week" => Stretch::Week,
        "last" => Stretch::LastCommit,
        "uncommitted" => Stretch::Uncommitted,
        hash => Stretch::Commit(hash.to_string()),
    };
    let mut session = Session::open(from)?;
    if record {
        let span = codereview::spec::span(&session, &stretch)?;
        let base = span
            .from
            .with_context(|| "there is no commit before this stretch to record at")?;
        let (worktree, check) = codereview::stretch::prepare(&session, &base)?;
        eprintln!("running {} in {}", check.run, worktree.display());
        let status = std::process::Command::new("sh")
            .arg("-c")
            .arg(&check.run)
            .current_dir(&worktree)
            .status()?;
        if !status.success() {
            eprintln!(
                "{} failed; its recordings are compared as far as they go",
                check.run
            );
        }
    }
    if let Some(name) = test {
        let file = session
            .end_to_end()
            .tests
            .into_iter()
            .find(|t| t.test.name == name || t.test.label == name)
            .map(|t| t.test.path)
            .with_context(|| format!("no end-to-end test called {name:?}"))?;
        let c = codereview::stretch::compare(&mut session, &stretch, &file, name)?;
        if json {
            println!("{}", serde_json::to_string_pretty(&c)?);
            return Ok(());
        }
        let mark = |r: &Option<codereview::recording::Recording>| match r {
            None => "not run",
            Some(r) if r.passed => "passed",
            Some(_) => "failed",
        };
        let at = c.base.as_ref().map_or("—".to_string(), |b| b.short.clone());
        println!(
            "{name}: {} at {at}, {} now",
            mark(&c.before),
            mark(&c.after)
        );
        for p in &c.steps {
            let text = |s: &Option<codereview::recording::Step>| {
                s.as_ref().map_or("—".to_string(), |s| s.text.clone())
            };
            let what = match (&p.before, &p.after, p.same_screen) {
                (None, _, _) => "new",
                (_, None, _) => "gone",
                (_, _, Some(false)) => "screen changed",
                (_, _, Some(true)) => "same screen",
                _ => "",
            };
            println!("  {:<40} {:<40} {what}", text(&p.before), text(&p.after));
        }
        return Ok(());
    }
    let r = codereview::stretch::report(&mut session, &stretch)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&r)?);
        return Ok(());
    }
    println!("{}", r.span.label);
    for c in r.changes.iter().filter(|c| c.what.alarming()) {
        println!("  {} {} ({}:{})", c.what.word(), c.label, c.path, c.line);
    }
    println!(
        "{} of {} changed lines no end-to-end test ran",
        r.unran_lines, r.changed
    );
    for u in &r.unran {
        println!("  {} — {} of {} lines", u.path, u.lines, u.changed);
    }
    if r.without_coverage > 0 {
        println!("({} end-to-end tests left no coverage)", r.without_coverage);
    }
    for t in &r.touched {
        println!("ran {} changed lines: {}", t.lines, t.label);
    }
    match &r.base {
        None => println!("no commit before it to compare with"),
        Some(b) if b.recorded == 0 => println!(
            "not recorded at {}{}",
            b.short,
            b.command
                .as_ref()
                .map_or(String::new(), |c| format!("; --record runs {c} there"))
        ),
        Some(b) => {
            println!("before: recorded at {}", b.short);
            for o in &r.outcomes {
                let m = |v: Option<bool>| match v {
                    None => "–",
                    Some(true) => "✓",
                    Some(false) => "✗",
                };
                println!(
                    "  {} → {} {}{}",
                    m(o.before),
                    m(o.after),
                    o.label,
                    o.screens_changed
                        .filter(|n| *n > 0)
                        .map_or(String::new(), |n| format!(" (screen changed at {n} steps)"))
                );
            }
        }
    }
    Ok(())
}

/// `e2e --test`: one end-to-end test, and where it leads.
fn end_to_end_test(from: &std::path::Path, name: &str, json: bool) -> Result<()> {
    let mut session = Session::open(from)?;
    let file = session
        .end_to_end()
        .tests
        .into_iter()
        .find(|t| t.test.name == name || t.test.label == name)
        .map(|t| t.test.path)
        .with_context(|| format!("no end-to-end test called {name:?}"))?;
    let d = codereview::e2e::detail(&mut session, &file, name)
        .with_context(|| format!("no end-to-end test called {name:?}"))?;
    if json {
        println!("{}", serde_json::to_string_pretty(&d)?);
        return Ok(());
    }
    println!(
        "{} ({}:{})",
        d.test.test.label, d.test.test.path, d.test.test.line
    );
    if let Some(r) = &d.test.recording {
        println!("\nsteps");
        for (s, source) in r.steps.iter().zip(&d.sources) {
            let mark = match s.passed {
                Some(true) => "✓ ",
                Some(false) => "✗ ",
                None => "",
            };
            println!("  {:>6.1}s {mark}{}", s.at_ms as f64 / 1000.0, s.text);
            if let Some(source) = source {
                println!("          {source}");
            }
        }
    }
    println!("\ncode it ran");
    match &d.ran {
        None => println!("  the run left no coverage.lcov"),
        Some(files) => {
            for f in files {
                println!(
                    "  {} — {} lines, {} only this test",
                    f.path, f.lines, f.only_here
                );
            }
        }
    }
    println!("\ndocumentation");
    for doc in &d.docs {
        let how = if doc.declared {
            "named by the test".to_string()
        } else {
            format!("a guess: {}", doc.shared.join(", "))
        };
        println!(
            "  {} — {}:{} ({how})",
            doc.section.heading, doc.section.path, doc.section.line
        );
    }
    println!("\nsmaller tests that reach the same code");
    for t in &d.smaller {
        println!(
            "  {} ({}:{}) — {}",
            t.test.test.label,
            t.test.test.path,
            t.test.test.line,
            t.shared.join(", ")
        );
    }
    Ok(())
}

/// `def`: where a name is defined.
fn definitions(from: &std::path::Path, name: &str) -> Result<()> {
    let mut session = Session::open(from)?;
    let defs = session.definitions(name);
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

/// `refs`: every occurrence of a name.
fn references(from: &std::path::Path, name: &str) -> Result<()> {
    let mut session = Session::open(from)?;
    let refs = session.references(name);
    if refs.is_empty() {
        println!("no occurrence of {name}");
    }
    for r in refs {
        println!("{}:{}:{}: {}", r.path, r.line, r.column + 1, r.text);
    }
    Ok(())
}

/// `symbols`: definitions whose name contains the query.
fn symbols(from: &std::path::Path, query: &str) -> Result<()> {
    let mut session = Session::open(from)?;
    for s in session.search_symbols(query) {
        println!("{}:{}: {} {}", s.path, s.line, s.kind, s.name);
    }
    Ok(())
}

/// Asks for a password and prints its hash. The password is never written anywhere: it is
/// read without an echo when there is a terminal to read from, and only the hash is printed.
#[cfg(feature = "web")]
fn hash_password() -> Result<()> {
    use std::io::IsTerminal;
    const MIN: usize = 12;
    let interactive = std::io::stdin().is_terminal();
    let password = if interactive {
        let first = rpassword::prompt_password("Password: ")?;
        let again = rpassword::prompt_password("Again: ")?;
        if first != again {
            bail!("the two do not match");
        }
        first
    } else {
        let mut line = String::new();
        std::io::stdin().read_line(&mut line)?;
        line.trim_end_matches(['\n', '\r']).to_string()
    };
    if password.chars().count() < MIN {
        bail!(
            "a password guarding a published site should be at least {MIN} characters; \
             a few unrelated words make a good one"
        );
    }
    let hash = codereview::web::hash_password(&password)?;
    println!("{hash}");
    if interactive {
        eprintln!("\nPut that in the service environment, quoted. In /etc/codereview.env:");
        eprintln!("  CODEREVIEW_PASSWORD_HASH='{hash}'");
    }
    Ok(())
}

#[cfg(any(feature = "tui", feature = "web"))]
fn open_targets(from: &std::path::Path, paths: &[String]) -> Result<codereview::session::Opened> {
    // The user's own file, the same one `Session::open` reads for every other subcommand.
    // Passing `None` here means the defaults, which is not the same thing at all: it is a
    // colour scheme the page cannot save, an agent that cannot be configured, and no checks.
    codereview::session::open_targets(from, paths, codereview::config::Config::path(), |m| {
        eprintln!("{m}")
    })
}

/// One prompt, answer streamed to stdout, permission requests answered on stdin (or
/// auto-allowed with `--yes`), tool calls and diagnostics on stderr.
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
                    details,
                    options,
                } => {
                    let choice = if yes {
                        options
                            .iter()
                            .find(|o| o.kind.starts_with("allow"))
                            .or(options.first())
                    } else {
                        eprintln!("permission: {title}");
                        if let Some(details) = &details {
                            eprintln!("{details}");
                        }
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

/// `codereview kinds`: draw a sample, label it, or say how the rules agree with it.
fn kinds(dataset: &Path, action: KindsAction) -> Result<()> {
    use codereview::kinds::{Dataset, Draw, agreement, draw};
    match action {
        KindsAction::Draw {
            sources,
            count,
            seed,
            per_repository,
        } => {
            let seed = seed.unwrap_or_else(|| {
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(1, |d| d.as_secs())
            });
            let mut set = Dataset::open(dataset)?;
            let how = Draw {
                count,
                seed,
                per_repository,
            };
            let drawn = draw(&mut set, &sources, &how)?;
            for skipped in &drawn.passed_over {
                eprintln!("passed over {skipped}");
            }
            for name in &drawn.added {
                println!("{name}");
            }
            eprintln!(
                "drew {} of {} tests in {} repositories into {} (seed {seed}); {} samples now",
                drawn.added.len(),
                drawn.candidates,
                drawn.repositories,
                dataset.display(),
                set.samples.len()
            );
            Ok(())
        }
        #[cfg(feature = "tui")]
        KindsAction::Label { name } => codereview::tui::kinds::run(dataset, name.as_deref()),
        #[cfg(not(feature = "tui"))]
        KindsAction::Label { .. } => bail!("built without the tui feature"),
        KindsAction::Agree { verbose } => {
            let set = Dataset::open(dataset)?;
            let rows = agreement(&set)?;
            let finished = rows.first().map_or(0, |r| r.finished);
            if finished == 0 {
                println!("no sample is finished yet; label some with `codereview kinds label`");
                return Ok(());
            }
            println!("{finished} samples classified by hand\n");
            for row in &rows {
                let options: Vec<String> = row
                    .options
                    .iter()
                    .map(|(o, n)| format!("{o} {n}"))
                    .collect();
                println!(
                    "{:<62} {:>3}/{:<3} options: {}",
                    row.question.title(),
                    row.agree,
                    row.finished,
                    options.join(", ")
                );
                if verbose {
                    for (sample, person, rules, why) in &row.differ {
                        println!("    {sample}: by hand {person}, rules {rules} ({why})");
                    }
                }
            }
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_specifications() {
        assert_eq!(parse_lines("12").unwrap(), (12, 12));
        assert_eq!(parse_lines("12-20").unwrap(), (12, 20));
        assert_eq!(parse_lines(" 3 - 4 ").unwrap(), (3, 4));
        assert_eq!(parse_lines("7-7").unwrap(), (7, 7));
        for bad in ["0", "0-3", "20-12", "", "-", "x", "1-", "1-x", "-3", "1.5"] {
            assert!(parse_lines(bad).is_err(), "{bad:?} should not parse");
        }
    }

    /// A repository with one file, for the numbering test below. The library's own scratch
    /// repository is not reachable from here: the binary is its own crate.
    fn scratch_repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let git = |args: &[&str]| {
            let out = std::process::Command::new("git")
                .arg("-C")
                .arg(dir.path())
                .args(args)
                .output()
                .unwrap();
            assert!(out.status.success(), "git {args:?}");
        };
        git(&["init", "-q", "-b", "main"]);
        git(&["config", "user.email", "t@example.com"]);
        git(&["config", "user.name", "Tester"]);
        std::fs::write(dir.path().join("a.rs"), "fn a() {}\nfn b() {}\n").unwrap();
        std::fs::write(dir.path().join("Makefile"), "all:\n\techo hi\n").unwrap();
        git(&["add", "."]);
        git(&["commit", "-q", "-m", "first"]);
        dir
    }

    /// `list` numbers comments by their order in the file, and `toggle` takes the same
    /// number: the two must agree, or a toggle moves the wrong comment.
    #[test]
    fn listing_and_toggling_agree_on_the_numbers() {
        let dir = scratch_repo();
        let config = dir.path().join(".git/codereview.toml");
        let mut session = Session::open_with_config(dir.path(), Some(config.clone())).unwrap();
        session
            .add_comment("a.rs", Some((1, 1)), "first", None, None)
            .unwrap();
        session
            .add_comment("a.rs", Some((2, 2)), "second", None, None)
            .unwrap();
        session
            .add_comment("Makefile", None, "third", None, None)
            .unwrap();

        let numbered = |s: &Session| -> Vec<(usize, String)> {
            s.review
                .comments()
                .iter()
                .enumerate()
                .map(|(i, c)| (i + 1, c.text.clone()))
                .collect()
        };
        assert_eq!(
            numbered(&session),
            vec![
                (1, "first".to_string()),
                (2, "second".to_string()),
                (3, "third".to_string())
            ]
        );

        // Toggling number two completes the second comment and nothing else.
        let target = session.review.comments()[1].clone();
        session.toggle_comment(&target).unwrap();
        let session = Session::open_with_config(dir.path(), Some(config)).unwrap();
        let by_text = |text: &str| {
            session
                .review
                .comments()
                .iter()
                .find(|c| c.text == text)
                .map(|c| c.is_pending())
                .unwrap()
        };
        assert!(by_text("first"));
        assert!(!by_text("second"), "the second one was completed");
        assert!(by_text("third"));
        // And the numbers still line up, completed comments included.
        assert_eq!(numbered(&session).len(), 3);
    }
}
