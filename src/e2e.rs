//! The end-to-end tests: what a repository opens on.
//!
//! When most of the code was written by an agent, what is worth a reviewer's time is what the
//! software demonstrably does, and the one place that is both written down and checked by a
//! machine is a test that drives it the way a person does — a browser, a terminal, the
//! command line — and looks at what comes back. So a repository opens on those tests, each
//! with the recording its last run left behind, and everything else in the tool is reached
//! from there.
//!
//! A test is end to end when it runs the program and drives it through the interface a person
//! uses (the `classify` module's Dynamic and GUI): when its path says so, when its file drives
//! the software from outside — a browser driver, a pseudo-terminal, the built binary — or when
//! a recording names it, which settles it. The test that only hands another file's tests to a
//! runner is named by their recordings as their runner, and is left out: it is not a
//! behaviour of its own.
//!
//! A repository with none is told so before anything else, with what one would look like for
//! the kind of software it seems to be, and the agent offered the job of writing the first.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::analysis::Staleness;
use crate::recording::Recording;
use crate::session::Session;
use crate::testsuite::TestCase;

/// Words in a test's path that make it end to end: the drivers and the directory names those
/// suites conventionally live in.
const END_TO_END_WORDS: &[&str] = &[
    "e2e",
    "browser",
    "playwright",
    "cypress",
    "selenium",
    "puppeteer",
    "webdriver",
    "headless",
];

/// Why a test's path says it is end to end, if it does. Only a file of nothing but tests can:
/// `src/browser.rs` is a module, and the tests beside its code are not end to end whatever it
/// is called.
pub fn path_says(test: &TestCase) -> Option<String> {
    if !test_only(&test.path) {
        return None;
    }
    let joined = test.path.to_lowercase();
    if ["end_to_end", "end-to-end", "endtoend"]
        .iter()
        .any(|w| joined.contains(w))
    {
        return Some("its path says end to end".into());
    }
    // Any word of a directory, but of the file's own name only the first or a dotted part:
    // `browser.rs` and `login.e2e.ts` say it, a fixture named `css_playwright_selector.rs`
    // does not.
    let (dirs, name) = test.path.rsplit_once('/').unwrap_or(("", &test.path));
    let mut parts = name.split('.');
    let lead = parts.next().map(crate::spec::words).unwrap_or_default();
    crate::spec::words(dirs)
        .into_iter()
        .chain(lead.into_iter().take(1))
        .chain(parts.map(str::to_lowercase))
        .find(|w| END_TO_END_WORDS.contains(&w.as_str()))
        .map(|w| format!("its path says {w}"))
}

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
pub(crate) struct Drivers<'a> {
    root: &'a Path,
    seen: HashMap<String, Option<&'static str>>,
}

impl<'a> Drivers<'a> {
    pub(crate) fn new(root: &'a Path) -> Self {
        Self {
            root,
            seen: HashMap::new(),
        }
    }

    /// Why `test` is end to end, if it is: its path, what its file drives, or that it left a
    /// recording, in that order.
    pub(crate) fn why(&mut self, test: &TestCase, recorded: bool) -> Option<String> {
        if let Some(why) = path_says(test) {
            return Some(why);
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
pub(crate) fn names(r: &Recording, test: &TestCase) -> bool {
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
        let security = crate::spec::security(&test).is_some();
        let Some(why) = drivers.why(&test, at.is_some()) else {
            continue;
        };
        let recording = at.map(|i| {
            matched[i] = true;
            recordings[i].clone()
        });
        out.push(EndToEnd {
            security,
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

/// A file an end-to-end test ran, and how much of it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RanFile {
    pub path: String,
    /// Lines it ran.
    pub lines: usize,
    /// Of those, the lines no other end-to-end test ran: what this test alone shows working.
    pub only_here: usize,
    /// The lines it ran, as first and last of each run of them.
    pub ranges: Vec<(u32, u32)>,
}

/// A smaller test that reaches code the end-to-end test ran.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Smaller {
    #[serde(flatten)]
    pub test: crate::spec::SpecTest,
    /// The files it shares with the end-to-end test.
    pub shared: Vec<String>,
}

/// One end-to-end test as its own view shows it: where a reviewer goes into the code and the
/// documentation from.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Detail {
    #[serde(flatten)]
    pub test: EndToEnd,
    /// For each of the recording's steps, the line of the test that made it.
    pub sources: Vec<Option<String>>,
    /// The code its run executed, by what only it ran and then by how much; `None` when the
    /// run left no coverage.
    pub ran: Option<Vec<RanFile>>,
    pub docs: Vec<crate::docs::DocLink>,
    pub smaller: Vec<Smaller>,
}

/// At most this many files are listed as what a test ran, and smaller tests below it.
const MOST_RAN: usize = 60;
const MOST_SMALLER: usize = 20;

/// The end-to-end test in `file` called `name`, with what it ran, what it documents and the
/// tests below it. `None` when there is no such end-to-end test.
pub fn detail(session: &mut Session, file: &str, name: &str) -> Option<Detail> {
    let opening = session.end_to_end();
    let test = opening
        .tests
        .iter()
        .find(|t| t.test.path == file && (t.test.name == name || t.test.label == name))?
        .clone();
    let root = session.root().to_path_buf();

    let source: Vec<String> = std::fs::read_to_string(root.join(file))
        .unwrap_or_default()
        .lines()
        .map(str::to_string)
        .collect();
    let sources = test
        .recording
        .iter()
        .flat_map(|r| &r.steps)
        .map(|s| {
            s.line
                .checked_sub(1)
                .and_then(|i| source.get(i))
                .map(|l| l.trim().to_string())
        })
        .collect();

    // What it ran, against what every other end-to-end test ran.
    let ran_by = |t: &EndToEnd| {
        t.recording
            .as_ref()
            .and_then(|r| crate::recording::lines_run(&root, r))
    };
    let mine = ran_by(&test).map(|m| {
        m.into_iter()
            .filter(|(path, _)| !test_only(path) && session.files.binary_search(path).is_ok())
            .collect::<BTreeMap<_, _>>()
    });
    let ran = mine.as_ref().map(|mine| {
        let others: Vec<BTreeMap<String, Vec<u32>>> = opening
            .tests
            .iter()
            .filter(|t| !(t.test.path == test.test.path && t.test.name == test.test.name))
            .filter_map(ran_by)
            .collect();
        let mut files: Vec<RanFile> = mine
            .iter()
            .map(|(path, lines)| {
                let elsewhere: HashSet<u32> = others
                    .iter()
                    .filter_map(|o| o.get(path))
                    .flatten()
                    .copied()
                    .collect();
                RanFile {
                    path: path.clone(),
                    lines: lines.len(),
                    only_here: lines.iter().filter(|l| !elsewhere.contains(l)).count(),
                    ranges: crate::recording::ranges(lines),
                }
            })
            .collect();
        files.sort_by(|a, b| (b.only_here, b.lines, &a.path).cmp(&(a.only_here, a.lines, &b.path)));
        files.truncate(MOST_RAN);
        files
    });

    // What the test says it shows is its sentence. Its steps are mostly how it gets there —
    // open, leave, go back — which every test shares, and they would pull every test towards
    // the same long sections.
    let words = unquoted(&test.test.label);
    let declared = test
        .recording
        .as_ref()
        .map(|r| r.docs.clone())
        .unwrap_or_default();
    let docs = crate::docs::for_test(&root, &session.files, &declared, &words, 3);

    // The smaller tests: those that name a file it ran, or live in one.
    let smaller = match &mine {
        None => Vec::new(),
        Some(mine) => {
            let tests: Vec<TestCase> = session
                .tests(0)
                .files
                .into_iter()
                .flat_map(|f| f.tests)
                .collect();
            let coverage = session.coverage();
            let annotated = crate::spec::annotate(&root, tests, Some(&coverage));
            let mut found: Vec<Smaller> = annotated
                .into_iter()
                .filter(|t| !t.end_to_end)
                .filter_map(|t| {
                    let mut shared: Vec<String> = t
                        .reaches
                        .iter()
                        .chain(std::iter::once(&t.test.path))
                        .filter(|p| mine.contains_key(*p))
                        .cloned()
                        .collect();
                    shared.sort();
                    shared.dedup();
                    (!shared.is_empty()).then_some(Smaller { test: t, shared })
                })
                .collect();
            // A file counts for more the higher it is in what the test ran, which puts the
            // code only this test shows working first: a unit test of that is worth more than
            // one of the plumbing every test goes through.
            let rank: HashMap<&str, usize> = ran
                .iter()
                .flatten()
                .enumerate()
                .map(|(i, f)| (f.path.as_str(), i))
                .collect();
            let weight = |t: &Smaller| -> f64 {
                t.shared
                    .iter()
                    .map(|p| 1.0 / (1 + rank.get(p.as_str()).copied().unwrap_or(MOST_RAN)) as f64)
                    .sum()
            };
            found.sort_by(|a, b| {
                weight(b).total_cmp(&weight(a)).then(
                    (&a.test.test.path, a.test.test.line)
                        .cmp(&(&b.test.test.path, b.test.test.line)),
                )
            });
            found.truncate(MOST_SMALLER);
            found
        }
    };

    Some(Detail {
        test,
        sources,
        ran,
        docs,
        smaller,
    })
}

/// `text` without what it quotes, and without words spelled like paths or file names.
fn unquoted(text: &str) -> String {
    let mut out = String::new();
    let mut quoted = false;
    for c in text.chars() {
        match c {
            '“' => quoted = true,
            '”' => quoted = false,
            '"' | '`' => quoted = !quoted,
            _ if !quoted => out.push(c),
            _ => {}
        }
    }
    out.split_whitespace()
        .filter(|w| !w.contains(['/', '.', '_', ':']))
        .collect::<Vec<_>>()
        .join(" ")
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
    fn a_test_leads_to_the_code_it_ran_its_documentation_and_the_tests_below_it() {
        let dir = scratch_repo();
        let root = dir.path();
        put(
            root,
            "src/review.rs",
            "pub fn save_review_file() {}\npub fn load_review_file() {}\npub fn other() {}\n",
        );
        put(root, "src/page.rs", "pub fn draw() {}\n");
        put(
            root,
            "src/unit.rs",
            "#[test]\nfn save_works() { crate::review::save_review_file(); }\n",
        );
        put(
            root,
            "README.md",
            "# tool\n\n## Saving a review\n\nThe review is saved to disk and loaded again.\n\n\
             ## Drawing\n\nPages are drawn.\n",
        );
        put(
            root,
            "tests/cli.rs",
            "// CARGO_BIN_EXE_tool\n#[test]\nfn a_review_is_saved() {\n    run(\"save\");\n}\n\
             #[test]\nfn a_page_is_drawn() {\n    run(\"draw\");\n}\n",
        );
        let record = |slug: &str, name: &str, steps: &str, lcov: &str| {
            put(
                root,
                &format!("target/e2e/t/{slug}/test.json"),
                &format!(
                    r#"{{"test": "{name}", "file": "tests/cli.rs", "passed": true,
                        "steps": {steps}}}"#
                ),
            );
            put(root, &format!("target/e2e/t/{slug}/coverage.lcov"), lcov);
        };
        record(
            "saved",
            "a_review_is_saved",
            r#"[{"at_ms": 0, "line": 4, "kind": "action", "text": "save the review"}]"#,
            &format!(
                "SF:{}/src/review.rs\nDA:1,1\nDA:2,1\nDA:3,0\nend_of_record\n\
                 SF:src/page.rs\nDA:1,1\nend_of_record\nSF:tests/cli.rs\nDA:4,1\nend_of_record\n",
                root.display()
            ),
        );
        record(
            "drawn",
            "a_page_is_drawn",
            "[]",
            "SF:src/page.rs\nDA:1,1\nend_of_record\n",
        );
        git(root, &["add", "-A"]);
        let mut session = Session::open(root).unwrap();
        let d = detail(&mut session, "tests/cli.rs", "a_review_is_saved").unwrap();
        assert_eq!(d.sources, [Some("run(\"save\");".to_string())]);
        let ran: Vec<(&str, usize, usize)> = d
            .ran
            .as_ref()
            .unwrap()
            .iter()
            .map(|f| (f.path.as_str(), f.lines, f.only_here))
            .collect();
        assert_eq!(
            ran,
            [("src/review.rs", 2, 2), ("src/page.rs", 1, 0)],
            "what only it ran first; the test's own file is not code it ran"
        );
        assert_eq!(d.ran.as_ref().unwrap()[0].ranges, [(1, 2)]);
        assert_eq!(d.docs[0].section.heading, "Saving a review", "{:?}", d.docs);
        let smaller: Vec<&str> = d
            .smaller
            .iter()
            .map(|s| s.test.test.name.as_str())
            .collect();
        assert_eq!(smaller, ["save_works"]);
        assert_eq!(d.smaller[0].shared, ["src/review.rs"]);
        assert!(detail(&mut session, "tests/cli.rs", "no_such_test").is_none());
    }

    fn git(root: &Path, args: &[&str]) {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(root)
            .args(args)
            .output()
            .unwrap();
        assert!(out.status.success());
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
