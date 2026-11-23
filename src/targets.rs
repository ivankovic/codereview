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

//! What can be reviewed, and against what: the two halves of picking a review range that is
//! not one of the stretches. The head is a worktree as it stands on disk, a local or a remote
//! branch, or a tag; a pull request is a head and a base already and is the forge's business.
//! Each head comes with the bases worth comparing it against, the likeliest first. Both front
//! ends offer the same choices from here.

use std::path::PathBuf;

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

use crate::repo::{Repo, Worktree};
use crate::spec::Stretch;

/// Something to review.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum Head {
    /// A worktree as it is on disk, uncommitted work and untracked files included.
    Worktree {
        path: PathBuf,
        name: String,
    },
    Branch {
        name: String,
    },
    RemoteBranch {
        name: String,
    },
    Tag {
        name: String,
    },
}

impl Head {
    /// What a picker calls it.
    pub fn label(&self) -> String {
        match self {
            Head::Worktree { name, .. } => format!("worktree {name}"),
            Head::Branch { name } | Head::RemoteBranch { name } | Head::Tag { name } => {
                name.clone()
            }
        }
    }
}

/// Every head the repository offers, by kind, each kind most recent first.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Heads {
    /// Every worktree, this one included, with what it has checked out.
    pub worktrees: Vec<(Head, Worktree)>,
    pub branches: Vec<Head>,
    pub remote_branches: Vec<Head>,
    pub tags: Vec<Head>,
}

/// The heads of `repo`: its worktrees, branches, remote branches and the last tags.
pub fn heads(repo: &Repo) -> Result<Heads> {
    let worktrees = repo
        .worktrees()?
        .into_iter()
        .filter(|w| !w.head.is_empty())
        .map(|w| {
            let name = w
                .path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| w.path.display().to_string());
            (
                Head::Worktree {
                    path: w.path.clone(),
                    name,
                },
                w,
            )
        })
        .collect();
    let branches = repo
        .branches()?
        .into_iter()
        .map(|b| Head::Branch { name: b.name })
        .collect();
    let remote_branches = repo
        .remote_branches()?
        .into_iter()
        .map(|b| Head::RemoteBranch { name: b.name })
        .collect();
    let tags = repo
        .tags(30)?
        .into_iter()
        .map(|name| Head::Tag { name })
        .collect();
    Ok(Heads {
        worktrees,
        branches,
        remote_branches,
        tags,
    })
}

/// Something to compare a head against: a revision, and what to call it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Base {
    pub rev: String,
    pub label: String,
    /// Why it is offered first, when it is.
    pub why: Option<String>,
}

/// What `head` is worth comparing against, the likeliest first: for a worktree, its own last
/// commit, which leaves its uncommitted work alone in the diff; then the default branch, the
/// one checked out here, and every other local branch and the last tags. The head itself is
/// left out.
pub fn bases(repo: &Repo, head: &Head) -> Result<Vec<Base>> {
    let mut out: Vec<Base> = Vec::new();
    let mut push = |rev: String, label: String, why: Option<&str>| {
        if !out.iter().any(|b| b.rev == rev) {
            out.push(Base {
                rev,
                label,
                why: why.map(str::to_string),
            });
        }
    };
    let own = match head {
        Head::Worktree { path, .. } => {
            let w = repo.worktrees()?.into_iter().find(|w| &w.path == path);
            if let Some(w) = &w {
                let what = w.branch.clone().unwrap_or_else(|| w.head[..7].to_string());
                push(
                    w.head.clone(),
                    format!("its last commit, {what}"),
                    Some("its uncommitted work alone"),
                );
            }
            w.and_then(|w| w.branch)
        }
        Head::Branch { name } | Head::RemoteBranch { name } | Head::Tag { name } => {
            Some(name.clone())
        }
    };
    if let Some(default) = repo.default_branch()
        && own.as_deref() != Some(default.as_str())
    {
        push(
            default.clone(),
            default,
            Some("the default branch, where pull requests go"),
        );
    }
    let current = repo.head_label();
    for b in repo.branches()? {
        if own.as_deref() == Some(b.name.as_str()) {
            continue;
        }
        let why = (b.name == current).then_some("checked out here");
        push(b.name.clone(), b.name, why);
    }
    for t in repo.tags(30)? {
        if own.as_deref() != Some(t.as_str()) {
            push(t.clone(), t, None);
        }
    }
    Ok(out)
}

/// `head` against `base` as a review range: what the head has that the base does not, from
/// where they parted. A worktree is taken as it is on disk now, as a snapshot commit.
pub fn range(repo: &Repo, head: &Head, base: &str) -> Result<Stretch> {
    if base.is_empty() {
        bail!("nothing to compare with");
    }
    let (rev, label) = match head {
        Head::Worktree { path, name } => {
            let snap = repo.snapshot(path)?;
            let w = repo.worktrees()?.into_iter().find(|w| &w.path == path);
            let uncommitted = w.is_some_and(|w| w.head != snap);
            let label = if uncommitted {
                format!("worktree {name}, uncommitted")
            } else {
                format!("worktree {name}")
            };
            (snap, Some(label))
        }
        Head::Branch { name } | Head::RemoteBranch { name } | Head::Tag { name } => {
            (name.clone(), None)
        }
    };
    Ok(Stretch::Range {
        base: base.to_string(),
        head: rev,
        pull: None,
        label,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn git(dir: &std::path::Path, args: &[&str]) -> String {
        let out = Command::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .unwrap();
        assert!(out.status.success(), "{args:?}: {out:?}");
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    /// A repository on `main`, a branch `feature`, and a worktree of `feature` with work in it
    /// that is not committed, one file changed and one new.
    fn repository() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let main = dir.path().join("main");
        std::fs::create_dir(&main).unwrap();
        git(&main, &["init", "-q", "-b", "main"]);
        git(&main, &["config", "user.name", "T"]);
        git(&main, &["config", "user.email", "t@example.com"]);
        std::fs::write(main.join("a.txt"), "a\n").unwrap();
        git(&main, &["add", "."]);
        git(&main, &["commit", "-q", "-m", "first"]);
        git(&main, &["tag", "v1"]);
        let wt = dir.path().join("agent");
        git(
            &main,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "feature",
                wt.to_str().unwrap(),
            ],
        );
        std::fs::write(wt.join("a.txt"), "a\nb\n").unwrap();
        git(&wt, &["commit", "-q", "-am", "on the feature"]);
        std::fs::write(wt.join("a.txt"), "a\nb\nc\n").unwrap();
        std::fs::write(wt.join("new.txt"), "new\n").unwrap();
        (dir, wt)
    }

    #[test]
    fn every_head_is_offered_worktrees_first() {
        let (dir, _) = repository();
        let repo = Repo::open(&dir.path().join("main")).unwrap();
        let heads = heads(&repo).unwrap();
        let names: Vec<_> = heads.worktrees.iter().map(|(h, _)| h.label()).collect();
        assert_eq!(names, ["worktree main", "worktree agent"]);
        assert!(heads.worktrees[0].1.current);
        assert_eq!(heads.worktrees[1].1.branch.as_deref(), Some("feature"));
        let branches: Vec<_> = heads.branches.iter().map(Head::label).collect();
        assert!(branches.contains(&"feature".to_string()), "{branches:?}");
        assert_eq!(heads.tags, [Head::Tag { name: "v1".into() }]);
    }

    #[test]
    fn a_worktree_is_compared_with_its_own_commit_first() {
        let (dir, wt) = repository();
        let repo = Repo::open(&dir.path().join("main")).unwrap();
        let head = Head::Worktree {
            path: wt.clone(),
            name: "agent".into(),
        };
        let bases = bases(&repo, &head).unwrap();
        assert!(
            bases[0].label.starts_with("its last commit, feature"),
            "{bases:?}"
        );
        assert_eq!(bases[1].rev, "main", "then the default branch: {bases:?}");
        assert!(
            !bases.iter().any(|b| b.rev == "feature"),
            "not itself: {bases:?}"
        );
    }

    /// The snapshot is the worktree on disk, untracked files too, and taking it moves
    /// nothing: not the worktree's index, not its files, not a branch.
    #[test]
    fn a_worktree_s_uncommitted_work_is_reviewed_without_touching_it() {
        let (dir, wt) = repository();
        let main = dir.path().join("main");
        let repo = Repo::open(&main).unwrap();
        let status_before = git(&wt, &["status", "--porcelain"]);
        let refs_before = git(&main, &["for-each-ref"]);
        let head = Head::Worktree {
            path: wt.clone(),
            name: "agent".into(),
        };
        let feature = git(&main, &["rev-parse", "feature"]);

        let only_uncommitted = range(&repo, &head, &feature).unwrap();
        let Stretch::Range {
            head: snap, label, ..
        } = &only_uncommitted
        else {
            panic!("a range");
        };
        assert_eq!(label.as_deref(), Some("worktree agent, uncommitted"));
        let files = repo.paths_between(Some(&feature), Some(snap)).unwrap();
        assert_eq!(files, ["a.txt", "new.txt"], "the untracked file is in it");

        let against_main = range(&repo, &head, "main").unwrap();
        let Stretch::Range { head: snap2, .. } = &against_main else {
            panic!("a range");
        };
        assert_eq!(snap, snap2, "the same worktree, the same snapshot");
        assert_eq!(git(&wt, &["status", "--porcelain"]), status_before);
        assert_eq!(git(&main, &["for-each-ref"]), refs_before, "no ref moved");
    }

    /// An agent's worktree inside the checkout is not part of the checkout's own work, even
    /// where nothing ignores it.
    #[test]
    fn a_worktree_inside_another_is_not_part_of_its_snapshot() {
        let (dir, _) = repository();
        let main = dir.path().join("main");
        git(&main, &["worktree", "add", "-q", "-b", "inner", "nested"]);
        std::fs::write(main.join("mine.txt"), "mine\n").unwrap();
        let repo = Repo::open(&main).unwrap();
        let head = Head::Worktree {
            path: main.clone(),
            name: "main".into(),
        };
        let Stretch::Range { head: snap, .. } = range(&repo, &head, "main").unwrap() else {
            panic!("a range");
        };
        let files = repo.paths_between(Some("main"), Some(&snap)).unwrap();
        assert_eq!(files, ["mine.txt"]);
    }

    #[test]
    fn a_clean_worktree_is_its_commit() {
        let (dir, wt) = repository();
        git(&wt, &["add", "-A"]);
        git(&wt, &["commit", "-q", "-m", "all of it"]);
        let repo = Repo::open(&dir.path().join("main")).unwrap();
        let head = Head::Worktree {
            path: wt.clone(),
            name: "agent".into(),
        };
        let Stretch::Range { head, label, .. } = range(&repo, &head, "main").unwrap() else {
            panic!("a range");
        };
        assert_eq!(head, git(&wt, &["rev-parse", "HEAD"]));
        assert_eq!(label.as_deref(), Some("worktree agent"));
    }
}
