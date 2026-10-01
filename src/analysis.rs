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
use std::sync::{Arc, Mutex, OnceLock};

use anyhow::{Context, Result};
use regex::Regex;
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

/// Whether `left` and `right` name the same directory, symlinks and trailing slashes aside.
fn same_dir(left: &Path, right: &Path) -> bool {
    let l = left.canonicalize();
    let r = right.canonicalize();
    l.as_deref().unwrap_or(left) == r.as_deref().unwrap_or(right)
}

/// Whether `check` belongs to the repository at `root`: it names no repositories, or names
/// this one.
pub fn applies(check: &Check, root: &Path) -> bool {
    check.repos.is_empty() || check.repos.iter().any(|r| same_dir(Path::new(r), root))
}

/// Whether `check` may run in `root` without being asked. The roots in the configuration are
/// compared as git reports them, resolved, so that a path written with a symlink or a trailing
/// slash still matches the repository it names.
pub fn watches(check: &Check, root: &Path) -> bool {
    applies(check, root) && check.watch.iter().any(|w| same_dir(Path::new(w), root))
}

/// The checks a repository already has, found from the files at its root, for a repository
/// nobody has configured anything for: its Makefile's `test` and `check` targets first, since
/// those are the project's own word for it, then its language's test runner.
///
/// Every command here is from this table, never from the repository — a `package.json` says
/// whether there is a test script, not what to type — and none of them runs until someone
/// presses its button, which is also when it is saved to the configuration. What a test
/// command runs is the repository's own code, and that is what running its tests means.
pub fn detect(root: &Path) -> Vec<Check> {
    let has = |name: &str| root.join(name).is_file();
    let read = |name: &str| std::fs::read_to_string(root.join(name)).unwrap_or_default();
    let mut found: Vec<Check> = Vec::new();
    let mut add = |run: &str| {
        found.push(Check {
            name: run.to_string(),
            run: run.to_string(),
            ..Check::default()
        })
    };

    let makefile = ["GNUmakefile", "makefile", "Makefile"]
        .into_iter()
        .find(|m| has(m))
        .map(read)
        .unwrap_or_default();
    for target in ["test", "check"] {
        let defines = makefile.lines().any(|line| {
            line.strip_prefix(target).is_some_and(|rest| {
                let rest = rest.trim_start();
                rest.starts_with(':') && !rest.starts_with(":=")
            })
        });
        if defines {
            add(&format!("make {target}"));
        }
    }

    if has("Cargo.toml") {
        let workspace = if read("Cargo.toml").contains("[workspace]") {
            " --workspace"
        } else {
            ""
        };
        if on_path("cargo-nextest") {
            add(&format!("cargo nextest run{workspace}"));
        } else {
            add(&format!("cargo test{workspace}"));
        }
    }
    if has("package.json") {
        let script = serde_json::from_str::<serde_json::Value>(&read("package.json"))
            .ok()
            .and_then(|p| p["scripts"]["test"].as_str().map(str::to_string));
        // `npm init` writes a test script whose whole job is to fail.
        if script.is_some_and(|s| !s.contains("no test specified")) {
            let runner = if has("pnpm-lock.yaml") {
                "pnpm test"
            } else if has("yarn.lock") {
                "yarn test"
            } else if has("bun.lock") || has("bun.lockb") {
                "bun run test"
            } else {
                "npm test"
            };
            add(runner);
        }
    }
    let pytest = has("pytest.ini")
        || has("conftest.py")
        || read("pyproject.toml").contains("pytest")
        || read("setup.cfg").contains("[tool:pytest]")
        || read("tox.ini").contains("[pytest]")
        || ((has("pyproject.toml") || has("setup.py")) && root.join("tests").is_dir());
    if pytest {
        add(if has("uv.lock") {
            "uv run pytest"
        } else if has("poetry.lock") {
            "poetry run pytest"
        } else {
            "python3 -m pytest"
        });
    }
    if has("go.mod") {
        add("go test ./...");
    }
    found
}

/// Whether `program` is an executable file in a directory on `PATH`.
fn on_path(program: &str) -> bool {
    let runnable = |m: std::fs::Metadata| {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            m.is_file() && m.permissions().mode() & 0o111 != 0
        }
        #[cfg(not(unix))]
        {
            m.is_file()
        }
    };
    std::env::var_os("PATH").is_some_and(|path| {
        std::env::split_paths(&path)
            .any(|dir| std::fs::metadata(dir.join(program)).is_ok_and(runnable))
    })
}

/// How far the code has moved on since a result: commits made after it, and files in the
/// working tree changed after it. A result with both at zero is about what is on disk now.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Staleness {
    pub commits: usize,
    pub files: usize,
}

/// The counts a test runner prints at the end, for a check that leaves no artefact behind:
/// `cargo test`'s `test result:` lines, added up across its binaries; nextest's `Summary`;
/// pytest's closing line; jest's and vitest's `Tests` line. The output is only the tail of what
/// was printed, so a `cargo test` with more binaries than fit is counted short — nextest and
/// the others print one total at the very end and are not.
pub fn figures_from_output(output: &str) -> Option<Figures> {
    static COUNT: OnceLock<Regex> = OnceLock::new();
    let count = COUNT.get_or_init(|| {
        Regex::new(r"(\d+) (passed|failed|skipped|ignored|errors?|xfailed|xpassed|todo)\b")
            .expect("regex")
    });
    let tally = |line: &str| {
        let (mut passed, mut failed, mut skipped) = (0u64, 0u64, 0u64);
        for c in count.captures_iter(line) {
            let n: u64 = c[1].parse().unwrap_or(0);
            match &c[2] {
                "passed" | "xpassed" => passed += n,
                "failed" | "error" | "errors" => failed += n,
                _ => skipped += n,
            }
        }
        (passed, failed, skipped)
    };
    let figures = |(passed, failed, skipped): (u64, u64, u64)| Figures {
        tests: Some(passed + failed),
        failed: Some(failed),
        skipped: Some(skipped),
        ..Figures::default()
    };

    let cargo: Vec<&str> = output
        .lines()
        .filter(|l| l.trim_start().starts_with("test result:"))
        .collect();
    let summary = output.lines().rev().find(|l| {
        let l = l.trim();
        (l.starts_with("Summary [") && l.contains(" run:"))
            || (l.starts_with('=') && l.ends_with('=') && l.contains(" in "))
            || l.starts_with("Tests:")
            || (l.starts_with("Tests ") && l.contains('('))
    });
    if let Some(line) = summary {
        // nextest and pytest spell `1 failed` inside a longer line; vitest writes `1 failed |`.
        let t = tally(line);
        if t != (0, 0, 0) {
            return Some(figures(t));
        }
    }
    if !cargo.is_empty() {
        let t = cargo
            .iter()
            .map(|l| tally(l))
            .fold((0, 0, 0), |a, b| (a.0 + b.0, a.1 + b.1, a.2 + b.2));
        return Some(figures(t));
    }
    None
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
            repos: Vec::new(),
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

    /// Each runner's own closing words, as it prints them.
    #[test]
    fn a_runner_says_how_many_tests_it_ran() {
        let counts = |out: &str| {
            figures_from_output(out)
                .map(|f| (f.tests.unwrap(), f.failed.unwrap(), f.skipped.unwrap()))
        };
        // cargo test, one line per binary, added up.
        assert_eq!(
            counts(
                "running 3 tests\ntest result: ok. 3 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out\n\
                 test result: FAILED. 5 passed; 2 failed; 0 ignored; 0 measured; 0 filtered out\n"
            ),
            Some((10, 2, 1))
        );
        // nextest's summary, including when `make check` is what ran it.
        assert_eq!(
            counts(
                "cargo fmt --check\n     Summary [   6.237s] 109 tests run: 107 passed, 2 failed, 2 skipped\n"
            ),
            Some((109, 2, 2))
        );
        // pytest.
        assert_eq!(
            counts(
                "tests/test_a.py ..F\n==== 1 failed, 10 passed, 1 skipped, 2 warnings in 0.12s ====\n"
            ),
            Some((11, 1, 1))
        );
        // jest, and vitest.
        assert_eq!(
            counts("Tests:       1 failed, 1 skipped, 10 passed, 12 total\n"),
            Some((11, 1, 1))
        );
        assert_eq!(
            counts("      Tests  1 failed | 9 passed (10)\n"),
            Some((10, 1, 0))
        );
        // A command that is not a test runner says nothing about tests.
        assert_eq!(counts("Finished `release` profile\n"), None);
    }

    #[test]
    fn a_repository_says_how_to_test_it() {
        let dir = tempfile::tempdir().unwrap();
        let names = |d: &Path| detect(d).into_iter().map(|c| c.run).collect::<Vec<_>>();
        assert!(
            names(dir.path()).is_empty(),
            "nothing there, nothing offered"
        );

        // The Makefile's own targets come first; `check:=` is a variable, not a target.
        std::fs::write(
            dir.path().join("Makefile"),
            "check := yes\ntest: build\n\tcargo test\nlint:\n",
        )
        .unwrap();
        std::fs::write(dir.path().join("Cargo.toml"), "[package]\nname = \"x\"\n").unwrap();
        std::fs::write(dir.path().join("go.mod"), "module x\n").unwrap();
        let found = names(dir.path());
        assert_eq!(found[0], "make test", "{found:?}");
        assert!(!found.contains(&"make check".to_string()), "{found:?}");
        assert!(found[1].starts_with("cargo "), "{found:?}");
        assert_eq!(found[2], "go test ./...");

        // npm's placeholder test script is not a test suite.
        let js = tempfile::tempdir().unwrap();
        std::fs::write(
            js.path().join("package.json"),
            r#"{"scripts": {"test": "echo \"Error: no test specified\" && exit 1"}}"#,
        )
        .unwrap();
        assert!(names(js.path()).is_empty());
        std::fs::write(
            js.path().join("package.json"),
            r#"{"scripts": {"test": "vitest run"}}"#,
        )
        .unwrap();
        std::fs::write(js.path().join("pnpm-lock.yaml"), "").unwrap();
        assert_eq!(names(js.path()), vec!["pnpm test"]);

        let py = tempfile::tempdir().unwrap();
        std::fs::write(
            py.path().join("pyproject.toml"),
            "[tool.pytest.ini_options]\n",
        )
        .unwrap();
        std::fs::write(py.path().join("uv.lock"), "").unwrap();
        assert_eq!(names(py.path()), vec!["uv run pytest"]);
    }

    #[test]
    fn a_check_found_in_one_repository_stays_there() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        let everywhere = check("true", "", "");
        assert!(applies(&everywhere, a.path()) && applies(&everywhere, b.path()));
        let mut scoped = check("true", "", "");
        scoped.repos = vec![format!("{}/", a.path().display())];
        assert!(
            applies(&scoped, a.path()),
            "a trailing slash is the same directory"
        );
        assert!(!applies(&scoped, b.path()));
        // Watching a repository the check does not belong to does not make it run there.
        scoped.watch = vec![b.path().display().to_string()];
        assert!(!watches(&scoped, b.path()));
    }
}
