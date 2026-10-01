//! `OVERVIEW.md`: what the tool last found out about a repository, kept next to `REVIEW.md`
//! and `NOTES.md`, in a shape a person can read and this module can read back.
//!
//! Two reasons for a file. Working most of this out takes time — the symbol index behind
//! "what the rest of it leans on" and the tests is seconds of parsing on a large repository —
//! and a fresh open should not wait for it: the file is read first, shown at once as of the
//! commit it names, and replaced when the work is done. And a repository someone had an agent
//! build has no document saying what it is, where to start, and what its tests check; this is
//! that document, kept current by the tool, and readable by the next agent too.
//!
//! ```text
//! # Overview
//!
//! As of commit 3f2a9c1 on main, 2026-09-25 10:12.
//!
//! ## About
//! (whatever a person or the agent wrote, kept as it is)
//!
//! ## Facts
//! - 70 files, 133 tests in 27 files, 2 marked to skip
//! - make check: passed in 41 s, 2026-09-25 10:12
//!
//! ## Start here
//! - src/main.rs — the command line
//!
//! ## What the rest of it leans on
//! - src/session.rs — Session, DiffTarget, FileView
//!
//! ## Tests
//! ### src/analysis.rs
//! - a command runs and says how it went
//! - behind nginx (skipped)
//!
//! ## Named by no test
//! - src/web/api.rs — get_analysis, get_blame
//! ```
//!
//! The sections the tool owns are rewritten whole. `About`, and any heading it does not know,
//! are kept as they are, so the file can carry what people add to it. Nothing in it changes
//! with the clock, and the tests carry no line numbers, so that it is rewritten only when the
//! repository changed in a way worth a line of a commit's diff.

use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

pub const FILE_NAME: &str = "OVERVIEW.md";

const ABOUT: &str = "About";
const FACTS: &str = "Facts";
const START: &str = "Start here";
const LEANS: &str = "What the rest of it leans on";
const TESTS: &str = "Tests";
const UNTESTED: &str = "Named by no test";

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Facts {
    /// The commit, abbreviated, the facts were worked out at; `None` before the first one.
    pub commit: Option<String>,
    pub branch: String,
    /// When, as `YYYY-MM-DD HH:MM`.
    pub when: String,
    pub files: usize,
    pub tests: usize,
    pub test_files: usize,
    pub skipped: usize,
    pub checks: Vec<CheckFact>,
    /// `(path, why)`.
    pub entries: Vec<(String, String)>,
    /// `(path, the names that carried it)`.
    pub leans_on: Vec<(String, Vec<String>)>,
    /// `(path, its tests as phrases, each with whether it is marked to skip)`.
    pub tests_by_file: Vec<(String, Vec<(String, bool)>)>,
    pub untested: Vec<(String, Vec<String>)>,
    /// The lines under `## About`, as they are.
    pub about: Vec<String>,
    /// Sections under headings this module does not own, as they are, in order.
    pub kept: Vec<String>,
}

/// What a check last did, as a fact worth a line: not when it ran relative to now.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CheckFact {
    pub name: String,
    pub passed: bool,
    pub seconds: f64,
    /// When it started, as `YYYY-MM-DD HH:MM`.
    pub when: String,
}

impl CheckFact {
    /// A finished run as a fact; `None` while it runs.
    pub fn from_run(run: &crate::analysis::Run) -> Option<Self> {
        if run.running {
            return None;
        }
        let when = chrono::DateTime::parse_from_rfc3339(&run.started)
            .ok()
            .map(|t| t.format("%Y-%m-%d %H:%M").to_string())
            .unwrap_or_default();
        Some(Self {
            name: run.name.clone(),
            passed: run.exit == Some(0),
            seconds: run.seconds,
            when,
        })
    }
}

impl Facts {
    pub fn load(root: &Path) -> Result<Option<Self>> {
        match std::fs::read_to_string(root.join(FILE_NAME)) {
            Ok(text) => Ok(Some(Self::parse(&text))),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e).with_context(|| format!("cannot read {FILE_NAME}")),
        }
    }

    /// Writes the file when what it would say is not what it says.
    pub fn save(&self, root: &Path) -> Result<bool> {
        let path = root.join(FILE_NAME);
        let text = self.render();
        if std::fs::read_to_string(&path).is_ok_and(|now| now == text) {
            return Ok(false);
        }
        std::fs::write(&path, text).with_context(|| format!("cannot write {FILE_NAME}"))?;
        Ok(true)
    }

    /// Takes over what a file carries that the tool does not work out: the About section,
    /// anything under a heading of its own, and the checks when the caller knows none.
    pub fn carry_over(&mut self, from: &Facts, checks_known: bool) {
        self.about = from.about.clone();
        self.kept = from.kept.clone();
        if !checks_known {
            self.checks = from.checks.clone();
        }
    }

    pub fn render(&self) -> String {
        let mut out = String::from("# Overview\n\n");
        match &self.commit {
            Some(c) => out.push_str(&format!(
                "As of commit {c} on {}, {}.\n",
                self.branch, self.when
            )),
            None => out.push_str(&format!("As of {}, with no commits yet.\n", self.when)),
        }
        if !self.about.is_empty() {
            out.push_str(&format!("\n## {ABOUT}\n"));
            for line in &self.about {
                out.push_str(line);
                out.push('\n');
            }
        }
        out.push_str(&format!("\n## {FACTS}\n"));
        let plural = |n: usize, one: &str| format!("{n} {one}{}", if n == 1 { "" } else { "s" });
        let mut line = format!(
            "- {}, {} in {}",
            plural(self.files, "file"),
            plural(self.tests, "test"),
            plural(self.test_files, "file")
        );
        if self.skipped > 0 {
            line.push_str(&format!(", {} marked to skip", self.skipped));
        }
        out.push_str(&line);
        out.push('\n');
        for c in &self.checks {
            out.push_str(&format!(
                "- {}: {} in {:.0} s, {}\n",
                c.name,
                if c.passed { "passed" } else { "failed" },
                c.seconds,
                c.when
            ));
        }
        out.push_str(&format!("\n## {START}\n"));
        for (path, why) in &self.entries {
            out.push_str(&format!("- {path} — {why}\n"));
        }
        out.push_str(&format!("\n## {LEANS}\n"));
        for (path, names) in &self.leans_on {
            out.push_str(&format!("- {path} — {}\n", names.join(", ")));
        }
        out.push_str(&format!("\n## {TESTS}\n"));
        for (path, tests) in &self.tests_by_file {
            out.push_str(&format!("### {path}\n"));
            for (label, skipped) in tests {
                out.push_str(&format!(
                    "- {label}{}\n",
                    if *skipped { " (skipped)" } else { "" }
                ));
            }
        }
        out.push_str(&format!("\n## {UNTESTED}\n"));
        for (path, names) in &self.untested {
            out.push_str(&format!("- {path} — {}\n", names.join(", ")));
        }
        for section in &self.kept {
            out.push('\n');
            out.push_str(section);
        }
        out
    }

    /// Reads the file back. Every line has one shape per section, and a line that has not
    /// got it is passed over rather than taken for something; a section with a heading this
    /// module does not know is kept whole.
    pub fn parse(text: &str) -> Self {
        let mut facts = Facts::default();
        let mut section: Option<String> = None;
        let mut test_file: Option<String> = None;
        let mut kept: Option<String> = None;
        for line in text.lines() {
            if let Some(rest) = line.strip_prefix("As of ") {
                facts.read_as_of(rest);
                continue;
            }
            if let Some(heading) = line.strip_prefix("## ") {
                if let Some(s) = kept.take() {
                    facts.kept.push(s);
                }
                let heading = heading.trim().to_string();
                section = Some(heading.clone());
                test_file = None;
                if !matches!(
                    heading.as_str(),
                    ABOUT | FACTS | START | LEANS | TESTS | UNTESTED
                ) {
                    kept = Some(format!("{line}\n"));
                }
                continue;
            }
            if let Some(s) = kept.as_mut() {
                s.push_str(line);
                s.push('\n');
                continue;
            }
            let Some(section) = section.as_deref() else {
                continue;
            };
            match section {
                ABOUT => facts.about.push(line.to_string()),
                FACTS => facts.read_fact(line),
                START => {
                    if let Some((path, why)) = split_dash(line) {
                        facts.entries.push((path, why));
                    }
                }
                LEANS => {
                    if let Some((path, names)) = split_dash(line) {
                        facts.leans_on.push((path, split_names(&names)));
                    }
                }
                UNTESTED => {
                    if let Some((path, names)) = split_dash(line) {
                        facts.untested.push((path, split_names(&names)));
                    }
                }
                TESTS => {
                    if let Some(path) = line.strip_prefix("### ") {
                        test_file = Some(path.trim().to_string());
                        facts
                            .tests_by_file
                            .push((path.trim().to_string(), Vec::new()));
                    } else if let (Some(_), Some(label)) = (&test_file, line.strip_prefix("- ")) {
                        let (label, skipped) = match label.trim_end().strip_suffix(" (skipped)") {
                            Some(l) => (l, true),
                            None => (label.trim_end(), false),
                        };
                        if let Some((_, tests)) = facts.tests_by_file.last_mut() {
                            tests.push((label.to_string(), skipped));
                        }
                    }
                }
                _ => {}
            }
        }
        if let Some(s) = kept.take() {
            facts.kept.push(s);
        }
        // Blank lines at the About section's ends are the file's spacing, not its words.
        while facts.about.last().is_some_and(|l| l.trim().is_empty()) {
            facts.about.pop();
        }
        while facts.about.first().is_some_and(|l| l.trim().is_empty()) {
            facts.about.remove(0);
        }
        facts.tests = facts.tests_by_file.iter().map(|(_, t)| t.len()).sum();
        facts.test_files = facts.tests_by_file.len();
        facts.skipped = facts
            .tests_by_file
            .iter()
            .flat_map(|(_, t)| t)
            .filter(|(_, s)| *s)
            .count();
        for s in &mut facts.kept {
            *s = s.trim_end().to_string() + "\n";
        }
        facts
    }

    /// `commit 3f2a9c1 on main, 2026-09-25 10:12.` or `2026-09-25 10:12, with no commits yet.`
    fn read_as_of(&mut self, rest: &str) {
        let rest = rest.trim_end_matches('.');
        if let Some(rest) = rest.strip_prefix("commit ") {
            let (commit, rest) = rest.split_once(" on ").unwrap_or((rest, ""));
            self.commit = Some(commit.to_string());
            if let Some((branch, when)) = rest.rsplit_once(", ") {
                self.branch = branch.to_string();
                self.when = when.to_string();
            }
        } else if let Some((when, _)) = rest.split_once(", ") {
            self.when = when.to_string();
        }
    }

    /// `- 70 files, 133 tests in 27 files, 2 marked to skip` or `- make check: passed in 41 s,
    /// 2026-09-25 10:12`. The test counts are re-derived from the tests section anyway.
    fn read_fact(&mut self, line: &str) {
        let Some(line) = line.strip_prefix("- ") else {
            return;
        };
        if let Some((name, rest)) = line.split_once(": ") {
            let Some((verdict, rest)) = rest.split_once(" in ") else {
                return;
            };
            let Some((seconds, when)) = rest.split_once(" s, ") else {
                return;
            };
            if let Ok(seconds) = seconds.trim().parse::<f64>() {
                self.checks.push(CheckFact {
                    name: name.to_string(),
                    passed: verdict == "passed",
                    seconds,
                    when: when.trim().to_string(),
                });
            }
        } else if let Some((files, _)) = line.split_once(" file") {
            self.files = files.trim().parse().unwrap_or(0);
        }
    }
}

/// `- path — rest` into its two parts.
fn split_dash(line: &str) -> Option<(String, String)> {
    let line = line.strip_prefix("- ")?;
    let (path, rest) = line.split_once(" — ")?;
    Some((path.trim().to_string(), rest.trim().to_string()))
}

fn split_names(names: &str) -> Vec<String> {
    names
        .split(", ")
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .map(str::to_string)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Facts {
        Facts {
            commit: Some("3f2a9c1".into()),
            branch: "main".into(),
            when: "2026-09-25 10:12".into(),
            files: 70,
            tests: 3,
            test_files: 2,
            skipped: 1,
            checks: vec![CheckFact {
                name: "make check".into(),
                passed: true,
                seconds: 41.0,
                when: "2026-09-25 10:12".into(),
            }],
            entries: vec![("src/main.rs".into(), "the command line".into())],
            leans_on: vec![(
                "src/session.rs".into(),
                vec!["Session".into(), "DiffTarget".into()],
            )],
            tests_by_file: vec![
                (
                    "src/analysis.rs".into(),
                    vec![
                        ("a command runs and says how it went".into(), false),
                        ("behind nginx".into(), true),
                    ],
                ),
                (
                    "tests/browser.rs".into(),
                    vec![("the phone layout works".into(), false)],
                ),
            ],
            untested: vec![("src/web/api.rs".into(), vec!["get_analysis".into()])],
            about: vec![
                "A tool for reviewing.".into(),
                "".into(),
                "Read src/main.rs first.".into(),
            ],
            kept: vec!["## Decisions\n- keep git\n".into()],
        }
    }

    /// Written and read back, the file is the same facts; written again, the same bytes.
    #[test]
    fn the_file_round_trips() {
        let facts = sample();
        let text = facts.render();
        assert!(
            text.contains("As of commit 3f2a9c1 on main, 2026-09-25 10:12."),
            "{text}"
        );
        assert!(
            text.contains("- 70 files, 3 tests in 2 files, 1 marked to skip"),
            "{text}"
        );
        assert!(
            text.contains("- make check: passed in 41 s, 2026-09-25 10:12"),
            "{text}"
        );
        assert!(text.contains("- behind nginx (skipped)"), "{text}");
        assert!(text.ends_with("## Decisions\n- keep git\n"), "{text}");
        let back = Facts::parse(&text);
        assert_eq!(back, facts);
        assert_eq!(back.render(), text);
    }

    /// What people write stays, and what the tool works out is replaced.
    #[test]
    fn people_keep_their_sections_and_the_tool_keeps_its_own() {
        let dir = tempfile::tempdir().unwrap();
        let mut old = sample();
        old.about = vec!["Written by hand.".into()];
        assert!(old.save(dir.path()).unwrap());
        assert!(
            !old.save(dir.path()).unwrap(),
            "nothing changed, nothing written"
        );

        let mut fresh = Facts {
            files: 71,
            ..Facts::default()
        };
        fresh.carry_over(&Facts::load(dir.path()).unwrap().unwrap(), false);
        assert_eq!(fresh.about, vec!["Written by hand.".to_string()]);
        assert_eq!(fresh.kept, old.kept);
        assert_eq!(
            fresh.checks, old.checks,
            "no check results known: the file's stay"
        );
        assert!(fresh.save(dir.path()).unwrap());
        let text = std::fs::read_to_string(dir.path().join(FILE_NAME)).unwrap();
        assert!(
            text.contains("- 71 files") && text.contains("Written by hand."),
            "{text}"
        );
        assert!(
            !text.contains("src/main.rs"),
            "the tool's sections are the tool's: {text}"
        );
    }

    #[test]
    fn a_repository_without_commits_says_so() {
        let facts = Facts {
            when: "2026-09-25 10:12".into(),
            ..Facts::default()
        };
        let text = facts.render();
        assert!(
            text.contains("As of 2026-09-25 10:12, with no commits yet."),
            "{text}"
        );
        assert_eq!(Facts::parse(&text), facts);
    }
}
