//! The end-to-end tests: what a repository opens on.
//!
//! When most of the code was written by an agent, what is worth a reviewer's time is what the
//! software demonstrably does, and the one place that is both written down and checked by a
//! machine is a test that drives it the way a person does — a browser, a terminal, the
//! command line — and looks at what comes back. So a repository opens on those tests, each
//! with the recording its last run left behind, and everything else in the tool is reached
//! from there.
//!
//! A test is end to end when its path says so (the rules in `spec`), when its file drives
//! the software from outside — a browser driver, a pseudo-terminal, the built binary — or when
//! a recording names it, which settles it. The test that only hands another file's tests to a
//! runner is named by their recordings as their runner, and is left out: it is not a
//! behaviour of its own.
//!
//! A repository with none is told so before anything else, with what one would look like for
//! the kind of software it seems to be, and the agent offered the job of writing the first.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::analysis::Staleness;
use crate::recording::Recording;
use crate::session::Session;
use crate::spec::Level;
use crate::testsuite::TestCase;

/// What in a test file shows that it drives the software from outside, and what it drives.
/// Spelled as the code spells them: these are the libraries' and the build tool's own names.
const DRIVERS: &[(&str, &str)] = &[
    ("puppeteer", "a browser"),
    ("playwright", "a browser"),
    ("selenium", "a browser"),
    ("webdriver", "a browser"),
    ("cypress", "a browser"),
    ("chromedp", "a browser"),
    ("fantoccini", "a browser"),
    ("thirtyfour", "a browser"),
    ("headless_chrome", "a browser"),
    ("portable_pty", "a terminal"),
    ("expectrl", "a terminal"),
    ("rexpect", "a terminal"),
    ("pexpect", "a terminal"),
    ("node-pty", "a terminal"),
    ("CARGO_BIN_EXE_", "the built program"),
    ("assert_cmd", "the built program"),
];

/// Whether all of `path` is test code, by where it is and what it is called: a test that
/// drives the software from outside lives in a file of its own. Code with its unit tests
/// beside it is not one, whatever it mentions — a module that recognises puppeteer names it.
pub fn test_only(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    let (dirs, name) = lower.rsplit_once('/').unwrap_or(("", &lower));
    let dirs: Vec<&str> = dirs.split('/').collect();
    dirs.iter().any(|d| {
        matches!(
            *d,
            "tests"
                | "test"
                | "e2e"
                | "end_to_end"
                | "end-to-end"
                | "endtoend"
                | "__tests__"
                | "spec"
                | "specs"
                | "integration"
                | "functional"
        )
    }) || name.contains(".test.")
        || name.contains(".spec.")
        || name.contains(".e2e.")
        || name.ends_with("_test.go")
        || name.ends_with("_test.py")
        || name.starts_with("test_")
}

/// What `text`, a test file, drives, if it drives anything.
pub fn drives(text: &str) -> Option<&'static str> {
    DRIVERS
        .iter()
        .find(|(needle, _)| text.contains(needle))
        .map(|(_, what)| *what)
}

/// The text of a test file and of the files it pulls in by path: a Rust test that keeps its
/// harness beside it with `#[path = "..."]` drives what the harness drives.
fn test_file_text(root: &Path, path: &str) -> String {
    let mut text = std::fs::read_to_string(root.join(path)).unwrap_or_default();
    let dir = Path::new(path).parent().unwrap_or(Path::new(""));
    let included: Vec<String> = text
        .lines()
        .filter_map(|l| {
            let rest = l.trim().strip_prefix("#[path")?;
            let start = rest.find('"')? + 1;
            let end = start + rest[start..].find('"')?;
            Some(rest[start..end].to_string())
        })
        .filter(|p| !p.contains("..") && !p.starts_with('/'))
        .collect();
    for p in included {
        if let Ok(more) = std::fs::read_to_string(root.join(dir).join(p)) {
            text.push_str(&more);
        }
    }
    text
}

/// What each test file drives, read once per file.
struct Drivers<'a> {
    root: &'a Path,
    seen: HashMap<String, Option<&'static str>>,
}

impl<'a> Drivers<'a> {
    fn new(root: &'a Path) -> Self {
        Self {
            root,
            seen: HashMap::new(),
        }
    }

    /// Why `test` is end to end, if it is: its path, what its file drives, or that it left a
    /// recording, in that order.
    fn why(&mut self, test: &TestCase, recorded: bool) -> Option<String> {
        let (level, level_why) = crate::spec::level(test);
        if level == Level::EndToEnd {
            return Some(level_why);
        }
        let root = self.root;
        let driven = *self.seen.entry(test.path.clone()).or_insert_with(|| {
            test_only(&test.path)
                .then(|| drives(&test_file_text(root, &test.path)))
                .flatten()
        });
        match (driven, recorded) {
            (Some(what), _) => Some(format!("it drives {what}")),
            (None, true) => Some("it left a recording".to_string()),
            (None, false) => None,
        }
    }
}

/// Moves the tests that drive the software, or left a recording of doing so, to the end-to-end
/// level of the specification, so that it and the opening agree on what is end to end. A
/// security test stays where it is: there, security is the heading a reviewer reads first.
pub fn promote(root: &Path, tests: &mut [crate::spec::SpecTest]) {
    let recordings = crate::recording::find(root);
    let mut drivers = Drivers::new(root);
    for t in tests {
        if matches!(t.level, Level::Security | Level::EndToEnd) {
            continue;
        }
        let recorded = recordings.iter().any(|r| names(r, &t.test));
        if let Some(why) = drivers.why(&t.test, recorded) {
            t.level = Level::EndToEnd;
            t.why = why;
        }
    }
}

/// One end-to-end test, with its last recording.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EndToEnd {
    #[serde(flatten)]
    pub test: TestCase,
    /// Why it counts as end to end, in a few words.
    pub why: String,
    /// Whether it is about security as well: a mark, at any level.
    pub security: bool,
    pub recording: Option<Recording>,
    /// How far the code has moved on since the recording was made.
    pub stale: Option<Staleness>,
}

impl EndToEnd {
    /// Whether its last run failed.
    pub fn failed(&self) -> bool {
        self.recording.as_ref().is_some_and(|r| !r.passed)
    }
}

/// What a repository opens on.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Opening {
    /// The end-to-end tests, by file and line.
    pub tests: Vec<EndToEnd>,
    /// Recordings whose test is not there any more: renamed, moved or deleted since.
    pub orphans: Vec<Recording>,
    /// Every test of the repository, of any level.
    pub total: usize,
    pub failed: usize,
    /// End-to-end tests with no recording to show.
    pub unrecorded: usize,
    /// What the software looks like it is, for saying what an end-to-end test of it would
    /// do: `a web page`, `a terminal program`, `a command`; `None` for what looks like a
    /// library, which has nobody driving it.
    pub kind: Option<String>,
    /// What the agent is asked when there are none.
    pub ask: String,
}

impl Opening {
    /// Whether the repository has no end-to-end tests at all: the first finding.
    pub fn missing(&self) -> bool {
        self.tests.is_empty()
    }
}

/// Whether recording `r` is of `test`.
fn names(r: &Recording, test: &TestCase) -> bool {
    r.file == test.path
        && (r.test == test.name
            || r.test == test.label
            || crate::testsuite::phrase(&r.test) == test.label)
}

/// The end-to-end tests among `tests`, the repository's tests, with their recordings.
pub fn of(session: &Session, tests: Vec<TestCase>) -> Opening {
    let root = session.root();
    let recordings = crate::recording::find(root);
    let total = tests.len();
    let runners: Vec<(&str, &str)> = recordings
        .iter()
        .filter_map(|r| r.run_by.as_ref())
        .map(|b| (b.file.as_str(), b.test.as_str()))
        .collect();
    let mut drivers = Drivers::new(root);
    let mut matched = vec![false; recordings.len()];
    let mut out = Vec::new();
    for test in tests {
        if runners
            .iter()
            .any(|(f, t)| *f == test.path && (*t == test.name || *t == test.label))
        {
            continue;
        }
        let at = recordings.iter().position(|r| names(r, &test));
        let (level, _) = crate::spec::level(&test);
        let Some(why) = drivers.why(&test, at.is_some()) else {
            continue;
        };
        let recording = at.map(|i| {
            matched[i] = true;
            recordings[i].clone()
        });
        out.push(EndToEnd {
            security: level == Level::Security,
            test,
            why,
            recording,
            stale: None,
        });
    }
    let whens: Vec<Option<&str>> = out
        .iter()
        .map(|t| t.recording.as_ref().and_then(|r| r.started.as_deref()))
        .collect();
    let stale = session.changed_since_each(&whens);
    for (t, s) in out.iter_mut().zip(stale) {
        t.stale = s;
    }
    out.sort_by(|a, b| (&a.test.path, a.test.line).cmp(&(&b.test.path, b.test.line)));
    let orphans = recordings
        .into_iter()
        .zip(matched)
        .filter(|(_, m)| !m)
        .map(|(r, _)| r)
        .collect();
    let kind = kind(session);
    Opening {
        failed: out.iter().filter(|t| t.failed()).count(),
        unrecorded: out.iter().filter(|t| t.recording.is_none()).count(),
        tests: out,
        orphans,
        total,
        ask: crate::prompts::first_end_to_end(kind.as_deref()),
        kind,
    }
}

/// What kind of software the repository looks like, from its manifests and its files.
pub fn kind(session: &Session) -> Option<String> {
    let root = session.root();
    let read = |name: &str| std::fs::read_to_string(root.join(name)).unwrap_or_default();
    let manifests = [
        read("Cargo.toml"),
        read("package.json"),
        read("pyproject.toml"),
        read("requirements.txt"),
        read("go.mod"),
    ]
    .join("\n");
    let has = |words: &[&str]| words.iter().any(|w| manifests.contains(w));
    let pages = session.files.iter().any(|f| {
        (f.ends_with(".html")
            || f.ends_with(".vue")
            || f.ends_with(".svelte")
            || f.ends_with(".tsx"))
            && !f.starts_with("docs/")
            && !f.starts_with("tests/")
    });
    if pages
        || has(&[
            "\"react\"",
            "\"vue\"",
            "\"svelte\"",
            "\"next\"",
            "axum",
            "actix-web",
            "flask",
            "django",
            "fastapi",
            "express",
        ])
    {
        return Some("a web page".into());
    }
    if has(&[
        "ratatui",
        "crossterm",
        "cursive",
        "textual",
        "curses",
        "\"ink\"",
        "blessed",
        "bubbletea",
    ]) {
        return Some("a terminal program".into());
    }
    let entry = session.files.iter().any(|f| {
        f == "src/main.rs"
            || f.starts_with("src/bin/")
            || f.ends_with("__main__.py")
            || f.starts_with("cmd/")
            || f == "main.go"
    }) || has(&["\"bin\"", "[project.scripts]", "console_scripts"]);
    entry.then(|| "a command".into())
}

/// How many end-to-end tests each file has, for a heading.
pub fn by_file(tests: &[EndToEnd]) -> BTreeMap<&str, usize> {
    let mut out = BTreeMap::new();
    for t in tests {
        *out.entry(t.test.path.as_str()).or_insert(0) += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::tests::scratch_repo;

    fn case(path: &str, name: &str, line: usize) -> TestCase {
        TestCase {
            name: name.into(),
            label: crate::testsuite::phrase(name),
            path: path.into(),
            line,
            end: line,
            ignored: false,
            checks: 1,
            shape: 0,
        }
    }

    fn put(root: &Path, path: &str, text: &str) {
        let path = root.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    #[test]
    fn a_test_that_drives_the_software_is_end_to_end() {
        assert_eq!(drives("const p = require('puppeteer');"), Some("a browser"));
        assert_eq!(
            drives("use portable_pty::native_pty_system;"),
            Some("a terminal")
        );
        assert_eq!(
            drives("Command::new(env!(\"CARGO_BIN_EXE_tool\"))"),
            Some("the built program")
        );
        assert_eq!(drives("assert_eq!(parse(\"x\"), 1);"), None);
        // Only a file that is all tests can be one: a module naming the drivers is not.
        assert!(test_only("tests/terminal.rs"));
        assert!(test_only("web/e2e/login.ts"));
        assert!(test_only("src/review.spec.ts"));
        assert!(test_only("pkg/server_test.go"));
        assert!(!test_only("src/e2e.rs"));
        assert!(!test_only("src/web/api.rs"));
    }

    #[test]
    fn a_repository_opens_on_its_end_to_end_tests_and_their_recordings() {
        let dir = scratch_repo();
        let root = dir.path();
        put(
            root,
            "tests/cli.rs",
            "#[path = \"cli/harness.rs\"]\nmod harness;\n",
        );
        put(root, "tests/cli/harness.rs", "use portable_pty::PtySize;\n");
        put(
            root,
            "tests/runner.rs",
            "// runs page.js in a browser: puppeteer\n",
        );
        put(
            root,
            "target/e2e/terminal/opens/test.json",
            r#"{"test": "a_file_opens", "file": "tests/cli.rs", "line": 3, "passed": false,
                "started": "2000-01-01T00:00:00Z", "steps": []}"#,
        );
        put(
            root,
            "target/e2e/browser/page/test.json",
            r#"{"test": "the page loads", "file": "tests/page.js", "line": 1, "passed": true,
                "run_by": {"file": "tests/runner.rs", "test": "every_page_test_passes"}}"#,
        );
        put(
            root,
            "target/e2e/browser/gone/test.json",
            r#"{"test": "a test since deleted", "file": "tests/page.js", "passed": true}"#,
        );
        let session = Session::open(root).unwrap();
        let tests = vec![
            case("src/lib.rs", "parses", 1),
            case("tests/cli.rs", "a_file_opens", 3),
            case("tests/cli.rs", "a_comment_is_saved", 9),
            case("tests/page.js", "the page loads", 1),
            case("tests/runner.rs", "every_page_test_passes", 2),
        ];
        let opening = of(&session, tests);
        let names: Vec<(&str, &str)> = opening
            .tests
            .iter()
            .map(|t| (t.test.name.as_str(), t.why.as_str()))
            .collect();
        assert_eq!(
            names,
            [
                ("a_file_opens", "it drives a terminal"),
                ("a_comment_is_saved", "it drives a terminal"),
                ("the page loads", "it left a recording"),
            ],
            "the unit test is not one, and the runner stands in for nothing"
        );
        assert_eq!(opening.total, 5);
        assert_eq!((opening.failed, opening.unrecorded), (1, 1));
        let first = &opening.tests[0];
        assert!(first.failed());
        assert!(
            first.stale.is_some_and(|s| s.commits >= 1),
            "a recording from 2000 is behind every commit: {:?}",
            first.stale
        );
        let orphans: Vec<&str> = opening.orphans.iter().map(|r| r.test.as_str()).collect();
        assert_eq!(orphans, ["a test since deleted"]);
        assert!(!opening.missing());
    }

    #[test]
    fn a_repository_without_any_says_so_and_what_to_ask_for() {
        let dir = scratch_repo();
        let session = Session::open(dir.path()).unwrap();
        let opening = of(&session, vec![case("src/lib.rs", "parses", 1)]);
        assert!(opening.missing());
        assert_eq!(opening.total, 1);
        assert!(opening.ask.contains("test.json"), "{}", opening.ask);
    }
}
