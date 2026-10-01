//! What a repository looks like when you open it: where to start reading, and how it is doing.
//!
//! Two questions a newcomer asks and nobody answers, because the answers are scattered across
//! a README nobody links to, a `main` somewhere under `src`, and a `git log` nobody runs.
//! Everything here is read when it is asked for and kept nowhere, like the rest of the
//! server: the repository is the record.
//!
//! The expensive half — which files the rest of the tree leans on — is not here. It needs the
//! symbol index, which is built on first use and costs seconds on a cold repository, so it is
//! [`crate::symbols::Index::reach`] and is asked for separately.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::anchor::AnchorState;
use crate::repo::Commit;
use crate::session::Session;

/// How far back "lately" reaches, for the commits, the authors and the churn.
const LATELY: &str = "7.days";
/// How many of each list is worth showing at a glance.
const FEW: usize = 6;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Overview {
    /// The project's own words about itself.
    pub readme: Option<Readme>,
    /// The other documents at the root, and under `docs/`, worth knowing exist.
    pub docs: Vec<String>,
    /// Where the program starts, by the conventions of whatever it is written in.
    pub entries: Vec<EntryPoint>,
    pub state: State,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Readme {
    pub path: String,
    /// The first paragraphs, with the title line dropped: what the project says it is.
    pub opening: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntryPoint {
    pub path: String,
    /// Why this file is one, in a few words, since a list of paths explains nothing.
    pub why: String,
}

/// How the repository is doing, in the numbers a reviewer would ask for first.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct State {
    pub branch: String,
    /// Paths git calls changed, staged and unstaged and untracked together.
    pub changed: usize,
    pub last: Option<Commit>,
    pub commits: usize,
    pub authors: Vec<String>,
    pub files: usize,
    /// Comments under `Pending`, and how many of them have lost the line they were left on.
    /// Nothing else can tell you the second number, and on a tree that changes this fast it
    /// is the one that says whether the review is keeping up.
    pub pending: usize,
    pub moved: usize,
    pub stale: usize,
    /// `TODO`, `FIXME`, `HACK` and `XXX` in tracked files.
    pub todos: usize,
    /// The largest files, which is where the trouble usually is.
    pub biggest: Vec<Biggest>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Biggest {
    pub path: String,
    pub bytes: u64,
}

/// Everything that costs a git command or a file read. See the module comment for what is
/// deliberately not here.
pub fn of(session: &Session) -> Result<Overview> {
    let root = session.root();
    Ok(Overview {
        readme: readme(session),
        docs: docs(session),
        entries: entries(session),
        state: state(session, root)?,
    })
}

/// The project's own words: the first README-ish file, and the prose it opens with. `None`
/// when there is no README, or when it has no prose to open with — a README that is a title
/// and a row of badges says nothing worth a section.
fn readme(session: &Session) -> Option<Readme> {
    let path = session.files.iter().find(|f| {
        let name = f.rsplit('/').next().unwrap_or(f).to_lowercase();
        !f.contains('/') && (name == "readme.md" || name == "readme" || name == "readme.rst")
    })?;
    let text = std::fs::read_to_string(session.root().join(path)).ok()?;
    let opening = opening(&text);
    (!opening.is_empty()).then(|| Readme {
        path: path.clone(),
        opening,
    })
}

/// The first paragraphs of prose in a README, as plain text: about two hundred characters'
/// worth, cut at six hundred.
///
/// What a README opens with is often everything but prose, and all of it is for a renderer:
/// a logo in HTML, a row of badges, a table of contents that is nothing but links, a code
/// block showing how to install. None of that is left: titles, which are the project's name
/// and already on the page; lines that are only images, links or markup once those are taken
/// out; HTML comments, code blocks, tables, link definitions, and reStructuredText directives
/// with their options. What is left keeps its words and loses its markup — a link becomes its
/// text. The opening ends at the first heading after some prose, which is where the project
/// stops saying what it is and starts on something else.
fn opening(text: &str) -> String {
    use regex::Regex;
    use std::sync::OnceLock;
    static IMAGE: OnceLock<Regex> = OnceLock::new();
    static LINK: OnceLock<Regex> = OnceLock::new();
    static TAG: OnceLock<Regex> = OnceLock::new();
    static DEFINITION: OnceLock<Regex> = OnceLock::new();
    static EMPHASIS: OnceLock<Regex> = OnceLock::new();
    let image =
        IMAGE.get_or_init(|| Regex::new(r"!\[[^\]]*\](?:\([^)]*\)|\[[^\]]*\])").expect("regex"));
    let link = LINK.get_or_init(|| {
        Regex::new(r"\[([^\]]*)\](?:\([^)]*\)|\[[^\]]*\])|<(https?://[^>]+)>").expect("regex")
    });
    let tag = TAG.get_or_init(|| Regex::new(r"</?[A-Za-z][^>]*>").expect("regex"));
    // `**strong**`, `__strong__` and `*emphasis*`; not `_x_`, which is half of every
    // snake_case name, and not a `* ` that starts a list item.
    let emphasis = EMPHASIS.get_or_init(|| {
        Regex::new(r"\*\*([^*\n]+)\*\*|__([^_\n]+)__|\*([^*\s][^*\n]*)\*").expect("regex")
    });
    let definition = DEFINITION.get_or_init(|| Regex::new(r"^\s*\[[^\]]+\]:\s").expect("regex"));

    let mut paragraphs: Vec<String> = Vec::new();
    let mut current: Vec<String> = Vec::new();
    let mut fence: Option<&str> = None;
    let mut comment = false;
    let mut directive = false;
    // A paragraph as plain text, or nothing when it had nothing to say. Images go first and
    // across line breaks, since alt text is prose about a picture and wraps like prose. Then
    // what is left of a line once its links and markup are gone says whether it had anything
    // to say: a line of links is a menu, not a sentence.
    let clean = |lines: &[String]| -> Option<String> {
        let joined = lines.join("\n");
        let bare = image.replace_all(&joined, "");
        let kept: Vec<String> = bare
            .lines()
            .filter_map(|line| {
                let said = tag.replace_all(&link.replace_all(line, ""), "").to_string();
                if !said.chars().any(char::is_alphanumeric) {
                    return None;
                }
                let linked = link.replace_all(line, "${1}${2}");
                let untagged = tag.replace_all(&linked, "");
                let plain = emphasis.replace_all(&untagged, "${1}${2}${3}");
                let words = plain.split_whitespace().collect::<Vec<_>>().join(" ");
                let indent = &line[..line.len() - line.trim_start().len()];
                Some(format!("{indent}{words}"))
            })
            .collect();
        (!kept.is_empty()).then(|| kept.join("\n"))
    };
    let flush = |current: &mut Vec<String>, paragraphs: &mut Vec<String>| {
        if let Some(paragraph) = clean(current) {
            paragraphs.push(paragraph);
        }
        current.clear();
    };
    for raw in text.lines() {
        let line = raw.trim_end();
        let trimmed = line.trim_start();
        if let Some(marker) = fence {
            if trimmed.starts_with(marker) {
                fence = None;
            }
            continue;
        }
        if comment {
            comment = !line.contains("-->");
            continue;
        }
        if directive {
            if line.is_empty() || line.starts_with([' ', '\t']) {
                continue;
            }
            directive = false;
        }
        if trimmed.is_empty() {
            flush(&mut current, &mut paragraphs);
            if paragraphs.iter().map(String::len).sum::<usize>() > 200 {
                break;
            }
            continue;
        }
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            fence = Some(&trimmed[..3]);
            flush(&mut current, &mut paragraphs);
            continue;
        }
        if trimmed.starts_with("<!--") {
            comment = !line.contains("-->");
            continue;
        }
        if trimmed.starts_with(".. ") {
            directive = true;
            continue;
        }
        let underline = trimmed.len() >= 3
            && (trimmed.chars().all(|c| c == '=') || trimmed.chars().all(|c| c == '-'));
        if trimmed.starts_with('#') || underline {
            // A title underlined on the next line was taken for prose a line too early.
            if underline {
                current.pop();
            }
            flush(&mut current, &mut paragraphs);
            if !paragraphs.is_empty() {
                break;
            }
            continue;
        }
        if trimmed.starts_with('|') || definition.is_match(line) {
            continue;
        }
        current.push(line.to_string());
    }
    flush(&mut current, &mut paragraphs);

    let mut chosen: Vec<String> = Vec::new();
    for paragraph in paragraphs {
        if chosen.iter().map(String::len).sum::<usize>() > 200 {
            break;
        }
        chosen.push(paragraph);
    }
    // "A light theme is available:" introduced a screenshot, which is gone; it goes with it.
    while chosen.len() > 1 && chosen.last().is_some_and(|p| p.ends_with(':')) {
        chosen.pop();
    }
    let mut out = chosen.join("\n\n");
    if out.len() > 600 {
        let mut cut = 600;
        while !out.is_char_boundary(cut) {
            cut -= 1;
        }
        out.truncate(cut);
        out.push('…');
    }
    out
}

/// The documents beside the code: Markdown at the root and under `docs/`. The two the tool
/// owns are left out — they have views of their own.
fn docs(session: &Session) -> Vec<String> {
    session
        .files
        .iter()
        .filter(|f| {
            let name = f.rsplit('/').next().unwrap_or(f);
            let lower = name.to_lowercase();
            let interesting = !f.contains('/') || f.starts_with("docs/");
            interesting
                && lower.ends_with(".md")
                && !matches!(lower.as_str(), "review.md" | "notes.md")
                && !lower.starts_with("readme")
        })
        .take(12)
        .cloned()
        .collect()
}

/// Where the program starts, by convention. Conventions rather than cleverness: every one of
/// these is a file a person would open first in a project of that kind, and being wrong about
/// one costs a line in a list rather than a wrong answer.
fn entries(session: &Session) -> Vec<EntryPoint> {
    let has = |path: &str| session.files.iter().any(|f| f == path);
    let mut out = Vec::new();
    let mut add = |path: &str, why: &str| {
        if has(path) {
            out.push(EntryPoint {
                path: path.to_string(),
                why: why.to_string(),
            });
        }
    };

    // Rust.
    add("src/main.rs", "the binary starts here");
    add("src/lib.rs", "the library's root, and its module map");
    add("Cargo.toml", "what it is called, and what it depends on");
    // Python.
    add("__main__.py", "the package runs from here");
    add("main.py", "the program starts here");
    add("manage.py", "the Django entry point");
    add("app.py", "the application object");
    add(
        "pyproject.toml",
        "what it is called, and what it depends on",
    );
    // JavaScript and TypeScript.
    add("package.json", "its name, its scripts and its dependencies");
    add("index.js", "the entry point");
    add("index.ts", "the entry point");
    add("src/index.js", "the entry point");
    add("src/index.ts", "the entry point");
    add("src/main.ts", "the entry point");
    // Go.
    add("main.go", "the program starts here");
    add("go.mod", "the module and what it depends on");
    // The page, and the tasks.
    add("index.html", "the page");
    add("Makefile", "the tasks: build, test, install");
    add("Dockerfile", "how it is built into an image");

    // A Rust binary that is not `src/main.rs`, and a Go command: both are conventions with a
    // directory rather than a fixed path.
    for file in &session.files {
        if file.starts_with("src/bin/") && file.ends_with(".rs") {
            out.push(EntryPoint {
                path: file.clone(),
                why: "another binary".into(),
            });
        }
        if file.starts_with("cmd/") && file.ends_with("/main.go") {
            out.push(EntryPoint {
                path: file.clone(),
                why: "a command".into(),
            });
        }
    }
    out.truncate(FEW + 2);
    out
}

fn state(session: &Session, root: &Path) -> Result<State> {
    let status = session.repo.status()?;
    let activity = session.repo.activity(LATELY)?;
    let anchored = session.all_anchored();
    let pending: Vec<_> = anchored
        .iter()
        .filter(|a| a.comment.section == "Pending")
        .collect();

    let mut biggest: Vec<Biggest> = session
        .files
        .iter()
        .filter_map(|f| {
            let bytes = std::fs::metadata(root.join(f)).ok()?.len();
            Some(Biggest {
                path: f.clone(),
                bytes,
            })
        })
        .collect();
    // What the symbol index would not read is not where a TODO lives, and it is where the
    // seconds of a grep go: a fixture of millions of lines is skipped like the index skips it.
    let oversized: BTreeSet<String> = biggest
        .iter()
        .filter(|b| b.bytes > crate::symbols::MAX_FILE_BYTES)
        .map(|b| b.path.clone())
        .collect();
    let todos = session.todos(&oversized);
    biggest.sort_by_key(|b| std::cmp::Reverse(b.bytes));
    biggest.truncate(FEW);

    Ok(State {
        branch: session.repo.head_label(),
        changed: status.len(),
        last: session.repo.commit("HEAD").ok(),
        commits: activity.commits,
        authors: activity.authors,
        files: session.files.len(),
        pending: pending.len(),
        moved: pending
            .iter()
            .filter(|a| a.state == AnchorState::Moved)
            .count(),
        stale: pending
            .iter()
            .filter(|a| a.state == AnchorState::Stale)
            .count(),
        todos,
        biggest,
    })
}

/// What a TODO looks like, for `git grep -w -E`.
const TODO_PATTERN: &str = "TODO|FIXME|HACK|XXX";

/// The overview's TODO count, per file, kept between overviews so that the next one greps
/// only what moved. A working tree moves in two ways: a file is edited, which `git status`
/// names, or HEAD changes under it, which nothing names file by file. The first is counted
/// again file by file; the second starts over.
#[derive(Debug, Default, Clone)]
pub(crate) struct TodoCounts {
    /// Whether any count has been taken.
    taken: bool,
    /// The commit the counts stand on. Another commit is another working tree.
    head: Option<String>,
    /// The paths `git status` named at the last count, to count again at the next: a revert
    /// takes a file out of the status without a trace of its content having moved.
    status: BTreeSet<String>,
    /// Matches per tracked file, for the files with any.
    counts: BTreeMap<String, usize>,
}

impl TodoCounts {
    /// Brings the counts up to date with the working tree, leaving `skip` uncounted.
    pub(crate) fn update(&mut self, session: &Session, skip: &BTreeSet<String>) {
        let head = session.repo.short_hash("HEAD").ok();
        let status: BTreeSet<String> = session.status.keys().cloned().collect();
        let tracked: BTreeSet<&str> = session.files.iter().map(String::as_str).collect();
        let counted = |p: &String| tracked.contains(p.as_str()) && !skip.contains(p);
        if self.taken && head == self.head {
            let again: Vec<String> = self
                .status
                .union(&status)
                .filter(|p| counted(p))
                .cloned()
                .collect();
            if !again.is_empty() {
                let found = session
                    .repo
                    .grep_counts(TODO_PATTERN, &again)
                    .unwrap_or_default();
                for path in &again {
                    self.counts.remove(path);
                }
                self.counts.extend(found);
            }
        } else {
            let mut pathspecs = vec![".".to_string()];
            pathspecs.extend(skip.iter().map(|p| format!(":(exclude){p}")));
            self.counts = session
                .repo
                .grep_counts(TODO_PATTERN, &pathspecs)
                .unwrap_or_default();
        }
        // A file that left the tree, or grew past the line, takes its count with it.
        self.counts.retain(|p, _| counted(p));
        self.taken = true;
        self.head = head;
        self.status = status;
    }

    pub(crate) fn total(&self) -> usize {
        self.counts.values().sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::tests::{scratch_repo, scratch_session};

    /// What a README opens with, as the projects that write badges and logos write it.
    #[test]
    fn a_readme_opens_with_its_prose_not_its_badges() {
        let text = "\
<p align=\"center\"><img src=\"logo.png\" width=\"200\"></p>

![A logo, described at length, so long that its alt text
wraps onto a second line](logo.svg)

# project

[![CI](https://ci.example/badge.svg)](https://ci.example) [![crates.io](https://img.shields.io/c)](https://crates.io/c)
![License](https://img.shields.io/l)

<!-- toc -->
- [Install](#install)
- [Usage](#usage)
<!-- tocstop -->

A tool that does [the thing](https://example.com/thing) <b>well</b>,
and says so in **two** *lines*.

```sh
cargo install project
```

## Install

Not part of the opening.
";
        assert_eq!(
            opening(text),
            "A tool that does the thing well,\nand says so in two lines."
        );

        // Underlined titles, reStructuredText badges with their options, a table.
        let text = "\
project
=======

.. image:: https://ci.example/badge.svg
   :target: https://ci.example

| a | b |
|---|---|

Short.

Another paragraph, since the first was short.

A light theme is available:

![Light](light.png)
";
        assert_eq!(
            opening(text),
            "Short.\n\nAnother paragraph, since the first was short."
        );

        // Nothing but badges and a title: nothing to say.
        assert_eq!(opening("# x\n\n[![CI](https://b)](https://c)\n"), "");
        // A long opening is cut, on a character.
        let long = "é".repeat(500);
        let cut = opening(&long);
        assert!(cut.ends_with('…') && cut.len() <= 603, "{}", cut.len());
    }

    /// The TODO count follows edits, reverts and commits, greps again only the files that
    /// moved, and leaves a file the index would not read uncounted.
    #[test]
    fn todos_follow_the_tree_without_a_full_grep() {
        let dir = scratch_repo();
        let git = |args: &[&str]| {
            let out = std::process::Command::new("git")
                .arg("-C")
                .arg(dir.path())
                .args(args)
                .output()
                .unwrap();
            assert!(out.status.success(), "git {args:?}");
        };
        std::fs::write(dir.path().join("a.rs"), "// TODO one\n// FIXME two\n").unwrap();
        // A tracked file past the index's size, with a marker in it that is not counted.
        let mut big = "// HACK not counted\n".to_string();
        big.push_str(&"x".repeat(crate::symbols::MAX_FILE_BYTES as usize + 1));
        std::fs::write(dir.path().join("big.txt"), big).unwrap();
        git(&["add", "big.txt"]);
        let mut session = scratch_session(dir.path());
        assert_eq!(of(&session).unwrap().state.todos, 2);

        // An edit is counted again in that file alone: a count planted on a file the status
        // does not name survives, where a grep of the whole tree would have replaced it.
        std::fs::write(
            dir.path().join("a.rs"),
            "// TODO one\n// FIXME two\n// XXX three\n",
        )
        .unwrap();
        session.refresh().unwrap();
        let skip: BTreeSet<String> = ["big.txt".to_string()].into();
        let mut counts = TodoCounts::default();
        counts.update(&session, &skip);
        counts.counts.insert("Makefile".into(), 5);
        counts.update(&session, &skip);
        assert_eq!(counts.total(), 8, "{:?}", counts.counts);
        assert_eq!(of(&session).unwrap().state.todos, 3);

        // A revert leaves the status, and the count follows it out.
        git(&["checkout", "--", "a.rs"]);
        session.refresh().unwrap();
        assert_eq!(of(&session).unwrap().state.todos, 0);

        // A commit moves HEAD: everything is counted afresh, and the planted count is gone.
        std::fs::write(dir.path().join("a.rs"), "// TODO one\n").unwrap();
        git(&["commit", "-q", "-a", "-m", "todo"]);
        session.refresh().unwrap();
        counts.update(&session, &skip);
        assert_eq!(counts.total(), 1, "{:?}", counts.counts);
        assert_eq!(of(&session).unwrap().state.todos, 1);
    }

    /// What opening a repository says about it. `scratch_repo` leaves two commits, an
    /// unstaged edit and an untracked file.
    #[test]
    fn opening_a_repository_says_where_to_start_and_how_it_is_doing() {
        let dir = scratch_repo();
        std::fs::write(
            dir.path().join("README.md"),
            "# scratch\n\nA repository for the tests to look at.\nIt has two lines of prose.\n\nMore, after a break.\n",
        )
        .unwrap();
        std::fs::write(dir.path().join("Makefile"), "all:\n\techo hi\n").unwrap();
        std::fs::write(
            dir.path().join("a.rs"),
            "fn a() {}\n// TODO: this one\n// FIXME: and this\n",
        )
        .unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("src/main.rs"), "fn main() {}\n").unwrap();
        std::fs::write(
            dir.path().join("REVIEW.md"),
            "# Pending\n- In a.rs on line 1: why - \"fn a() {}\"\n- In a.rs on line 2: gone - \"nothing holds this\"\n",
        )
        .unwrap();
        let mut session = scratch_session(dir.path());
        session.refresh().unwrap();

        let overview = of(&session).unwrap();
        let readme = overview.readme.expect("a README was written");
        assert_eq!(readme.path, "README.md");
        assert!(
            readme.opening.starts_with("A repository for the tests"),
            "the title line is the project's name, not its description: {:?}",
            readme.opening
        );

        // A README with nothing but a title and badges has no section at all.
        std::fs::write(
            dir.path().join("README.md"),
            "# scratch\n\n[![CI](https://ci/badge.svg)](https://ci)\n",
        )
        .unwrap();
        assert!(of(&session).unwrap().readme.is_none());

        let entries: Vec<&str> = overview.entries.iter().map(|e| e.path.as_str()).collect();
        assert!(entries.contains(&"src/main.rs"), "{entries:?}");
        assert!(entries.contains(&"Makefile"), "{entries:?}");

        let state = overview.state;
        assert_eq!(state.branch, "main");
        assert!(state.changed > 0, "the scratch tree has an unstaged edit");
        assert_eq!(state.commits, 2, "both commits are from just now");
        assert_eq!(state.authors, vec!["Tester".to_string()]);
        assert_eq!(state.todos, 2, "one TODO and one FIXME");
        assert_eq!(state.pending, 2);
        assert_eq!(state.stale, 1, "the second comment's line is nowhere");
        assert!(!state.biggest.is_empty());
        assert_eq!(state.last.unwrap().subject, "second");
    }
}
