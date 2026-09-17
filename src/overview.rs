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
    pub added: usize,
    pub removed: usize,
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

/// The project's own words: the first README-ish file, its opening paragraphs, and its title
/// line dropped — the title is the project's name, which the page is already showing.
fn readme(session: &Session) -> Option<Readme> {
    let path = session.files.iter().find(|f| {
        let name = f.rsplit('/').next().unwrap_or(f).to_lowercase();
        !f.contains('/') && (name == "readme.md" || name == "readme" || name == "readme.rst")
    })?;
    let text = std::fs::read_to_string(session.root().join(path)).ok()?;
    let mut opening = String::new();
    for line in text.lines() {
        let line = line.trim_end();
        if opening.is_empty()
            && (line.is_empty() || line.starts_with('#') || line.starts_with("=="))
        {
            continue;
        }
        if line.is_empty() && opening.len() > 200 {
            break;
        }
        opening.push_str(line);
        opening.push('\n');
        if opening.len() > 600 {
            opening.push('…');
            break;
        }
    }
    Some(Readme {
        path: path.clone(),
        opening: opening.trim_end().to_string(),
    })
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
    biggest.sort_by_key(|b| std::cmp::Reverse(b.bytes));
    biggest.truncate(FEW);

    Ok(State {
        branch: session.repo.head_label(),
        changed: status.len(),
        last: session.repo.commit("HEAD").ok(),
        commits: activity.commits,
        authors: activity.authors,
        added: activity.added,
        removed: activity.removed,
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
        todos: session.repo.grep_count("TODO|FIXME|HACK|XXX").unwrap_or(0),
        biggest,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::tests::{scratch_repo, scratch_session};

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

        let entries: Vec<&str> = overview.entries.iter().map(|e| e.path.as_str()).collect();
        assert!(entries.contains(&"src/main.rs"), "{entries:?}");
        assert!(entries.contains(&"Makefile"), "{entries:?}");

        let state = overview.state;
        assert_eq!(state.branch, "main");
        assert!(state.changed > 0, "the scratch tree has an unstaged edit");
        assert_eq!(state.commits, 2, "both commits are from just now");
        assert_eq!(state.authors, vec!["Tester".to_string()]);
        assert!(state.added > 0, "the commits added lines");
        assert_eq!(state.todos, 2, "one TODO and one FIXME");
        assert_eq!(state.pending, 2);
        assert_eq!(state.stale, 1, "the second comment's line is nowhere");
        assert!(!state.biggest.is_empty());
        assert_eq!(state.last.unwrap().subject, "second");
    }
}
