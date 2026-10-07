//! What a stretch of history did, seen through the end-to-end tests.
//!
//! Three questions, in the order a reviewer needs them answered. Did the tests stop asking
//! anything — removed, skipped, weakened? That is `spec::changes`, and it comes first. Which
//! of the changed code did no end-to-end test run? That is where a person still has to read,
//! and it is found by laying the lines the stretch added or changed over the lines each
//! test's last run executed. And does the software still do what it did? For that each test
//! is shown as it ran before the stretch and as it runs after: its outcome, its steps, and,
//! for a terminal recording, whether the screen at each step is the one it was.
//!
//! The "before" is a run at the stretch's base commit, made in a worktree of that commit kept
//! inside the repository's git directory, by the project's own end-to-end command. Like every
//! run here, what it leaves is files — the recordings in that worktree — and nothing the tool
//! keeps. It is offered, never started unasked: it runs the code as it was, which is the
//! repository's code like any test run, and it can take minutes.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::config::Check;
use crate::e2e::{EndToEnd, test_only};
use crate::recording::{Recording, Step};
use crate::session::Session;
use crate::spec::{Span, Stretch, TestChange};

/// A file with changed code that no end-to-end test ran.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Unran {
    pub path: String,
    /// Lines of code the stretch added or changed here.
    pub changed: usize,
    /// Of those, the ones no end-to-end test ran.
    pub lines: usize,
    /// Those lines, as first and last of each run of them.
    pub ranges: Vec<(u32, u32)>,
}

/// An end-to-end test that ran some of what the stretch changed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Touched {
    pub path: String,
    pub name: String,
    pub label: String,
    /// Changed lines its run executed.
    pub lines: usize,
    pub files: Vec<String>,
}

/// The commit a stretch starts from, and what is known of the tests as they ran there.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Base {
    pub commit: String,
    pub short: String,
    /// Recordings made at it.
    pub recorded: usize,
    /// The command that would record them, when the repository has one.
    pub command: Option<String>,
}

/// How one end-to-end test went before the stretch and after it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Outcome {
    pub path: String,
    pub name: String,
    pub label: String,
    /// Whether it passed, where it ran.
    pub before: Option<bool>,
    pub after: Option<bool>,
    /// Steps there on one side and not the other.
    pub steps_changed: usize,
    /// Steps at which the screen is not what it was; `None` without two terminal recordings.
    pub screens_changed: Option<usize>,
}

/// What a stretch did, as the opening shows it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Report {
    pub span: Span,
    /// What happened to the tests, worst first.
    pub changes: Vec<TestChange>,
    /// Lines of code the stretch added or changed, and how many of them no end-to-end test
    /// ran.
    pub changed: usize,
    pub unran_lines: usize,
    pub unran: Vec<Unran>,
    pub touched: Vec<Touched>,
    /// End-to-end tests whose last run left no coverage: what they ran is not known, so
    /// "no test ran it" may be wrong by as much as they would have said.
    pub without_coverage: usize,
    pub base: Option<Base>,
    /// For each end-to-end test, before against after, once the base is recorded.
    pub outcomes: Vec<Outcome>,
}

/// One step of a test as it ran before and after: the same step on both sides, or one side.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepPair {
    pub before: Option<Step>,
    pub after: Option<Step>,
    /// Whether the screen at this step is the one it was; `None` without two terminal
    /// recordings.
    pub same_screen: Option<bool>,
}

/// One end-to-end test before and after a stretch, step by step.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Comparison {
    pub base: Option<Base>,
    pub before: Option<Recording>,
    pub after: Option<Recording>,
    pub steps: Vec<StepPair>,
}

/// At most this many files are listed as changed code no test ran.
const MOST_UNRAN: usize = 100;

/// How many worktrees of earlier commits are kept; the oldest goes when another is made.
const WORKTREES_KEPT: usize = 3;

/// Where the worktree of `commit` is kept: in the git directory, which no status shows and no
/// tool walks.
pub fn worktree_path(session: &Session, commit: &str) -> Result<PathBuf> {
    Ok(session
        .repo
        .common_dir()?
        .join("codereview")
        .join("before")
        .join(commit))
}

/// The command that records the end-to-end tests: a configured or found check whose name or
/// command says end to end.
pub fn command(session: &Session) -> Option<Check> {
    let root = session.root();
    let says = |c: &Check| {
        let text = format!("{} {}", c.name, c.run).to_lowercase();
        ["e2e", "end-to-end", "end_to_end", "endtoend"]
            .iter()
            .any(|w| text.contains(w))
    };
    session
        .config
        .analysis
        .iter()
        .filter(|c| crate::analysis::applies(c, root))
        .find(|c| says(c))
        .cloned()
        .or_else(|| crate::analysis::detect(root).into_iter().find(says))
}

/// Makes the worktree of `commit` to record the tests in, and says where it is and what to
/// run there. Older worktrees beyond the few kept are removed.
pub fn prepare(session: &Session, commit: &str) -> Result<(PathBuf, Check)> {
    let check = command(session)
        .ok_or_else(|| anyhow::anyhow!("no command records the end-to-end tests here"))?;
    let commit = session.repo.resolve(commit)?;
    let path = worktree_path(session, &commit)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
        let mut kept: Vec<(std::time::SystemTime, PathBuf)> = std::fs::read_dir(parent)?
            .flatten()
            .map(|e| e.path())
            .filter(|p| *p != path)
            .filter_map(|p| Some((std::fs::metadata(&p).ok()?.modified().ok()?, p)))
            .collect();
        kept.sort();
        while kept.len() >= WORKTREES_KEPT {
            let (_, old) = kept.remove(0);
            let _ = session.repo.remove_worktree(&old);
            let _ = std::fs::remove_dir_all(&old);
        }
    }
    session.repo.worktree(&commit, &path)?;
    Ok((path, check))
}

/// The recordings made at `commit`, if any.
fn recordings_at(session: &Session, commit: &str) -> Vec<Recording> {
    match worktree_path(session, commit) {
        Ok(path) if path.is_dir() => crate::recording::find(&path),
        _ => Vec::new(),
    }
}

/// The base of `span` as a commit, with what is recorded there.
fn base_of(session: &Session, span: &Span) -> Option<Base> {
    let from = span.from.as_deref()?;
    let commit = session.repo.resolve(from).ok()?;
    Some(Base {
        short: session
            .repo
            .short_hash(&commit)
            .unwrap_or_else(|_| commit.clone()),
        recorded: recordings_at(session, &commit).len(),
        command: command(session).map(|c| c.run),
        commit,
    })
}

/// The recording of `test` among `recordings`.
fn recording_of<'a>(recordings: &'a [Recording], test: &EndToEnd) -> Option<&'a Recording> {
    recordings.iter().find(|r| {
        r.file == test.test.path
            && (r.test == test.test.name
                || r.test == test.test.label
                || crate::testsuite::phrase(&r.test) == test.test.label)
    })
}

/// What `stretch` did, through the end-to-end tests.
pub fn report(session: &mut Session, stretch: &Stretch) -> Result<Report> {
    let span = crate::spec::span(session, stretch)?;
    let changes = session.spec_changes(&span)?;
    let opening = session.end_to_end();
    let root = session.root().to_path_buf();

    // What the stretch changed, in code: not test code, not files of nothing but tests, and
    // only files the tool sees as code or some test's coverage names.
    let mut changed = session.repo.changed_lines(&span.base, span.to.as_deref())?;
    let ran: Vec<(&EndToEnd, BTreeMap<String, Vec<u32>>)> = opening
        .tests
        .iter()
        .filter_map(|t| {
            let r = t.recording.as_ref()?;
            Some((t, crate::recording::lines_run(&root, r)?))
        })
        .collect();
    let code: HashSet<String> = session.coverage().code.into_iter().collect();
    let covered: HashSet<&str> = ran
        .iter()
        .flat_map(|(_, m)| m.keys().map(String::as_str))
        .collect();
    changed.retain(|path, _| {
        !test_only(path) && (code.contains(path) || covered.contains(path.as_str()))
    });
    // Only lines that are code: those some test's coverage counts at all, in a file coverage
    // knows; elsewhere, lines with something on them. And not test code inside a file of code.
    // Coverage made before a file last changed does not know its new lines, so it only says
    // which lines are code for a file it is newer than.
    let mut counted: BTreeMap<String, (HashSet<u32>, std::time::SystemTime)> = BTreeMap::new();
    for t in &opening.tests {
        let Some(r) = t.recording.as_ref() else {
            continue;
        };
        let Some(lines) = crate::recording::lines_counted(&root, r) else {
            continue;
        };
        let made = std::fs::metadata(root.join(&r.dir).join(crate::recording::COVERAGE))
            .and_then(|m| m.modified())
            .unwrap_or(std::time::UNIX_EPOCH);
        for (path, lines) in lines {
            let entry = counted
                .entry(path)
                .or_insert_with(|| (HashSet::new(), std::time::UNIX_EPOCH));
            entry.0.extend(lines);
            entry.1 = entry.1.max(made);
        }
    }
    for (path, lines) in changed.iter_mut() {
        let tests = session.index().test_rows(path);
        lines.retain(|l| !tests.iter().any(|(a, b)| (*a..=*b).contains(&(l - 1))));
        let written = std::fs::metadata(root.join(path))
            .and_then(|m| m.modified())
            .ok();
        match counted.get(path) {
            Some((code, made)) if written.is_some_and(|w| w <= *made) => {
                lines.retain(|l| code.contains(l))
            }
            _ => {
                let text = std::fs::read_to_string(root.join(path)).unwrap_or_default();
                let rows: Vec<&str> = text.lines().collect();
                lines.retain(|l| {
                    rows.get(*l as usize - 1)
                        .is_some_and(|row| !row.trim().is_empty())
                });
            }
        }
    }
    changed.retain(|_, lines| !lines.is_empty());

    let mut touched: Vec<Touched> = ran
        .iter()
        .filter_map(|(t, m)| {
            let mut files = Vec::new();
            let mut lines = 0;
            for (path, mine) in &changed {
                let Some(theirs) = m.get(path) else {
                    continue;
                };
                let n = mine
                    .iter()
                    .filter(|l| theirs.binary_search(l).is_ok())
                    .count();
                if n > 0 {
                    lines += n;
                    files.push(path.clone());
                }
            }
            (lines > 0).then(|| Touched {
                path: t.test.path.clone(),
                name: t.test.name.clone(),
                label: t.test.label.clone(),
                lines,
                files,
            })
        })
        .collect();
    touched.sort_by(|a, b| b.lines.cmp(&a.lines).then(a.label.cmp(&b.label)));

    let mut unran: Vec<Unran> = changed
        .iter()
        .filter_map(|(path, lines)| {
            let left: Vec<u32> = lines
                .iter()
                .copied()
                .filter(|l| {
                    !ran.iter()
                        .any(|(_, m)| m.get(path).is_some_and(|r| r.binary_search(l).is_ok()))
                })
                .collect();
            (!left.is_empty()).then(|| Unran {
                path: path.clone(),
                changed: lines.len(),
                lines: left.len(),
                ranges: crate::recording::ranges(&left),
            })
        })
        .collect();
    unran.sort_by(|a, b| b.lines.cmp(&a.lines).then(a.path.cmp(&b.path)));
    let unran_lines = unran.iter().map(|u| u.lines).sum();
    unran.truncate(MOST_UNRAN);

    let base = base_of(session, &span);
    let before = base
        .as_ref()
        .map(|b| recordings_at(session, &b.commit))
        .unwrap_or_default();
    let outcomes = if before.is_empty() {
        Vec::new()
    } else {
        opening
            .tests
            .iter()
            .map(|t| {
                let was = recording_of(&before, t);
                let is = t.recording.as_ref();
                let pairs = pair_steps(
                    was.map_or(&[][..], |r| &r.steps),
                    is.map_or(&[][..], |r| &r.steps),
                );
                let screens = screens(session, base.as_ref(), was, is, &pairs);
                Outcome {
                    path: t.test.path.clone(),
                    name: t.test.name.clone(),
                    label: t.test.label.clone(),
                    before: was.map(|r| r.passed),
                    after: is.map(|r| r.passed),
                    steps_changed: pairs
                        .iter()
                        .filter(|p| p.before.is_none() || p.after.is_none())
                        .count(),
                    screens_changed: screens
                        .map(|s| s.iter().filter(|same| **same == Some(false)).count()),
                }
            })
            .collect()
    };

    Ok(Report {
        span,
        changes,
        changed: changed.values().map(Vec::len).sum(),
        unran_lines,
        unran,
        touched,
        without_coverage: opening.tests.len() - ran.len(),
        base,
        outcomes,
    })
}

/// One end-to-end test across `stretch`, step by step.
pub fn compare(
    session: &mut Session,
    stretch: &Stretch,
    file: &str,
    name: &str,
) -> Result<Comparison> {
    let span = crate::spec::span(session, stretch)?;
    let opening = session.end_to_end();
    let test = opening
        .tests
        .iter()
        .find(|t| t.test.path == file && (t.test.name == name || t.test.label == name))
        .ok_or_else(|| anyhow::anyhow!("no end-to-end test called {name:?} in {file}"))?;
    let base = base_of(session, &span);
    let before_all = base
        .as_ref()
        .map(|b| recordings_at(session, &b.commit))
        .unwrap_or_default();
    let before = recording_of(&before_all, test).cloned();
    let after = test.recording.clone();
    let mut steps = pair_steps(
        before.as_ref().map_or(&[][..], |r| &r.steps),
        after.as_ref().map_or(&[][..], |r| &r.steps),
    );
    if let Some(same) = screens(
        session,
        base.as_ref(),
        before.as_ref(),
        after.as_ref(),
        &steps,
    ) {
        for (pair, same) in steps.iter_mut().zip(same) {
            pair.same_screen = same;
        }
    }
    Ok(Comparison {
        base,
        before,
        after,
        steps,
    })
}

/// The steps of two runs of one test, matched by what they say, in order: a longest common
/// run of the same kind and text, with what is on one side only between.
pub fn pair_steps(before: &[Step], after: &[Step]) -> Vec<StepPair> {
    let same = |a: &Step, b: &Step| a.kind == b.kind && a.text == b.text;
    let (n, m) = (before.len(), after.len());
    let mut table = vec![vec![0u32; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            table[i][j] = if same(&before[i], &after[j]) {
                table[i + 1][j + 1] + 1
            } else {
                table[i + 1][j].max(table[i][j + 1])
            };
        }
    }
    let (mut i, mut j) = (0, 0);
    let mut out = Vec::new();
    let pair = |b: Option<&Step>, a: Option<&Step>| StepPair {
        before: b.cloned(),
        after: a.cloned(),
        same_screen: None,
    };
    while i < n || j < m {
        if i < n && j < m && same(&before[i], &after[j]) {
            out.push(pair(Some(&before[i]), Some(&after[j])));
            i += 1;
            j += 1;
        } else if j < m && (i == n || table[i][j + 1] >= table[i + 1][j]) {
            out.push(pair(None, Some(&after[j])));
            j += 1;
        } else {
            out.push(pair(Some(&before[i]), None));
            i += 1;
        }
    }
    out
}

/// For each pair of steps on both sides, whether the screen at that moment is the same text
/// in both terminal recordings; `None` overall unless both are terminal recordings.
fn screens(
    session: &Session,
    base: Option<&Base>,
    before: Option<&Recording>,
    after: Option<&Recording>,
    pairs: &[StepPair],
) -> Option<Vec<Option<bool>>> {
    let (base, before, after) = (base?, before?, after?);
    let worktree = worktree_path(session, &base.commit).ok()?;
    let play = |root: &Path, r: &Recording| -> Option<crate::recording::Cast> {
        if r.kind != Some(crate::recording::Kind::Terminal) {
            return None;
        }
        let text = std::fs::read_to_string(root.join(r.media()?)).ok()?;
        crate::recording::frames(&text).ok()
    };
    let was = play(&worktree, before)?;
    let is = play(session.root(), after)?;
    let text = |cast: &crate::recording::Cast, at: u64| -> String {
        cast.screen_at(at)
            .iter()
            .map(|row| row.iter().map(|r| r.text.as_str()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    };
    Some(
        pairs
            .iter()
            .map(|p| match (&p.before, &p.after) {
                (Some(b), Some(a)) => Some(text(&was, b.at_ms) == text(&is, a.at_ms)),
                _ => None,
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn step(kind: &str, text: &str) -> Step {
        Step {
            at_ms: 0,
            line: 0,
            kind: kind.into(),
            text: text.into(),
            passed: None,
            detail: None,
        }
    }

    fn put(root: &Path, path: &str, text: &str) {
        let path = root.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    fn git(root: &Path, args: &[&str]) {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(root)
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// A test's recording, its steps, and the lines of `src/lib.rs` its run executed.
    fn record(root: &Path, passed: bool, screen: &str, ran: &[u32]) {
        let dir = root.join("target/e2e/cli/saves");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("test.json"),
            format!(
                r#"{{"test": "a_review_is_saved", "file": "tests/cli.rs", "passed": {passed},
                    "recording": "terminal.cast", "steps": [
                    {{"at_ms": 100, "kind": "action", "text": "save it"}},
                    {{"at_ms": 900, "kind": "check", "text": "it is saved"}}]}}"#
            ),
        )
        .unwrap();
        std::fs::write(
            dir.join("terminal.cast"),
            format!(
                "{{\"version\":2,\"width\":20,\"height\":2}}\n[0.1,\"o\",\"start\"]\n\
                 [0.8,\"o\",\"\\r\\n{screen}\"]\n"
            ),
        )
        .unwrap();
        // As a coverage tool writes it: every line of code, with how often it ran.
        let da: String = [1, 2, 3, 4, 6, 7, 8]
            .iter()
            .map(|l| format!("DA:{l},{}\n", u32::from(ran.contains(l))))
            .collect();
        std::fs::write(
            dir.join("coverage.lcov"),
            format!("SF:src/lib.rs\n{da}end_of_record\n"),
        )
        .unwrap();
    }

    #[test]
    fn a_stretch_says_what_no_test_ran_and_what_each_test_does_now() {
        let dir = crate::session::tests::scratch_repo();
        let root = dir.path();
        put(root, "Makefile", "e2e:\n\tcargo test --test cli\n");
        put(
            root,
            "tests/cli.rs",
            "// CARGO_BIN_EXE_tool\n#[test]\nfn a_review_is_saved() {}\n",
        );
        put(
            root,
            "src/lib.rs",
            "pub fn save_review() {\n    write();\n}\n",
        );
        git(root, &["add", "-A"]);
        git(root, &["commit", "-qm", "base"]);
        let base = String::from_utf8(
            std::process::Command::new("git")
                .arg("-C")
                .arg(root)
                .args(["rev-parse", "HEAD"])
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap()
        .trim()
        .to_string();
        // Uncommitted: the function saves twice now, and a new one nothing runs.
        put(
            root,
            "src/lib.rs",
            "pub fn save_review() {\n    write();\n    write();\n}\n\npub fn export_review() {\n    \
             write();\n}\n\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn exports() {}\n}\n",
        );
        record(root, true, "saved twice", &[1, 2, 3, 4]);
        let mut session = Session::open(root).unwrap();

        let said = report(&mut session, &Stretch::Uncommitted).unwrap();
        assert_eq!(said.touched.len(), 1, "{:?}", said.touched);
        assert_eq!(
            (said.touched[0].lines, said.touched[0].label.as_str()),
            (2, "a review is saved"),
            "the second write, and the brace git counts with it"
        );
        assert_eq!(said.unran.len(), 1);
        assert_eq!(said.unran[0].path, "src/lib.rs");
        assert_eq!(
            said.unran[0].ranges,
            [(6, 8)],
            "the new function is what nothing ran; its unit tests are not code: {:?}",
            said.unran
        );
        let b = said.base.as_ref().unwrap();
        assert_eq!((b.commit.as_str(), b.recorded), (base.as_str(), 0));
        assert_eq!(b.command.as_deref(), Some("make e2e"));
        assert!(said.outcomes.is_empty(), "nothing recorded at the base yet");

        // The tests as they ran at the base, recorded in its worktree.
        let (worktree, check) = prepare(&session, &base).unwrap();
        assert_eq!(check.run, "make e2e");
        assert!(worktree.join("src/lib.rs").is_file());
        assert!(
            !session
                .repo
                .status()
                .unwrap()
                .keys()
                .any(|p| p.contains("codereview")),
            "the worktree is not a change to the repository"
        );
        record(&worktree, false, "saved once", &[1, 2]);
        let after = report(&mut session, &Stretch::Uncommitted).unwrap();
        assert_eq!(after.base.as_ref().unwrap().recorded, 1);
        let o = &after.outcomes[0];
        assert_eq!(
            (o.before, o.after, o.steps_changed),
            (Some(false), Some(true), 0)
        );
        assert_eq!(o.screens_changed, Some(1), "{o:?}");

        let c = compare(
            &mut session,
            &Stretch::Uncommitted,
            "tests/cli.rs",
            "a_review_is_saved",
        )
        .unwrap();
        let same: Vec<Option<bool>> = c.steps.iter().map(|p| p.same_screen).collect();
        assert_eq!(same, [Some(true), Some(false)]);
        assert!(!c.before.unwrap().passed);
    }

    #[test]
    fn steps_are_matched_by_what_they_say() {
        let before = [
            step("action", "open"),
            step("check", "old"),
            step("action", "save"),
        ];
        let after = [
            step("action", "open"),
            step("action", "save"),
            step("check", "new"),
        ];
        let pairs = pair_steps(&before, &after);
        let shape: Vec<(Option<&str>, Option<&str>)> = pairs
            .iter()
            .map(|p| {
                (
                    p.before.as_ref().map(|s| s.text.as_str()),
                    p.after.as_ref().map(|s| s.text.as_str()),
                )
            })
            .collect();
        assert_eq!(
            shape,
            [
                (Some("open"), Some("open")),
                (Some("old"), None),
                (Some("save"), Some("save")),
                (None, Some("new")),
            ]
        );
    }
}
