//! Running the checks a project already has — its tests, its coverage, its linter, its
//! `make check` — and reading back what they leave behind.
//!
//! A run is a process; its result is a file. Nothing here is kept: [`Run`] lives for as long
//! as the front end holding it does, the same way an agent's transcript does, and what
//! outlives it is whatever the command itself wrote — a `junit.xml`, an `lcov.info`, or
//! nothing at all but an exit code. [`figures`] reads those files when someone asks, so two
//! browsers agree because they are looking at the same file and not at a copy of it.
//!
//! The command runs through `sh -c` with its error output folded into its ordinary output, so
//! that what comes back is in the order it was printed, and with no terminal at all: anything
//! that would stop to ask a question fails instead of waiting for an answer that cannot come.

use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::config::Check;

/// How much of a command's output is kept. A test suite that fails loudly can print
/// megabytes; what is worth showing is the end of it, where the summary is.
const MAX_OUTPUT: usize = 64 * 1024;

/// One run of one check, filled in by the thread that is watching the command.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Run {
    pub name: String,
    /// RFC 3339, so that a page can work out how long ago this was without being told.
    pub started: String,
    /// How long it took. Meaningless until `running` is false.
    pub seconds: f64,
    pub running: bool,
    /// `None` while it runs, and for a command killed by a signal. Zero is success.
    pub exit: Option<i32>,
    /// The tail of what it printed, its error output folded in where it happened.
    pub output: String,
}

impl Run {
    /// Whether a finished run is one to worry about.
    pub fn failed(&self) -> bool {
        !self.running && self.exit != Some(0)
    }
}

/// A handle on a run: the front end reads it, the thread behind it writes.
pub type Running = Arc<Mutex<Run>>;

/// Starts `check` in `root`. Returns as soon as the command is spawned; everything after that
/// arrives in the handle.
pub fn start(check: &Check, root: &Path) -> Result<Running> {
    let run = Arc::new(Mutex::new(Run {
        name: check.name.clone(),
        started: chrono::Local::now().to_rfc3339(),
        seconds: 0.0,
        running: true,
        exit: None,
        output: String::new(),
    }));
    // The shell folds stderr into stdout, which is one pipe to read and keeps the two streams
    // in the order they were printed rather than in the order two readers happened to wake.
    // `exec` rather than a trailing `2>&1`: appended to `a; b` that would redirect only `b`,
    // and a check is quite often more than one command.
    let mut child = Command::new("sh")
        .arg("-c")
        .arg(format!("exec 2>&1\n{}", check.run))
        .current_dir(root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .with_context(|| format!("cannot run {:?}", check.run))?;

    let handle = run.clone();
    std::thread::spawn(move || {
        let started = std::time::Instant::now();
        if let Some(out) = child.stdout.take() {
            for line in BufReader::new(out).lines().map_while(Result::ok) {
                let mut run = handle.lock().unwrap_or_else(|e| e.into_inner());
                append(&mut run.output, &line);
            }
        }
        let status = child.wait();
        let mut run = handle.lock().unwrap_or_else(|e| e.into_inner());
        run.seconds = started.elapsed().as_secs_f64();
        run.exit = status.ok().and_then(|s| s.code());
        run.running = false;
    });
    Ok(run)
}

/// Adds a line, dropping whole lines from the front once there is too much. Whole lines,
/// because half a line of someone's stack trace is worse than none of it.
fn append(output: &mut String, line: &str) {
    output.push_str(line);
    output.push('\n');
    while output.len() > MAX_OUTPUT {
        match output.find('\n') {
            Some(end) => {
                output.drain(..=end);
            }
            None => {
                output.clear();
                break;
            }
        }
    }
}

/// Whether `check` may run in `root` without being asked. The roots in the configuration are
/// compared as git reports them, resolved, so that a path written with a symlink or a trailing
/// slash still matches the repository it names.
pub fn watches(check: &Check, root: &Path) -> bool {
    let resolved = root.canonicalize();
    let root = resolved.as_deref().unwrap_or(root);
    check.watch.iter().any(|w| {
        let w = Path::new(w);
        w.canonicalize().as_deref().unwrap_or(w) == root
    })
}

/// What a check's artefact says, in the terms a dashboard shows.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Figures {
    pub tests: Option<u64>,
    pub failed: Option<u64>,
    pub skipped: Option<u64>,
    /// Lines covered, as a percentage.
    pub coverage: Option<f64>,
    /// When the artefact was last written, RFC 3339. A figure that says how old it is cannot
    /// be mistaken for a figure about what is on screen now.
    pub as_of: Option<String>,
}

/// The figures behind `check`, or `None` when it names no artefact, the artefact is not there,
/// or nothing in it could be read. A check whose command has never run has no artefact, which
/// is the same answer as a check that names none: nothing to show yet.
pub fn figures(check: &Check, root: &Path) -> Option<Figures> {
    let reads = check.reads.as_deref().filter(|r| !r.is_empty())?;
    // Artefact paths come from the user's own configuration, but they are still joined onto a
    // repository root, and a path that climbs out of it is a mistake worth refusing.
    if Path::new(reads).is_absolute() || reads.split('/').any(|part| part == "..") {
        return None;
    }
    let path = root.join(reads);
    let text = std::fs::read_to_string(&path).ok()?;
    let mut figures = match check.kind.as_str() {
        "junit" => from_junit(&text),
        "lcov" => from_lcov(&text),
        _ => None,
    }?;
    figures.as_of = std::fs::metadata(&path)
        .and_then(|m| m.modified())
        .ok()
        .map(|t| chrono::DateTime::<chrono::Local>::from(t).to_rfc3339());
    Some(figures)
}

/// JUnit XML, as nextest, pytest, jest and go-junit-report all write it. The wrapping
/// `<testsuites>` already totals its children when it is there; without one, the suites are
/// added up. Read with a scan rather than an XML parser: these are four integers in
/// attributes, and a whole parser is a dependency to read four integers.
fn from_junit(text: &str) -> Option<Figures> {
    let attr = |tag: &str, name: &str| -> Option<u64> {
        let at = text.find(tag)?;
        let end = text[at..].find('>')? + at;
        let head = &text[at..end];
        let key = format!("{name}=\"");
        let start = head.find(&key)? + key.len();
        let stop = head[start..].find('"')? + start;
        head[start..stop].parse().ok()
    };
    let (tests, failures, errors, skipped) = if text.contains("<testsuites") {
        (
            attr("<testsuites", "tests"),
            attr("<testsuites", "failures"),
            attr("<testsuites", "errors"),
            attr("<testsuites", "skipped"),
        )
    } else {
        let sum = |name: &str| -> Option<u64> {
            let key = format!("{name}=\"");
            let mut total = 0;
            let mut found = false;
            for head in text.split("<testsuite").skip(1) {
                let head = &head[..head.find('>').unwrap_or(head.len())];
                if let Some(start) = head.find(&key) {
                    let start = start + key.len();
                    if let Some(stop) = head[start..].find('"') {
                        if let Ok(n) = head[start..start + stop].parse::<u64>() {
                            total += n;
                            found = true;
                        }
                    }
                }
            }
            found.then_some(total)
        };
        (sum("tests"), sum("failures"), sum("errors"), sum("skipped"))
    };
    let tests = tests?;
    Some(Figures {
        tests: Some(tests),
        failed: Some(failures.unwrap_or(0) + errors.unwrap_or(0)),
        skipped,
        ..Figures::default()
    })
}

/// LCOV tracefiles, as llvm-cov, tarpaulin, jest and lcov itself write them: `LF:` lines found
/// and `LH:` lines hit, per file, added up over the report.
fn from_lcov(text: &str) -> Option<Figures> {
    let (mut found, mut hit) = (0u64, 0u64);
    for line in text.lines() {
        if let Some(n) = line.strip_prefix("LF:") {
            found += n.trim().parse::<u64>().unwrap_or(0);
        } else if let Some(n) = line.strip_prefix("LH:") {
            hit += n.trim().parse::<u64>().unwrap_or(0);
        }
    }
    if found == 0 {
        return None;
    }
    Some(Figures {
        coverage: Some(hit as f64 * 100.0 / found as f64),
        ..Figures::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(run: &str, reads: &str, kind: &str) -> Check {
        Check {
            name: "t".into(),
            run: run.into(),
            reads: Some(reads.into()),
            kind: kind.into(),
            watch: Vec::new(),
        }
    }

    /// The whole of the first step: a command runs, what it prints comes back in order, and
    /// the exit code is what the page colours the row with.
    #[test]
    fn a_command_runs_and_says_how_it_went() {
        let dir = tempfile::tempdir().unwrap();
        let run = start(&check("echo out; echo err >&2; exit 3", "", ""), dir.path()).unwrap();
        for _ in 0..200 {
            if !run.lock().unwrap().running {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let run = run.lock().unwrap();
        assert!(!run.running, "it should have finished");
        assert_eq!(run.exit, Some(3));
        assert!(run.failed());
        // Error output is folded in where it happened, not appended after the fact.
        assert_eq!(run.output, "out\nerr\n", "{:?}", run.output);
    }

    /// The output of a command that will not stop talking is kept whole-lined and bounded.
    #[test]
    fn a_chatty_command_cannot_fill_the_page() {
        let mut output = String::new();
        for i in 0..20_000 {
            append(
                &mut output,
                &format!("line {i} and some more text to make it wider"),
            );
        }
        assert!(output.len() <= MAX_OUTPUT, "{}", output.len());
        assert!(
            output.starts_with("line "),
            "cut at a line: {:?}",
            &output[..20]
        );
        assert!(output.ends_with("line 19999 and some more text to make it wider\n"));
    }

    #[test]
    fn junit_and_lcov_are_read_into_figures() {
        let dir = tempfile::tempdir().unwrap();
        // nextest writes one wrapping <testsuites> that already totals its children.
        std::fs::write(
            dir.path().join("junit.xml"),
            r#"<?xml version="1.0"?>
<testsuites name="nextest" tests="101" failures="1" errors="0" skipped="2">
  <testsuite name="codereview" tests="99" failures="1" errors="0" skipped="2"></testsuite>
  <testsuite name="bin" tests="2" failures="0" errors="0" skipped="0"></testsuite>
</testsuites>"#,
        )
        .unwrap();
        let f = figures(&check("", "junit.xml", "junit"), dir.path()).unwrap();
        assert_eq!(
            (f.tests, f.failed, f.skipped),
            (Some(101), Some(1), Some(2))
        );
        assert!(f.as_of.is_some(), "an artefact says how old it is");

        // Without the wrapper the suites are added up instead, which is what pytest writes.
        std::fs::write(
            dir.path().join("plain.xml"),
            r#"<testsuite name="a" tests="3" failures="1" errors="1"></testsuite>
<testsuite name="b" tests="4" failures="0" errors="0" skipped="1"></testsuite>"#,
        )
        .unwrap();
        let f = figures(&check("", "plain.xml", "junit"), dir.path()).unwrap();
        assert_eq!((f.tests, f.failed, f.skipped), (Some(7), Some(2), Some(1)));

        std::fs::write(
            dir.path().join("lcov.info"),
            "SF:a.rs\nLF:10\nLH:5\nend_of_record\nSF:b.rs\nLF:10\nLH:10\nend_of_record\n",
        )
        .unwrap();
        let f = figures(&check("", "lcov.info", "lcov"), dir.path()).unwrap();
        assert_eq!(f.coverage, Some(75.0));

        // Nothing there, nothing named, and a path that climbs out: all the same answer.
        assert!(figures(&check("", "missing.xml", "junit"), dir.path()).is_none());
        assert!(figures(&check("", "", "junit"), dir.path()).is_none());
        assert!(figures(&check("", "../../etc/passwd", "lcov"), dir.path()).is_none());
    }
}
