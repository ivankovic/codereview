//! The tests as a specification: what kind of test each one is, and what a stretch of history
//! did to them.
//!
//! When an agent wrote the code, its tests are the one place where what the system is meant to
//! do is written down and checked by a machine, so a reviewer starts there rather than in the
//! code. Two things make that list worth more than a count. The first is what kind of test
//! each one is: an end-to-end test that drives a browser says far more about whether the thing
//! works than a hundred tests of its helpers, a test that reads the documentation says
//! something none of them do, and a test that mocks everything it touches says less than it
//! looks. The kinds are the eight questions of the `classify` module, and the list is grouped
//! by whichever one the reviewer is asking. A test about security is marked as such, whatever
//! its kind. The second is what happened to the tests lately. An agent that cannot make a test
//! pass is tempted to make it stop asking — delete it, skip it, or loosen what it checks — and
//! each of those is a line or two in a diff nobody reads closely. Here they come first.
//!
//! What happened to the tests is found by parsing every test file the stretch touched, as it
//! was and as it is, and matching the tests up: by file and name, then by name alone for a test
//! that moved file, then by body for one that was renamed.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;
use std::time::{Duration, SystemTime};

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

use crate::classify::{Kind, Question};
use crate::session::Session;
use crate::testsuite::{TestCase, testable};

/// Words that make a test a security test wherever they appear in its name or its path.
/// Whole words only: `tokenizer` is not about tokens, and `token` itself is left out because
/// a lexer has as many of them as a sign-in does.
const SECURITY_WORDS: &[&str] = &[
    "security",
    "secure",
    "insecure",
    "auth",
    "authn",
    "authz",
    "authentication",
    "authorization",
    "authorisation",
    "unauthorized",
    "unauthorised",
    "forbidden",
    "password",
    "passwords",
    "passwd",
    "csrf",
    "xss",
    "ssrf",
    "injection",
    "traversal",
    "secret",
    "secrets",
    "permission",
    "permissions",
    "privilege",
    "privileges",
    "sandbox",
    "csp",
    "login",
    "signin",
    "credential",
    "credentials",
    "cookie",
    "cookies",
    "jwt",
    "oauth",
    "sanitize",
    "sanitise",
    "vulnerability",
    "exploit",
];

/// The words of `text`, lowercase: split at anything that is not a letter or a digit, and
/// at a change from lower case to upper case, so `signIn` and `sign_in` both give `sign`, `in`.
pub(crate) fn words(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut word = String::new();
    let mut previous_lower = false;
    for c in text.chars() {
        if !c.is_alphanumeric() {
            if !word.is_empty() {
                out.push(std::mem::take(&mut word).to_lowercase());
            }
            previous_lower = false;
            continue;
        }
        if c.is_uppercase() && previous_lower && !word.is_empty() {
            out.push(std::mem::take(&mut word).to_lowercase());
        }
        previous_lower = c.is_lowercase() || c.is_ascii_digit();
        word.push(c);
    }
    if !word.is_empty() {
        out.push(word.to_lowercase());
    }
    out
}

/// Why a test is about security, if it is: a word of its name, or of its path, from
/// [`SECURITY_WORDS`]. Security is a purpose rather than a kind, so it is a mark on a test of
/// any kind.
pub fn security(test: &TestCase) -> Option<String> {
    let pairs = |w: &[String]| {
        w.windows(2)
            .map(|p| format!("{}{}", p[0], p[1]))
            .collect::<Vec<_>>()
    };
    // "sign in" and "log in" are two words in a phrase and one in the vocabulary.
    let find = |w: Vec<String>| {
        let joined = pairs(&w);
        w.into_iter()
            .chain(joined)
            .find(|w| SECURITY_WORDS.contains(&w.as_str()))
    };
    if let Some(word) = find(words(&format!("{} {}", test.name, test.label))) {
        return Some(format!("its name says {word}"));
    }
    find(words(&test.path)).map(|word| format!("its path says {word}"))
}

/// What happened to a test, worst first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum What {
    /// Gone, with nothing that looks like it in its place.
    Removed,
    /// Still there, but marked to be skipped.
    Skipped,
    /// Still there, making fewer assertions than it did.
    Weakened,
    /// Its body changed.
    Changed,
    /// Its name or its file changed and its body did not.
    Renamed,
    /// No longer skipped.
    Unskipped,
    Added,
}

impl What {
    /// Whether this is something a reviewer should look at before anything else: a test
    /// that asks less than it did.
    pub fn alarming(self) -> bool {
        matches!(self, What::Removed | What::Skipped | What::Weakened)
    }

    pub fn word(self) -> &'static str {
        match self {
            What::Removed => "removed",
            What::Skipped => "skipped",
            What::Weakened => "weakened",
            What::Changed => "changed",
            What::Renamed => "renamed",
            What::Unskipped => "unskipped",
            What::Added => "added",
        }
    }
}

/// One test that a stretch of history did something to.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestChange {
    pub what: What,
    /// Whether it is about security, by its name or path.
    #[serde(default)]
    pub security: bool,
    /// What the test says it checks, as it is now; as it was, for a removed one.
    pub label: String,
    /// Where it is now, or where it was, for a removed one.
    pub path: String,
    /// 1-based, in the file as it is now, or as it was for a removed one.
    pub line: usize,
    /// Where it was and what it was called, when either is not what it is now.
    pub was: Option<String>,
    /// Assertions before and after, where both sides have the test.
    pub checks: Option<(usize, usize)>,
}

impl TestChange {
    /// What to mark it with beside its name: `security`, or nothing.
    pub fn tag(&self) -> Option<&'static str> {
        self.security.then_some("security")
    }
}

/// Matches the tests of `before` with those of `after` and says what happened to each that
/// changed. Unchanged tests are left out.
pub fn compare(before: Vec<TestCase>, after: Vec<TestCase>) -> Vec<TestChange> {
    let mut before: Vec<Option<TestCase>> = before.into_iter().map(Some).collect();
    let mut after: Vec<Option<TestCase>> = after.into_iter().map(Some).collect();
    let mut out = Vec::new();

    // Three passes, each over what the last left unmatched: the same file and label, then the
    // same label anywhere (the file moved), then the same body (the test was renamed).
    type Key = fn(&TestCase) -> Option<String>;
    let passes: [Key; 3] = [
        |t| Some(format!("{}\u{0}{}", t.path, t.label)),
        |t| Some(t.label.clone()),
        |t| (t.shape != 0).then(|| t.shape.to_string()),
    ];
    for key in passes {
        let mut waiting: HashMap<String, Vec<usize>> = HashMap::new();
        for (i, t) in before.iter().enumerate() {
            if let Some(k) = t.as_ref().and_then(key) {
                waiting.entry(k).or_default().push(i);
            }
        }
        for slot in after.iter_mut() {
            let Some(k) = slot.as_ref().and_then(key) else {
                continue;
            };
            let Some(i) = waiting.get_mut(&k).and_then(|v| {
                if v.is_empty() {
                    None
                } else {
                    Some(v.remove(0))
                }
            }) else {
                continue;
            };
            let old = before[i].take().expect("matched once");
            let new = slot.take().expect("matched once");
            if let Some(change) = between(&old, new) {
                out.push(change);
            }
        }
    }
    for old in before.into_iter().flatten() {
        out.push(TestChange {
            what: What::Removed,
            security: security(&old).is_some(),
            label: old.label,
            path: old.path,
            line: old.line,
            was: None,
            checks: None,
        });
    }
    for new in after.into_iter().flatten() {
        out.push(TestChange {
            what: What::Added,
            security: security(&new).is_some(),
            label: new.label,
            path: new.path,
            line: new.line,
            was: None,
            checks: None,
        });
    }
    out.sort_by(|a, b| {
        (a.what, !a.security, &a.path, a.line).cmp(&(b.what, !b.security, &b.path, b.line))
    });
    out
}

/// What happened between two versions of one test, if anything did.
fn between(old: &TestCase, new: TestCase) -> Option<TestChange> {
    let what = if new.ignored && !old.ignored {
        What::Skipped
    } else if new.checks < old.checks {
        What::Weakened
    } else if new.shape != old.shape {
        What::Changed
    } else if old.ignored && !new.ignored {
        What::Unskipped
    } else if new.label != old.label || new.path != old.path {
        What::Renamed
    } else {
        return None;
    };
    let was = if old.path != new.path {
        Some(format!("{} › {}", old.path, old.label))
    } else if old.label != new.label {
        Some(old.label.clone())
    } else {
        None
    };
    Some(TestChange {
        what,
        security: security(&new).is_some(),
        label: new.label,
        path: new.path,
        line: new.line,
        was,
        checks: Some((old.checks, new.checks)),
    })
}

/// A stretch of history the tests are compared across. The repository is what is reviewed;
/// a stretch is what is marked in it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "hash")]
pub enum Stretch {
    /// What is not committed yet.
    Uncommitted,
    /// HEAD against its parent.
    LastCommit,
    /// Everything since the last commit more than a week old, uncommitted work included.
    Week,
    /// One commit against its parent: a drill-down from the log.
    Commit(String),
}

impl Stretch {
    pub const CHOICES: [Stretch; 3] = [Stretch::Week, Stretch::LastCommit, Stretch::Uncommitted];

    /// The name a front end puts on the button that picks it.
    pub fn name(&self) -> &'static str {
        match self {
            Stretch::Uncommitted => "uncommitted",
            Stretch::LastCommit => "last commit",
            Stretch::Week => "last 7 days",
            Stretch::Commit(_) => "commit",
        }
    }
}

/// A stretch pinned to revisions: `None` for `from` is before the first commit, and for `to`
/// the working tree.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Span {
    pub stretch: Stretch,
    /// What it covers, for a heading: "the last 7 days, since 1a2b3c4".
    pub label: String,
    pub from: Option<String>,
    pub to: Option<String>,
    /// What a diff of the stretch starts from: `from`, or the empty tree when that is before
    /// the first commit, where every file is new.
    pub base: String,
}

const WEEK: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// Pins `stretch` to revisions in `session`'s repository.
pub fn span(session: &Session, stretch: &Stretch) -> Result<Span> {
    let repo = &session.repo;
    let has = repo.has_commits();
    let short = |rev: &str| repo.short_hash(rev).unwrap_or_else(|_| rev.to_string());
    let (label, from, to) = match stretch {
        Stretch::Uncommitted => (
            "uncommitted work".to_string(),
            has.then(|| "HEAD".to_string()),
            None,
        ),
        Stretch::LastCommit => {
            if !has {
                bail!("no commits yet");
            }
            let head = repo.commit("HEAD")?;
            (
                format!("the last commit, {} {}", head.short, head.subject),
                repo.parent("HEAD")?,
                Some(head.hash),
            )
        }
        Stretch::Week => {
            let from = repo.last_before(SystemTime::now() - WEEK);
            let label = match &from {
                Some(f) => format!("the last 7 days, since {}", short(f)),
                None => "the last 7 days, which is all of it".to_string(),
            };
            (label, from, None)
        }
        Stretch::Commit(hash) => {
            if hash.is_empty() || !hash.chars().all(|c| c.is_ascii_hexdigit()) {
                bail!("{hash:?} is not a commit hash");
            }
            let commit = repo.commit(hash)?;
            (
                format!("commit {} {}", commit.short, commit.subject),
                repo.parent(&commit.hash)?,
                Some(commit.hash),
            )
        }
    };
    Ok(Span {
        stretch: stretch.clone(),
        label,
        base: from
            .clone()
            .unwrap_or_else(|| crate::repo::EMPTY_TREE.to_string()),
        from,
        to,
    })
}

/// Every test `span` touched, as it was and as it is, compared.
pub fn changes(session: &Session, span: &Span) -> Result<Vec<TestChange>> {
    let repo = &session.repo;
    let paths = repo.paths_between(span.from.as_deref(), span.to.as_deref())?;
    let mut before = Vec::new();
    let mut after = Vec::new();
    let text = |bytes: Option<Vec<u8>>| bytes.map(|b| String::from_utf8_lossy(&b).into_owned());
    for path in paths.iter().filter(|p| testable(p)) {
        let old = match &span.from {
            Some(from) => text(repo.show(from, path)?),
            None => None,
        };
        let new = match &span.to {
            Some(to) => text(repo.show(to, path)?),
            None => text(repo.read_working(path)?),
        };
        if let Some(old) = old {
            before.extend(crate::symbols::tests_of(path, &old));
        }
        if let Some(new) = new {
            after.extend(crate::symbols::tests_of(path, &new));
        }
    }
    Ok(compare(before, after))
}

/// One test in the specification: what kind of test it is, the files it names and the areas
/// it counts towards.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpecTest {
    #[serde(flatten)]
    pub test: TestCase,
    /// The eight questions, answered.
    pub kind: Kind,
    /// Whether it runs the program and drives it the way a person does.
    pub end_to_end: bool,
    /// Why it is about security, when it is.
    pub security: Option<String>,
    /// The files outside test code that its body names; empty while the index is built, and
    /// for a test that drives the system from outside.
    #[serde(default)]
    pub reaches: Vec<String>,
    /// The areas of what it reaches, or of where it is when it reaches nothing it names.
    #[serde(default)]
    pub areas: Vec<String>,
    /// The area its own file is in: where it is filed, once, however much it names.
    #[serde(default)]
    pub home: String,
}

/// Directory names that hold a project's code rather than name a part of it: an area is the
/// first directory past these.
const SOURCE_ROOTS: &[&str] = &[
    "src", "lib", "app", "pkg", "internal", "source", "sources", "crates", "packages", "cmd",
];

/// The part of the project `path` belongs to, for a summary: its first directory that is not
/// a source root (`src/web/api.rs` is `web`, `crates/core/src/x.rs` is `core`), the source
/// root itself for a file directly in one (`src/main.rs` is `src`), and `top level` for a
/// file at the root.
pub fn area_of(path: &str) -> String {
    let mut dirs: Vec<&str> = path.split('/').collect();
    dirs.pop();
    match dirs.iter().find(|d| !SOURCE_ROOTS.contains(d)) {
        Some(d) => d.to_string(),
        None => dirs.last().map_or("top level", |d| d).to_string(),
    }
}

/// Area names as they are in this project: [`area_of`], except that a file directly in a
/// source root and named after an area is that area's, the way Rust keeps a module's root
/// file beside its directory (`src/web.rs` is `web`, with `src/web/`).
struct AreaNames(BTreeSet<String>);

impl AreaNames {
    fn new<'a>(paths: impl Iterator<Item = &'a str>) -> Self {
        Self(
            paths
                .filter(|p| {
                    let mut dirs: Vec<&str> = p.split('/').collect();
                    dirs.pop();
                    dirs.iter().any(|d| !SOURCE_ROOTS.contains(d))
                })
                .map(area_of)
                .collect(),
        )
    }

    fn of(&self, path: &str) -> String {
        let (dir, file) = path.rsplit_once('/').unwrap_or(("", path));
        let stem = file.split('.').next().unwrap_or(file);
        let in_root = dir.is_empty() || dir.split('/').all(|d| SOURCE_ROOTS.contains(&d));
        if in_root && !dir.is_empty() && self.0.contains(stem) {
            return stem.to_string();
        }
        area_of(path)
    }
}

/// What the symbol index knows about tests and code, for the summary: which files each test
/// names, and every file whose code a test could reach.
#[derive(Debug, Clone, Default)]
pub struct Coverage {
    pub reach: HashMap<(String, usize), Vec<String>>,
    pub code: Vec<String>,
}

/// One part of the project, as the summary shows it: how much code is there, how much of it
/// no test names, and how many tests of each group count towards it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Area {
    pub name: String,
    /// Files of code here, in a language whose tests are found.
    pub files: usize,
    /// Of those, the ones no test names and that have no tests of their own.
    pub unreached: usize,
    /// Tests counting towards it, by group, in the order of [`Spec::groups`].
    pub counts: Vec<usize>,
    pub ignored: usize,
}

/// The areas of the project, with what the tests do for each. A test counts towards every
/// area whose files it names, and towards its own area when it names none.
pub fn areas(tests: &[SpecTest], coverage: Option<&Coverage>, groups: &[Group]) -> Vec<Area> {
    let mut out: BTreeMap<String, Area> = BTreeMap::new();
    let blank = |name: &str| Area {
        name: name.to_string(),
        files: 0,
        unreached: 0,
        counts: vec![0; groups.len()],
        ignored: 0,
    };
    let names = names_for(tests, coverage);
    if let Some(coverage) = coverage {
        let reached: BTreeSet<&str> = coverage
            .reach
            .values()
            .flatten()
            .map(String::as_str)
            .chain(tests.iter().map(|t| t.test.path.as_str()))
            .collect();
        for file in &coverage.code {
            let name = names.of(file);
            let area = out.entry(name.clone()).or_insert_with(|| blank(&name));
            area.files += 1;
            area.unreached += usize::from(!reached.contains(file.as_str()));
        }
    }
    for test in tests {
        let summary = groups.first().map_or("", |g| test.kind.summary(g.question));
        let at = groups.iter().position(|g| g.title == summary);
        for name in &test.areas {
            let area = out.entry(name.clone()).or_insert_with(|| blank(name));
            if let Some(at) = at {
                area.counts[at] += 1;
            }
            area.ignored += usize::from(test.test.ignored);
        }
    }
    out.into_values().collect()
}

/// The area names of a project, from its code when that is known and its tests otherwise.
fn names_for(tests: &[SpecTest], coverage: Option<&Coverage>) -> AreaNames {
    match coverage {
        Some(c) => AreaNames::new(c.code.iter().map(String::as_str)),
        None => AreaNames::new(tests.iter().map(|t| t.test.path.as_str())),
    }
}

/// `tests` with what kind of test each is, what each names, and the areas it counts towards.
/// The files of the repository are read for the tests' lines; `coverage`, the symbol index's
/// word on what each test names, is `None` while the index is built, which leaves what a test
/// calls not classified.
pub fn annotate(root: &Path, tests: Vec<TestCase>, coverage: Option<&Coverage>) -> Vec<SpecTest> {
    let recordings = crate::recording::find(root);
    let mut drivers = crate::e2e::Drivers::new(root);
    let mut reader = crate::classify::Reader::new(root);
    let names = match coverage {
        Some(c) => AreaNames::new(c.code.iter().map(String::as_str)),
        None => AreaNames::new(tests.iter().map(|t| t.path.as_str())),
    };
    tests
        .into_iter()
        .map(|test| {
            let reaches = coverage
                .and_then(|c| c.reach.get(&(test.path.clone(), test.line)))
                .cloned()
                .unwrap_or_default();
            let mut areas: Vec<String> = reaches.iter().map(|p| names.of(p)).collect();
            if areas.is_empty() {
                areas.push(names.of(&test.path));
            }
            areas.sort();
            areas.dedup();
            let recorded = recordings.iter().any(|r| crate::e2e::names(r, &test));
            let drives = drivers.why(&test, recorded);
            let (body, above) = reader.lines(&test);
            let kind = crate::classify::classify(&crate::classify::Evidence {
                test: &test,
                body: &body,
                above: &above,
                names: coverage.map(|_| reaches.as_slice()),
                drives: drives.as_deref(),
                exists: &|p| root.join(p).is_file(),
            });
            SpecTest {
                home: names.of(&test.path),
                end_to_end: kind.end_to_end(),
                security: security(&test),
                kind,
                test,
                reaches,
                areas,
            }
        })
        .collect()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpecFile {
    pub path: String,
    pub tests: Vec<SpecTest>,
}

/// The tests that answer one question the same way, by file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Group {
    pub question: Question,
    /// The answer they share: one of [`Question::summaries`].
    pub title: String,
    /// What the tests in it are, in a sentence; empty when there is nothing to add.
    pub says: String,
    /// What it means that there are none.
    pub none: String,
    /// Whether having none is a finding.
    pub finding: bool,
    pub files: Vec<SpecFile>,
    pub total: usize,
    pub ignored: usize,
}

/// Every test of the repository, grouped by its answer to `by` and then by file. Every option
/// of the question has a group, even an empty one, since no test driving the program is itself
/// a finding; mixed, none and not classified appear only when some test is.
pub fn groups(tests: Vec<SpecTest>, by: Question) -> Vec<Group> {
    let options = by.options();
    let mut groups: Vec<Group> = by
        .summaries()
        .into_iter()
        .map(|title| {
            let (says, none, finding) = crate::classify::describe(by, title);
            Group {
                question: by,
                title: title.to_string(),
                says: says.to_string(),
                none: none.to_string(),
                finding,
                files: Vec::new(),
                total: 0,
                ignored: 0,
            }
        })
        .collect();
    for test in tests {
        let summary = test.kind.summary(by).to_string();
        let group = groups
            .iter_mut()
            .find(|g| g.title == summary)
            .expect("a summary of the question");
        group.total += 1;
        group.ignored += usize::from(test.test.ignored);
        match group.files.last_mut() {
            Some(file) if file.path == test.test.path => file.tests.push(test),
            _ => group.files.push(SpecFile {
                path: test.test.path.clone(),
                tests: vec![test],
            }),
        }
    }
    groups.retain(|g| g.total > 0 || options.contains(&g.title.as_str()));
    groups
}

/// A question, as the front ends offer it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Asked {
    pub question: Question,
    pub title: String,
    pub short: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Spec {
    /// The question the tests are grouped by.
    pub by: Question,
    /// Every question there is to group by.
    pub questions: Vec<Asked>,
    pub span: Span,
    /// What the stretch did to the tests, worst first.
    pub changes: Vec<TestChange>,
    pub groups: Vec<Group>,
    /// The project by area, with what the tests do for each.
    pub areas: Vec<Area>,
    pub total: usize,
    pub ignored: usize,
}

/// The specification of `tests`, the repository's tests as they are, with what `stretch` did
/// to them. `coverage` is what the index knows about which code each test names, absent
/// while it is built.
pub fn of(
    session: &Session,
    tests: Vec<TestCase>,
    coverage: Option<&Coverage>,
    stretch: &Stretch,
    by: Question,
) -> Result<Spec> {
    let span = span(session, stretch)?;
    let changes = session.spec_changes(&span)?;
    let total = tests.len();
    let ignored = tests.iter().filter(|t| t.ignored).count();
    let tests = annotate(session.root(), tests, coverage);
    let grouped = groups(tests.clone(), by);
    Ok(Spec {
        by,
        questions: Question::ALL
            .iter()
            .map(|&question| Asked {
                question,
                title: question.title().to_string(),
                short: question.short().to_string(),
            })
            .collect(),
        span,
        changes,
        areas: areas(&tests, coverage, &grouped),
        groups: grouped,
        total,
        ignored,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn case(path: &str, name: &str) -> TestCase {
        TestCase {
            name: name.into(),
            label: crate::testsuite::phrase(name),
            path: path.into(),
            line: 1,
            end: 1,
            ignored: false,
            checks: 2,
            shape: 7,
        }
    }

    #[test]
    fn security_is_a_mark_with_its_reason() {
        let at = |path: &str, name: &str| security(&case(path, name));
        assert_eq!(
            at("src/web/tests.rs", "a_password_opens_a_session").as_deref(),
            Some("its name says password")
        );
        assert_eq!(
            at("src/auth/mod.rs", "round_trips").as_deref(),
            Some("its path says auth")
        );
        assert_eq!(
            at("src/web/tests.rs", "a_sign_in_is_slowed").as_deref(),
            Some("its name says signin")
        );
        assert_eq!(at("src/lexer.rs", "tokenizer_splits"), None);
        assert_eq!(at("e2e/cart.spec.ts", "adds an item"), None);
    }

    #[test]
    fn a_path_says_end_to_end_only_in_a_file_of_tests() {
        let at = |path: &str| crate::e2e::path_says(&case(path, "x"));
        assert_eq!(at("e2e/cart.spec.ts").as_deref(), Some("its path says e2e"));
        assert_eq!(
            at("tests/browser.rs").as_deref(),
            Some("its path says browser")
        );
        assert_eq!(
            at("tests/end_to_end/login.py").as_deref(),
            Some("its path says end to end")
        );
        assert_eq!(at("src/browser.rs"), None, "a module, not a suite");
        assert_eq!(
            at("tests/login.e2e.ts").as_deref(),
            Some("its path says e2e")
        );
        assert_eq!(
            at("src/test/fixtures/css_playwright_selector.rs"),
            None,
            "a fixture named after what it is about"
        );
        assert_eq!(at("tests/nginx_proxy.rs"), None);
    }

    #[test]
    fn what_happened_to_each_test() {
        let before = vec![
            case("a.rs", "stays"),
            case("a.rs", "goes"),
            case("a.rs", "gets_skipped"),
            case("a.rs", "loses_a_check"),
            case("a.rs", "is_edited"),
            TestCase {
                shape: 99,
                ..case("a.rs", "old_name")
            },
            case("a.rs", "moves_file"),
        ];
        let after = vec![
            case("a.rs", "stays"),
            TestCase {
                ignored: true,
                ..case("a.rs", "gets_skipped")
            },
            TestCase {
                checks: 1,
                shape: 8,
                ..case("a.rs", "loses_a_check")
            },
            TestCase {
                shape: 8,
                ..case("a.rs", "is_edited")
            },
            TestCase {
                shape: 99,
                ..case("a.rs", "new_name")
            },
            case("b.rs", "moves_file"),
            TestCase {
                shape: 50,
                ..case("a.rs", "brand_new")
            },
        ];
        let changes = compare(before, after);
        let seen: Vec<(What, &str, Option<&str>)> = changes
            .iter()
            .map(|c| (c.what, c.label.as_str(), c.was.as_deref()))
            .collect();
        assert_eq!(
            seen,
            vec![
                (What::Removed, "goes", None),
                (What::Skipped, "gets skipped", None),
                (What::Weakened, "loses a check", None),
                (What::Changed, "is edited", None),
                (What::Renamed, "new name", Some("old name")),
                (What::Renamed, "moves file", Some("a.rs › moves file")),
                (What::Added, "brand new", None),
            ]
        );
        assert_eq!(changes[2].checks, Some((2, 1)));
        assert!(changes[0].what.alarming() && !changes[3].what.alarming());
    }

    #[test]
    fn an_area_is_the_first_directory_past_the_source_root() {
        assert_eq!(area_of("src/web/api.rs"), "web");
        assert_eq!(area_of("src/tui/app/screens.rs"), "tui");
        assert_eq!(area_of("src/main.rs"), "src");
        assert_eq!(area_of("crates/core/src/lib.rs"), "core");
        assert_eq!(area_of("tests/browser.rs"), "tests");
        assert_eq!(area_of("main.go"), "top level");
    }

    #[test]
    fn a_test_counts_towards_what_it_names() {
        let coverage = Coverage {
            reach: HashMap::from([
                (
                    ("src/web/tests.rs".into(), 1),
                    vec!["src/web/api.rs".into(), "src/session.rs".into()],
                ),
                (("tests/browser.rs".into(), 1), vec![]),
            ]),
            code: vec![
                "src/web.rs".into(),
                "src/web/api.rs".into(),
                "src/web/auth.rs".into(),
                "src/session.rs".into(),
                "src/tui/app.rs".into(),
            ],
        };
        let dir = tempfile::tempdir().unwrap();
        let tests = annotate(
            dir.path(),
            vec![
                case("src/web/tests.rs", "api_round_trip"),
                case("tests/browser.rs", "the_phone_layout_works"),
            ],
            Some(&coverage),
        );
        assert_eq!(tests[0].areas, ["src", "web"]);
        assert_eq!(
            tests[1].areas,
            ["tests"],
            "names nothing: counted where it is"
        );
        let grouped = groups(tests.clone(), Question::Interaction);
        let titles: Vec<&str> = grouped.iter().map(|g| g.title.as_str()).collect();
        assert_eq!(titles, ["API", "GUI"], "no test is in a group of its own");
        let areas = areas(&tests, Some(&coverage), &grouped);
        let row = |name: &str| areas.iter().find(|a| a.name == name).unwrap().clone();
        let web = row("web");
        assert_eq!(
            (web.files, web.unreached),
            (3, 2),
            "web.rs is web's, and it and auth.rs are named by no test"
        );
        assert_eq!(web.counts, [1, 0], "it names api.rs: it calls the code");
        let tui = row("tui");
        assert_eq!(
            (tui.files, tui.unreached, tui.counts.iter().sum::<usize>()),
            (1, 1, 0)
        );
        assert_eq!(
            row("tests").counts,
            [0, 1],
            "its path says browser: it drives the program"
        );
    }

    #[test]
    fn every_option_of_the_question_is_a_group_even_empty() {
        let dir = tempfile::tempdir().unwrap();
        let groups = groups(
            annotate(
                dir.path(),
                vec![case("src/a.rs", "x"), case("src/a.rs", "y")],
                Some(&Coverage::default()),
            ),
            Question::Interaction,
        );
        let titles: Vec<&str> = groups.iter().map(|g| g.title.as_str()).collect();
        assert_eq!(titles, ["API", "GUI", "No interaction"]);
        assert_eq!(groups[2].total, 2, "they call nothing the index knows");
        assert_eq!(groups[2].files.len(), 1);
        assert_eq!(groups[1].total, 0);
    }

    /// A repository whose history does to its tests what an agent might.
    #[test]
    fn a_stretch_of_history_against_a_real_repository() {
        let dir = tempfile::tempdir().unwrap();
        let git = |args: &[&str]| {
            let out = Command::new("git")
                .arg("-C")
                .arg(dir.path())
                .args(args)
                .output()
                .unwrap();
            assert!(out.status.success(), "{args:?}: {out:?}");
        };
        git(&["init", "-q"]);
        git(&["config", "user.name", "T"]);
        git(&["config", "user.email", "t@example.com"]);
        let write = |text: &str| std::fs::write(dir.path().join("lib.rs"), text).unwrap();
        write(
            "#[test]\nfn checks_two_things() {\n    assert!(true);\n    assert_eq!(1, 1);\n}\n\n\
             #[test]\nfn will_go() {\n    assert!(true);\n}\n",
        );
        git(&["add", "."]);
        git(&["commit", "-q", "-m", "first"]);
        // The last commit: one test loses an assertion.
        write(
            "#[test]\nfn checks_two_things() {\n    assert!(true);\n}\n\n\
             #[test]\nfn will_go() {\n    assert!(true);\n}\n",
        );
        git(&["commit", "-q", "-am", "loosen"]);
        // Uncommitted: the other is deleted.
        write("#[test]\nfn checks_two_things() {\n    assert!(true);\n}\n");

        let config = dir.path().join(".git").join("codereview-config.toml");
        let session = Session::open_with_config(dir.path(), Some(config)).unwrap();
        let tests = crate::symbols::tests_of("lib.rs", "#[test]\nfn checks_two_things() {}\n");

        let last = of(
            &session,
            tests.clone(),
            None,
            &Stretch::LastCommit,
            Question::Interaction,
        )
        .unwrap();
        assert!(last.span.label.contains("loosen"), "{}", last.span.label);
        let seen: Vec<_> = last.changes.iter().map(|c| (c.what, c.checks)).collect();
        assert_eq!(seen, [(What::Weakened, Some((2, 1)))]);

        let now = of(
            &session,
            tests.clone(),
            None,
            &Stretch::Uncommitted,
            Question::Interaction,
        )
        .unwrap();
        let seen: Vec<_> = now
            .changes
            .iter()
            .map(|c| (c.what, c.label.as_str()))
            .collect();
        assert_eq!(seen, [(What::Removed, "will go")]);

        // The week covers both, and the first commit had no parent to compare with: from the
        // start, everything that is there now is new, and nothing was removed.
        let week = of(
            &session,
            tests.clone(),
            None,
            &Stretch::Week,
            Question::Interaction,
        )
        .unwrap();
        let seen: Vec<_> = week.changes.iter().map(|c| c.what).collect();
        assert_eq!(seen, [What::Added]);

        let first = session.repo.log(None, None, 1, 1).unwrap()[0].hash.clone();
        let drill = of(
            &session,
            tests,
            None,
            &Stretch::Commit(first),
            Question::Interaction,
        )
        .unwrap();
        assert!(drill.span.label.contains("first"));
        assert_eq!(drill.changes.len(), 2, "both added by the root commit");
        assert!(drill.changes.iter().all(|c| c.what == What::Added));

        assert!(span(&session, &Stretch::Commit("--all".into())).is_err());
    }
}
