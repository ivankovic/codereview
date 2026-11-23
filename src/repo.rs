//! Git access. Everything talks to the `git` binary through [`Repo`]; nothing else in the crate
//! runs git. Paths are repository-relative with forward slashes, exactly as git prints them, so
//! they can be handed straight back to `git show <rev>:<path>`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

/// git's well-known empty tree: the base a root commit is diffed against.
pub const EMPTY_TREE: &str = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileStatus {
    Added,
    Modified,
    Deleted,
    Renamed,
    Copied,
    TypeChanged,
    Unmerged,
    Untracked,
}

impl FileStatus {
    pub fn from_letter(letter: char) -> Option<Self> {
        Some(match letter {
            'A' => Self::Added,
            'M' => Self::Modified,
            'D' => Self::Deleted,
            'R' => Self::Renamed,
            'C' => Self::Copied,
            'T' => Self::TypeChanged,
            'U' => Self::Unmerged,
            '?' => Self::Untracked,
            _ => return None,
        })
    }

    pub fn letter(self) -> char {
        match self {
            Self::Added => 'A',
            Self::Modified => 'M',
            Self::Deleted => 'D',
            Self::Renamed => 'R',
            Self::Copied => 'C',
            Self::TypeChanged => 'T',
            Self::Unmerged => 'U',
            Self::Untracked => '?',
        }
    }
}

/// One file changed by a commit, in the index, or in the working tree.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangedFile {
    pub status: FileStatus,
    pub path: String,
    /// The previous path of a rename or copy.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub old_path: Option<String>,
}

impl ChangedFile {
    pub fn label(&self) -> String {
        match &self.old_path {
            Some(old) => format!("{} {} -> {}", self.status.letter(), old, self.path),
            None => format!("{} {}", self.status.letter(), self.path),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Commit {
    pub hash: String,
    pub short: String,
    pub parents: Vec<String>,
    pub author: String,
    pub email: String,
    /// ISO 8601 with offset, as `%aI` prints it.
    pub date: String,
    pub subject: String,
    pub body: String,
}

impl Commit {
    /// `YYYY-MM-DD`.
    pub fn day(&self) -> &str {
        self.date.get(..10).unwrap_or(&self.date)
    }

    /// The revision a commit's changes are shown against: its first parent, or the empty tree
    /// for a root commit.
    pub fn base(&self) -> &str {
        self.parents
            .first()
            .map(String::as_str)
            .unwrap_or(EMPTY_TREE)
    }
}

/// What was done to a repository over some stretch of time.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Activity {
    pub commits: usize,
    /// Everyone who made one, first seen first.
    pub authors: Vec<String>,
}

/// Working tree state of one path, from `git status --porcelain`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusEntry {
    pub path: String,
    /// Index (staged) status, `None` when the index matches HEAD for this path.
    pub staged: Option<FileStatus>,
    /// Working tree status against the index, `None` when unchanged.
    pub unstaged: Option<FileStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub old_path: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Repo {
    root: PathBuf,
}

/// The directory is inside no git repository, as git says.
#[derive(Debug)]
pub struct NotARepository(pub PathBuf);

impl std::fmt::Display for NotARepository {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} is not inside a git repository", self.0.display())
    }
}

impl std::error::Error for NotARepository {}

/// A worktree of the repository: where it is, what it has checked out.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Worktree {
    pub path: PathBuf,
    /// The branch checked out there; `None` when it is detached.
    pub branch: Option<String>,
    /// Its HEAD's commit; empty before its first.
    pub head: String,
    /// Whether it is the one this repository was opened in.
    pub current: bool,
}

/// `git worktree list --porcelain`, read: a block per worktree, blank lines between.
pub fn parse_worktrees(bytes: &[u8]) -> Vec<Worktree> {
    String::from_utf8_lossy(bytes)
        .split("\n\n")
        .filter_map(|block| {
            let mut path = None;
            let mut head = String::new();
            let mut branch = None;
            for line in block.lines() {
                if let Some(p) = line.strip_prefix("worktree ") {
                    path = Some(PathBuf::from(p));
                } else if let Some(h) = line.strip_prefix("HEAD ") {
                    head = h.to_string();
                } else if let Some(b) = line.strip_prefix("branch ") {
                    branch = Some(b.trim_start_matches("refs/heads/").to_string());
                }
            }
            Some(Worktree {
                path: path?,
                branch,
                head,
                current: false,
            })
        })
        .collect()
}

/// A local branch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Branch {
    pub name: String,
    /// The commit it points at.
    pub hash: String,
    /// When that commit was made, in seconds since the epoch.
    pub time: u64,
    /// Whether it is the one checked out.
    pub current: bool,
}

impl Repo {
    /// The repository containing `from`, or an error naming the directory when it is not inside
    /// one.
    ///
    /// Git saying the directory is in no repository is [`NotARepository`], which a caller may
    /// take as a cue to look further; anything else — git missing, or refusing to run there —
    /// is said as git said it, since looking elsewhere would only hide it.
    pub fn open(from: &Path) -> Result<Self> {
        match run_git(from, &["rev-parse", "--show-toplevel"]) {
            Ok(out) => Ok(Self {
                root: PathBuf::from(String::from_utf8_lossy(&out).trim_end()),
            }),
            Err(e) if format!("{e:#}").contains("not a git repository") => {
                Err(NotARepository(from.to_path_buf()).into())
            }
            Err(e) => Err(e.context(format!("git refused to run in {}", from.display()))),
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn git(&self, args: &[&str]) -> Result<Vec<u8>> {
        run_git(&self.root, args)
    }

    /// A git command's output, for the few callers outside this module with a question of
    /// their own. `args` are fixed strings, never a path or a revision from a request.
    pub fn git_output(&self, args: &[&str]) -> Result<Vec<u8>> {
        self.git(args)
    }

    /// Refuses a revision that git would read as an option. Revisions come from the UIs and
    /// the web API, so the check lives here rather than in every caller.
    fn check_rev(rev: &str) -> Result<()> {
        if rev.starts_with('-') {
            bail!("{rev:?} is not a revision");
        }
        Ok(())
    }

    /// Refuses a path that leaves the repository: absolute, or with a `..` component.
    fn check_path(path: &str) -> Result<()> {
        let p = Path::new(path);
        if p.is_absolute()
            || p.components()
                .any(|c| matches!(c, std::path::Component::ParentDir))
        {
            bail!("{path:?} is outside the repository");
        }
        Ok(())
    }

    /// True when the repository has at least one commit. A freshly initialised repository has
    /// no HEAD to diff against, and every caller that reads `HEAD:<path>` must cope.
    pub fn has_commits(&self) -> bool {
        self.git(&["rev-parse", "--verify", "-q", "HEAD"]).is_ok()
    }

    /// `user.name` from git's config, if set.
    pub fn user_name(&self) -> Option<String> {
        let out = self.git(&["config", "user.name"]).ok()?;
        let name = String::from_utf8_lossy(&out).trim().to_string();
        (!name.is_empty()).then_some(name)
    }

    /// The name of the checked-out branch, or the short hash when detached.
    pub fn head_label(&self) -> String {
        if let Ok(out) = self.git(&["symbolic-ref", "-q", "--short", "HEAD"]) {
            return String::from_utf8_lossy(&out).trim().to_string();
        }
        match self.git(&["rev-parse", "--short", "HEAD"]) {
            Ok(out) => String::from_utf8_lossy(&out).trim().to_string(),
            Err(_) => "no commits".to_string(),
        }
    }

    /// How many commits reachable from HEAD were made after `when`; none in a repository with
    /// no commits yet. Commit times are whole seconds and `--since` includes the second it is
    /// given, so a commit in the same second as `when` is taken to have come first — which
    /// it almost always did, since a run is started after the commit it is about.
    pub fn commits_since(&self, when: std::time::SystemTime) -> Result<usize> {
        if !self.has_commits() {
            return Ok(0);
        }
        let secs = when
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let since = format!("--since=@{}", secs + 1);
        let out = self.git(&["rev-list", "--count", &since, "HEAD"])?;
        Ok(String::from_utf8_lossy(&out).trim().parse().unwrap_or(0))
    }

    /// When each commit on HEAD after `when` was made, in seconds since the epoch, newest
    /// first: one command for any number of questions about what came after a moment.
    pub fn commit_times_since(&self, when: std::time::SystemTime) -> Result<Vec<u64>> {
        if !self.has_commits() {
            return Ok(Vec::new());
        }
        let secs = when
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let since = format!("--since=@{}", secs + 1);
        let out = self.git(&["log", "--format=%ct", &since, "HEAD"])?;
        Ok(String::from_utf8_lossy(&out)
            .lines()
            .filter_map(|l| l.trim().parse().ok())
            .collect())
    }

    /// Every tracked file plus untracked files that are not ignored, sorted.
    pub fn list_files(&self) -> Result<Vec<String>> {
        let out = self.git(&[
            "ls-files",
            "-z",
            "--cached",
            "--others",
            "--exclude-standard",
        ])?;
        let mut files: Vec<String> = out
            .split(|b| *b == 0)
            .filter(|s| !s.is_empty())
            .map(|s| String::from_utf8_lossy(s).into_owned())
            .collect();
        // A symlink has no content worth reviewing, and following one would read a file
        // outside the repository. git lists them like any other path, so drop them here,
        // before they reach the tree, the symbol index or a request.
        files.retain(|f| {
            !std::fs::symlink_metadata(self.root.join(f)).is_ok_and(|m| m.file_type().is_symlink())
        });
        files.sort();
        files.dedup();
        Ok(files)
    }

    /// What has been done to the repository lately: how many commits since `since` (anything
    /// git's `--since` understands, `7.days` for one week), and who made them. Not how many
    /// lines they changed: that is `--numstat`, which diffs every one of them, and on a
    /// repository whose commits carry a corpus it took seconds for two figures nobody read.
    pub fn activity(&self, since: &str) -> Result<Activity> {
        let out = self.git(&["log", &format!("--since={since}"), "--format=%an"])?;
        let mut activity = Activity::default();
        for author in String::from_utf8_lossy(&out).lines() {
            activity.commits += 1;
            if !activity.authors.contains(&author.to_string()) {
                activity.authors.push(author.to_string());
            }
        }
        Ok(activity)
    }

    /// How many tracked lines match `pattern`, as whole words, case sensitively. git's own
    /// grep, so it searches what is tracked and skips what is ignored — which is the
    /// difference between counting a project's `TODO`s and counting its dependencies'.
    ///
    /// An unmatched pattern is not an error: git grep says so with an exit code, and "none"
    /// is the answer, not a failure.
    /// How many lines match `pattern` (extended regex, whole words) in each tracked file
    /// that has any, within `pathspecs` — every file when there are none. Files without a
    /// match are left out, as `git grep -c` leaves them out.
    pub fn grep_counts(
        &self,
        pattern: &str,
        pathspecs: &[String],
    ) -> Result<BTreeMap<String, usize>> {
        let output = git_command(&self.root)
            .args(["grep", "-c", "-w", "-E", "--", pattern])
            .args(pathspecs)
            .output()
            .context("cannot run git grep")?;
        // Exit status 1 is "nothing matched", which is a count of zero everywhere.
        if !output.status.success() {
            return Ok(BTreeMap::new());
        }
        Ok(String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter_map(|l| l.rsplit_once(':'))
            .filter_map(|(path, n)| Some((path.to_string(), n.trim().parse::<usize>().ok()?)))
            .collect())
    }

    /// Working tree and index status, keyed by path.
    pub fn status(&self) -> Result<BTreeMap<String, StatusEntry>> {
        let out = self.git(&["status", "--porcelain=v1", "-z", "--untracked-files=all"])?;
        Ok(parse_status(&out))
    }

    /// Stages the whole working tree when `all`, then commits the index with `message`, and
    /// reads back the commit it made. The repository's own hooks run, exactly as they do on
    /// the command line: a `pre-commit` that refuses stops the commit and what it printed is
    /// what comes back. git is given no terminal, so anything that would ask a question —
    /// a passphrase for a signature, a credential — fails instead of waiting forever.
    pub fn create_commit(&self, message: &str, all: bool) -> Result<Commit> {
        let message = message.trim();
        if message.is_empty() {
            bail!("a commit needs a message");
        }
        if all {
            self.git(&["add", "-A"])?;
        }
        let output = git_command(&self.root)
            .args(["commit", "-m", message])
            .output()
            .context("cannot run git commit")?;
        if !output.status.success() {
            bail!("{}", complaint(&output));
        }
        self.commit("HEAD")
    }

    /// The most recent `limit` commits reachable from `rev` (or HEAD), newest first, skipping
    /// the first `skip`. When `path` is given, only commits touching it, following renames.
    pub fn log(
        &self,
        rev: Option<&str>,
        path: Option<&str>,
        skip: usize,
        limit: usize,
    ) -> Result<Vec<Commit>> {
        if !self.has_commits() {
            return Ok(Vec::new());
        }
        let skip = skip.to_string();
        let limit = limit.to_string();
        let mut args = vec![
            "log",
            "-z",
            LOG_FORMAT,
            "--skip",
            &skip,
            "--max-count",
            &limit,
        ];
        if let Some(rev) = rev {
            Self::check_rev(rev)?;
            args.push(rev);
        }
        if let Some(path) = path {
            Self::check_path(path)?;
            args.push("--follow");
            args.push("--");
            args.push(path);
        }
        let out = self.git(&args)?;
        Ok(parse_log(&out))
    }

    /// One commit by hash or any other revision expression.
    pub fn commit(&self, rev: &str) -> Result<Commit> {
        Self::check_rev(rev)?;
        let out = self.git(&["log", "-z", LOG_FORMAT, "--max-count", "1", rev])?;
        parse_log(&out)
            .into_iter()
            .next()
            .with_context(|| format!("no commit {rev}"))
    }

    /// The files a commit changed against its first parent, with renames detected.
    pub fn commit_files(&self, hash: &str) -> Result<Vec<ChangedFile>> {
        Self::check_rev(hash)?;
        let out = self.git(&[
            "diff-tree",
            "--no-commit-id",
            "--name-status",
            "-z",
            "-r",
            "-M",
            "--root",
            hash,
        ])?;
        Ok(parse_name_status(&out))
    }

    /// Files that differ between two revisions.
    pub fn diff_files(&self, from: &str, to: &str) -> Result<Vec<ChangedFile>> {
        Self::check_rev(from)?;
        Self::check_rev(to)?;
        let out = self.git(&["diff", "--name-status", "-z", "-M", from, to])?;
        Ok(parse_name_status(&out))
    }

    /// The lines each file has that `from` did not, by path: what a stretch of history added
    /// or changed, counted in `to` (the working tree when `None`, untracked files included).
    /// Lines only removed leave nothing to count.
    pub fn changed_lines(
        &self,
        from: &str,
        to: Option<&str>,
    ) -> Result<std::collections::BTreeMap<String, Vec<u32>>> {
        Self::check_rev(from)?;
        let mut args = vec![
            "diff",
            "-U0",
            "--no-color",
            "--no-ext-diff",
            "--no-renames",
            from,
        ];
        if let Some(to) = to {
            Self::check_rev(to)?;
            args.push(to);
        }
        let out = self.git(&args)?;
        let mut lines = parse_unified_zero(&String::from_utf8_lossy(&out));
        if to.is_none() {
            for (path, entry) in self.status()? {
                if entry.unstaged != Some(FileStatus::Untracked) {
                    continue;
                }
                let Ok(Some(bytes)) = self.read_working(&path) else {
                    continue;
                };
                let count = bytes.iter().filter(|b| **b == b'\n').count() as u32
                    + u32::from(!bytes.is_empty() && !bytes.ends_with(b"\n"));
                if count > 0 {
                    lines.insert(path, (1..=count).collect());
                }
            }
        }
        Ok(lines)
    }

    /// The full hash of the commit `rev` names.
    pub fn resolve(&self, rev: &str) -> Result<String> {
        Self::check_rev(rev)?;
        let spec = format!("{rev}^{{commit}}");
        let out = self.git(&["rev-parse", "--verify", &spec])?;
        Ok(String::from_utf8_lossy(&out).trim().to_string())
    }

    /// Where git keeps what all of this repository's worktrees share.
    pub fn common_dir(&self) -> Result<PathBuf> {
        let out = self.git(&["rev-parse", "--git-common-dir"])?;
        let dir = PathBuf::from(String::from_utf8_lossy(&out).trim());
        Ok(if dir.is_absolute() {
            dir
        } else {
            self.root.join(dir)
        })
    }

    /// A detached worktree of `commit` at `path`, made unless one is already there.
    pub fn worktree(&self, commit: &str, path: &Path) -> Result<()> {
        Self::check_rev(commit)?;
        if path.join(".git").exists() {
            return Ok(());
        }
        let path = path
            .to_str()
            .with_context(|| format!("{} is not a path git can take", path.display()))?;
        self.git(&["worktree", "add", "--detach", "--force", path, commit])?;
        Ok(())
    }

    /// Removes the worktree at `path`, whatever is in it.
    pub fn remove_worktree(&self, path: &Path) -> Result<()> {
        let path = path
            .to_str()
            .with_context(|| format!("{} is not a path git can take", path.display()))?;
        self.git(&["worktree", "remove", "--force", path])?;
        Ok(())
    }

    /// The first parent of `rev`, or `None` for a root commit.
    pub fn parent(&self, rev: &str) -> Result<Option<String>> {
        Self::check_rev(rev)?;
        let spec = format!("{rev}^{{commit}}");
        self.git(&["rev-parse", "--verify", "-q", &spec])?;
        let parent = format!("{rev}^");
        Ok(self
            .git(&["rev-parse", "--verify", "-q", &parent])
            .ok()
            .map(|out| String::from_utf8_lossy(&out).trim().to_string()))
    }

    /// The newest commit reachable from HEAD made before `when`, or `None` when every commit
    /// is newer, or there are none.
    pub fn last_before(&self, when: std::time::SystemTime) -> Option<String> {
        if !self.has_commits() {
            return None;
        }
        let secs = when
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let before = format!("--before=@{secs}");
        let out = self
            .git(&["rev-list", "--max-count=1", &before, "HEAD"])
            .ok()?;
        let hash = String::from_utf8_lossy(&out).trim().to_string();
        (!hash.is_empty()).then_some(hash)
    }

    /// The paths that differ between `from` and `to`, renames taken apart into the path that
    /// went and the path that came. `None` for `from` is the empty tree, before the first
    /// commit; `None` for `to` is the working tree, untracked files included.
    pub fn paths_between(&self, from: Option<&str>, to: Option<&str>) -> Result<Vec<String>> {
        let from = from.unwrap_or(EMPTY_TREE);
        Self::check_rev(from)?;
        let mut args = vec!["diff", "--name-only", "-z", "--no-renames", from];
        if let Some(to) = to {
            Self::check_rev(to)?;
            args.push(to);
        }
        let out = self.git(&args)?;
        let mut paths: Vec<String> = out
            .split(|b| *b == 0)
            .filter(|p| !p.is_empty())
            .map(|p| String::from_utf8_lossy(p).into_owned())
            .collect();
        if to.is_none() {
            let untracked = self.git(&["ls-files", "-z", "--others", "--exclude-standard"])?;
            paths.extend(
                untracked
                    .split(|b| *b == 0)
                    .filter(|p| !p.is_empty())
                    .map(|p| String::from_utf8_lossy(p).into_owned()),
            );
        }
        paths.sort();
        paths.dedup();
        Ok(paths)
    }

    /// `rev` as a commit, abbreviated the way git abbreviates it: unambiguous here, now.
    pub fn short_hash(&self, rev: &str) -> Result<String> {
        Self::check_rev(rev)?;
        let spec = format!("{rev}^{{commit}}");
        let out = self.git(&["rev-parse", "--short", "--verify", &spec])?;
        Ok(String::from_utf8_lossy(&out).trim().to_string())
    }

    /// The names at the top of the tree at `rev`: where a licence file is.
    pub fn top_level(&self, rev: &str) -> Result<Vec<String>> {
        Self::check_rev(rev)?;
        let out = self.git(&["ls-tree", "--name-only", rev])?;
        Ok(String::from_utf8_lossy(&out)
            .lines()
            .map(str::to_string)
            .collect())
    }

    /// The `owner/name` of the GitHub repository `origin` points at, if it points at one.
    pub fn github_name(&self) -> Option<String> {
        github_name(&self.remote_url()?)
    }

    /// The repository's address as the breadcrumb shows it: where `origin` points, without the
    /// scheme, or the checkout's path with the home directory as `~`.
    pub fn address(&self) -> String {
        self.remote_url()
            .map(|u| short_address(&u))
            .unwrap_or_else(|| tilde(&self.root))
    }

    /// Where `origin` points, as git has it: the repository's address, for the breadcrumb.
    pub fn remote_url(&self) -> Option<String> {
        let out = self.git(&["remote", "get-url", "origin"]).ok()?;
        let url = String::from_utf8_lossy(&out).trim().to_string();
        (!url.is_empty()).then_some(url)
    }

    /// The local branches, the checked-out one marked, most recently committed to first.
    pub fn branches(&self) -> Result<Vec<Branch>> {
        let out = self.git(&[
            "for-each-ref",
            "--sort=-committerdate",
            "--format=%(HEAD)%00%(refname:short)%00%(objectname)%00%(committerdate:unix)",
            "refs/heads",
        ])?;
        Ok(parse_branches(&out))
    }

    /// Every worktree of this repository, the main checkout first, as git lists them.
    pub fn worktrees(&self) -> Result<Vec<Worktree>> {
        let out = self.git(&["worktree", "list", "--porcelain"])?;
        let here = self
            .root
            .canonicalize()
            .unwrap_or_else(|_| self.root.clone());
        let mut list = parse_worktrees(&out);
        for w in &mut list {
            w.current = w.path.canonicalize().unwrap_or_else(|_| w.path.clone()) == here;
        }
        Ok(list)
    }

    /// The remote-tracking branches, most recently committed to first, without the `HEAD`
    /// each remote has pointing at one of them.
    pub fn remote_branches(&self) -> Result<Vec<Branch>> {
        let out = self.git(&[
            "for-each-ref",
            "--sort=-committerdate",
            "--format=%(HEAD)%00%(refname:short)%00%(objectname)%00%(committerdate:unix)",
            "refs/remotes",
        ])?;
        Ok(parse_branches(&out)
            .into_iter()
            .filter(|b| !b.name.ends_with("/HEAD") && b.name.contains('/'))
            .collect())
    }

    /// The tags, most recent first, at most `limit`.
    pub fn tags(&self, limit: usize) -> Result<Vec<String>> {
        let count = format!("--count={limit}");
        let out = self.git(&[
            "for-each-ref",
            "--sort=-creatordate",
            &count,
            "--format=%(refname:short)",
            "refs/tags",
        ])?;
        Ok(String::from_utf8_lossy(&out)
            .lines()
            .filter(|t| !t.is_empty())
            .map(str::to_string)
            .collect())
    }

    /// The worktree at `path` as it is on disk, uncommitted work and untracked files
    /// included, as a commit whose parent is its HEAD: something a diff can be taken against
    /// like any commit. It is made in the object store alone, through an index of its own, so
    /// the worktree, its index and every branch are left as they were; with nothing
    /// uncommitted it is HEAD itself.
    pub fn snapshot(&self, path: &Path) -> Result<String> {
        let wt = Repo::open(path)?;
        if wt.common_dir()?.canonicalize()? != self.common_dir()?.canonicalize()? {
            bail!("{} is not a worktree of this repository", path.display());
        }
        let head = wt.resolve("HEAD")?;
        // Dated as its HEAD is, so that the same work on the same commit is the same snapshot
        // whenever it is taken, and a page asking again finds what it found before.
        let when = format!(
            "@{} +0000",
            String::from_utf8_lossy(&wt.git(&["log", "-1", "--format=%ct", "HEAD"])?).trim()
        );
        let scratch = self.common_dir()?.join("codereview");
        std::fs::create_dir_all(&scratch)
            .with_context(|| format!("cannot make {}", scratch.display()))?;
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let index = scratch.join(format!("snapshot-index-{}-{nanos}", std::process::id()));
        let with_index = |args: &[&str]| -> Result<Vec<u8>> {
            let output = git_command(&wt.root)
                .args(args)
                .env("GIT_INDEX_FILE", &index)
                .env("GIT_AUTHOR_NAME", "codereview")
                .env("GIT_AUTHOR_EMAIL", "codereview@localhost")
                .env("GIT_COMMITTER_NAME", "codereview")
                .env("GIT_COMMITTER_EMAIL", "codereview@localhost")
                .env("GIT_AUTHOR_DATE", &when)
                .env("GIT_COMMITTER_DATE", &when)
                .output()
                .with_context(|| format!("cannot run git {}", args.join(" ")))?;
            if !output.status.success() {
                bail!("git {} failed: {}", args.join(" "), said(&output.stderr));
            }
            Ok(output.stdout)
        };
        // Another worktree inside this one (an agent's, under `.claude/worktrees`, say) is
        // a repository of its own, not part of this one's work, whatever `.gitignore` says.
        let nested: Vec<String> = self
            .worktrees()?
            .into_iter()
            .filter_map(|w| {
                let rel = w.path.strip_prefix(&wt.root).ok()?;
                (!rel.as_os_str().is_empty()).then(|| format!(":(exclude){}", rel.display()))
            })
            .collect();
        let made = (|| -> Result<String> {
            with_index(&["read-tree", "HEAD"])?;
            let mut add = vec!["add", "-A", "--", "."];
            add.extend(nested.iter().map(String::as_str));
            with_index(&add)?;
            let tree = String::from_utf8_lossy(&with_index(&["write-tree"])?)
                .trim()
                .to_string();
            if tree == wt.resolve_tree("HEAD")? {
                return Ok(head.clone());
            }
            let message = format!("uncommitted work in {}", path.display());
            let commit = with_index(&["commit-tree", &tree, "-p", &head, "-m", &message])?;
            Ok(String::from_utf8_lossy(&commit).trim().to_string())
        })();
        let _ = std::fs::remove_file(&index);
        made
    }

    fn resolve_tree(&self, rev: &str) -> Result<String> {
        Self::check_rev(rev)?;
        let spec = format!("{rev}^{{tree}}");
        let out = self.git(&["rev-parse", "--verify", &spec])?;
        Ok(String::from_utf8_lossy(&out).trim().to_string())
    }

    /// The branch pull requests usually go into: the one `origin/HEAD` names, else `main` or
    /// `master` where there is one.
    pub fn default_branch(&self) -> Option<String> {
        if let Ok(out) = self.git(&["symbolic-ref", "-q", "--short", "refs/remotes/origin/HEAD"]) {
            let name = String::from_utf8_lossy(&out).trim().to_string();
            if let Some(branch) = name.strip_prefix("origin/") {
                return Some(branch.to_string());
            }
        }
        ["main", "master"]
            .into_iter()
            .find(|b| {
                self.git(&["rev-parse", "--verify", "-q", &format!("refs/heads/{b}")])
                    .is_ok()
            })
            .map(str::to_string)
    }

    /// The commit `a` and `b` last had in common, where a pull request from `b` into `a` starts.
    pub fn merge_base(&self, a: &str, b: &str) -> Result<Option<String>> {
        Self::check_rev(a)?;
        Self::check_rev(b)?;
        Ok(self
            .git(&["merge-base", a, b])
            .ok()
            .map(|out| String::from_utf8_lossy(&out).trim().to_string())
            .filter(|hash| !hash.is_empty()))
    }

    /// Whether the commit `hash` is in this repository's object store.
    pub fn has_commit(&self, hash: &str) -> bool {
        Self::check_rev(hash).is_ok()
            && self
                .git(&["cat-file", "-e", &format!("{hash}^{{commit}}")])
                .is_ok()
    }

    /// Fetches `refspecs` from `origin` into the object store only: no branch or remote ref
    /// moves, so a pull request's commits can be read without changing what the user has.
    pub fn fetch_objects(&self, refspecs: &[&str]) -> Result<()> {
        for spec in refspecs {
            Self::check_rev(spec)?;
            if spec.contains(':') || spec.starts_with('+') {
                bail!("{spec:?} would write a ref");
            }
        }
        let mut args = vec![
            "fetch",
            "--quiet",
            "--refmap=",
            "--no-tags",
            "--no-write-fetch-head",
            "origin",
        ];
        args.extend(refspecs);
        self.git(&args)?;
        Ok(())
    }

    /// Whether `path` is in the commit HEAD names and the working tree holds it unchanged: the
    /// one case where a line number in the file on disk is also one in a commit.
    pub fn committed_as_is(&self, path: &str) -> bool {
        if Self::check_path(path).is_err() || !self.has_commits() {
            return false;
        }
        let spec = format!("HEAD:{path}");
        let in_head = self.git(&["cat-file", "-e", &spec]).is_ok();
        let unchanged = self
            .git(&["status", "--porcelain=v1", "-z", "--", path])
            .is_ok_and(|out| out.is_empty());
        in_head && unchanged
    }

    /// The content of `path` at `rev`, or `None` when it does not exist there. `rev` may be
    /// `""` for the index (`git show :path`).
    pub fn show(&self, rev: &str, path: &str) -> Result<Option<Vec<u8>>> {
        Self::check_rev(rev)?;
        Self::check_path(path)?;
        let spec = format!("{rev}:{path}");
        let output = git_command(&self.root)
            .args(["show", &spec])
            .output()
            .context("cannot run git show")?;
        if output.status.success() {
            Ok(Some(output.stdout))
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            if stderr.contains("does not exist")
                || stderr.contains("exists on disk, but not in")
                || stderr.contains("bad revision")
                || stderr.contains("invalid object name")
                || stderr.contains("Path '")
            {
                Ok(None)
            } else {
                bail!(
                    "git show {spec}: {}",
                    stderr.trim().lines().next().unwrap_or("failed")
                )
            }
        }
    }

    /// Where `path` really is in the working tree, or `None` when nothing is there.
    /// Resolving before reading is what keeps a symlink in the checkout from being used to
    /// read a file elsewhere on the machine: `check_path` only rules out `..` and absolute
    /// paths, which a link does not need.
    pub fn working_path(&self, path: &str) -> Result<Option<PathBuf>> {
        Self::check_path(path)?;
        let full = self.root.join(path);
        match std::fs::canonicalize(&full) {
            Ok(real) => {
                let root = self
                    .root
                    .canonicalize()
                    .unwrap_or_else(|_| self.root.clone());
                if !real.starts_with(&root) {
                    bail!("{path} leads outside the repository");
                }
                Ok(Some(real))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e).with_context(|| format!("cannot read {path}")),
        }
    }

    /// The file's content in the working tree, or `None` when it is missing.
    pub fn read_working(&self, path: &str) -> Result<Option<Vec<u8>>> {
        let Some(full) = self.working_path(path)? else {
            return Ok(None);
        };
        match std::fs::read(&full) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e).with_context(|| format!("cannot read {path}")),
        }
    }

    /// Per-line author, commit and date for `path` in the working tree.
    pub fn blame(&self, path: &str) -> Result<Vec<BlameLine>> {
        Self::check_path(path)?;
        let out = self.git(&["blame", "--porcelain", "--", path])?;
        Ok(parse_blame(&out))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlameLine {
    pub hash: String,
    pub author: String,
    /// `YYYY-MM-DD`.
    pub date: String,
    pub summary: String,
}

/// `%x1f`-separated fields, one commit per `-z` NUL. Body last, since it can contain anything.
const LOG_FORMAT: &str = "--format=%H%x1f%h%x1f%P%x1f%an%x1f%ae%x1f%aI%x1f%s%x1f%b";

/// The after-side lines of every hunk of a `git diff -U0`, by the path they are in.
fn parse_unified_zero(text: &str) -> std::collections::BTreeMap<String, Vec<u32>> {
    let mut out: std::collections::BTreeMap<String, Vec<u32>> = std::collections::BTreeMap::new();
    let mut path: Option<String> = None;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("+++ ") {
            path = rest
                .strip_prefix("b/")
                .map(|p| p.trim_end_matches('\t').to_string())
                .filter(|p| rest != "/dev/null" && !p.is_empty());
        } else if let Some(rest) = line.strip_prefix("@@ ") {
            let Some(path) = &path else {
                continue;
            };
            // `@@ -a[,b] +c[,d] @@`: d lines from c, one when d is left out.
            let Some(after) = rest.split(' ').find(|w| w.starts_with('+')) else {
                continue;
            };
            let mut parts = after[1..].split(',');
            let start: u32 = parts.next().and_then(|n| n.parse().ok()).unwrap_or(0);
            let count: u32 = parts.next().and_then(|n| n.parse().ok()).unwrap_or(1);
            if count > 0 {
                out.entry(path.clone())
                    .or_default()
                    .extend(start..start + count);
            }
        }
    }
    out
}

fn run_git(dir: &Path, args: &[&str]) -> Result<Vec<u8>> {
    let output = git_command(dir)
        .args(args)
        .output()
        .with_context(|| format!("cannot run git {}", args.join(" ")))?;
    if !output.status.success() {
        bail!("git {} failed: {}", args.join(" "), said(&output.stderr));
    }
    Ok(output.stdout)
}

/// What git said on its error output, short: the first line, which says what went wrong, and
/// the last, which is where a git, or a wrapper around it, says what to do about it.
fn said(stderr: &[u8]) -> String {
    let text = String::from_utf8_lossy(stderr);
    let lines: Vec<&str> = text
        .lines()
        .map(str::trim_end)
        .filter(|l| !l.trim().is_empty())
        .collect();
    match lines.as_slice() {
        [] => "no message".to_string(),
        [one] => one.to_string(),
        [first, .., last] => format!("{first}\n{last}"),
    }
}

/// What git said when a command failed: its complaints first, then what it printed, since a
/// hook that refuses usually says why on stdout. A few lines only, so a chatty hook cannot
/// fill the page that shows this.
fn complaint(output: &std::process::Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let lines: Vec<&str> = stderr
        .lines()
        .chain(stdout.lines())
        .map(str::trim_end)
        .filter(|l| !l.trim().is_empty())
        .take(5)
        .collect();
    if lines.is_empty() {
        return "git commit failed and said nothing".to_string();
    }
    lines.join("\n")
}

/// An address as the breadcrumb shows it: `github.com/o/n` for any of git's ways of writing
/// one, the path for a local remote.
pub fn short_address(url: &str) -> String {
    let rest = url
        .split_once("://")
        .map(|(_, r)| r)
        .unwrap_or(url)
        .trim_start_matches("git@");
    let rest = rest.rsplit_once('@').map(|(_, r)| r).unwrap_or(rest);
    let rest = match rest.split_once(':') {
        Some((host, path)) if !host.contains('/') && !path.starts_with("//") => {
            format!("{host}/{path}")
        }
        _ => rest.to_string(),
    };
    rest.trim_end_matches('/')
        .trim_end_matches(".git")
        .to_string()
}

/// A path with the home directory written `~`.
pub fn tilde(path: &std::path::Path) -> String {
    match std::env::var_os("HOME").map(std::path::PathBuf::from) {
        Some(home) if path.starts_with(&home) && home.as_os_str().len() > 1 => {
            format!("~/{}", path.strip_prefix(&home).unwrap_or(path).display())
        }
        _ => path.display().to_string(),
    }
}

/// Whether git would call this a binary file: a NUL byte in the first 8 KiB, which is its
/// own rule. Text is what this tool can show, diff and index; the rest is skipped.
/// The `owner/name` a GitHub remote URL names, in any of the forms git accepts:
/// `https://github.com/o/n.git`, `git@github.com:o/n.git`, `ssh://git@github.com/o/n`.
pub fn github_name(url: &str) -> Option<String> {
    let rest = url
        .strip_prefix("git@github.com:")
        .or_else(|| url.split_once("github.com/").map(|(_, r)| r))?;
    let rest = rest.trim_end_matches('/').trim_end_matches(".git");
    let mut parts = rest.split('/');
    let (owner, name) = (parts.next()?, parts.next()?);
    (parts.next().is_none() && !owner.is_empty() && !name.is_empty())
        .then(|| format!("{owner}/{name}"))
}

pub fn is_binary(bytes: &[u8]) -> bool {
    bytes.iter().take(8192).any(|b| *b == 0)
}

/// `git -C dir`, with the messages in one language. `show` tells a missing file from a real
/// failure by what git says, and under another locale it would say it differently.
///
/// Only the messages: the rest of the locale is the user's, as it is when they run git
/// themselves. A wrapper around git that fingerprints the repository with `sort` (a trust
/// check, say) sorts by the caller's collation, and under `LC_ALL=C` it found the repository
/// changed that the same wrapper, run from the user's shell, had just trusted.
fn git_command(dir: &Path) -> Command {
    let mut command = Command::new("git");
    command.arg("-C").arg(dir);
    for (key, value) in locale_for_git(|key| std::env::var(key).ok()) {
        match value {
            Some(value) => command.env(key, value),
            None => command.env_remove(key),
        };
    }
    command
}

/// The locale variables to give git: messages in C, everything else as the user has it. An
/// `LC_ALL` would override `LC_MESSAGES`, so its value goes to `LANG` instead, which keeps the
/// collation and the rest where they were.
fn locale_for_git(get: impl Fn(&str) -> Option<String>) -> Vec<(&'static str, Option<String>)> {
    let mut vars = vec![("LC_MESSAGES", Some("C".to_string()))];
    if let Some(all) = get("LC_ALL").filter(|v| !v.is_empty()) {
        vars.push(("LC_ALL", None));
        vars.push(("LANG", Some(all)));
    }
    vars
}

pub fn parse_branches(bytes: &[u8]) -> Vec<Branch> {
    String::from_utf8_lossy(bytes)
        .lines()
        .filter_map(|line| {
            let mut fields = line.split('\0');
            let current = fields.next()? == "*";
            let name = fields.next()?.to_string();
            let hash = fields.next()?.to_string();
            let time = fields.next()?.parse().unwrap_or(0);
            Some(Branch {
                name,
                hash,
                time,
                current,
            })
        })
        .collect()
}

pub fn parse_log(bytes: &[u8]) -> Vec<Commit> {
    bytes
        .split(|b| *b == 0)
        .filter(|record| !record.is_empty())
        .filter_map(|record| {
            let text = String::from_utf8_lossy(record);
            let mut fields = text.splitn(8, '\x1f');
            let hash = fields.next()?.trim().to_string();
            let short = fields.next()?.to_string();
            let parents = fields
                .next()?
                .split_whitespace()
                .map(str::to_string)
                .collect();
            let author = fields.next()?.to_string();
            let email = fields.next()?.to_string();
            let date = fields.next()?.to_string();
            let subject = fields.next()?.to_string();
            let body = fields.next().unwrap_or("").trim_end().to_string();
            Some(Commit {
                hash,
                short,
                parents,
                author,
                email,
                date,
                subject,
                body,
            })
        })
        .collect()
}

/// `-z` name-status: `M\0path\0`, or `R100\0old\0new\0`.
pub fn parse_name_status(bytes: &[u8]) -> Vec<ChangedFile> {
    let mut fields = bytes
        .split(|b| *b == 0)
        .map(|s| String::from_utf8_lossy(s).into_owned())
        .filter(|s| !s.is_empty());
    let mut files = Vec::new();
    while let Some(status) = fields.next() {
        let Some(letter) = status.chars().next() else {
            continue;
        };
        let Some(status) = FileStatus::from_letter(letter) else {
            continue;
        };
        let Some(first) = fields.next() else {
            break;
        };
        if matches!(status, FileStatus::Renamed | FileStatus::Copied) {
            let Some(second) = fields.next() else {
                break;
            };
            files.push(ChangedFile {
                status,
                path: second,
                old_path: Some(first),
            });
        } else {
            files.push(ChangedFile {
                status,
                path: first,
                old_path: None,
            });
        }
    }
    files
}

/// `--porcelain=v1 -z`: `XY path\0`, with a rename followed by `old\0`.
pub fn parse_status(bytes: &[u8]) -> BTreeMap<String, StatusEntry> {
    let mut entries = BTreeMap::new();
    let mut fields = bytes.split(|b| *b == 0).filter(|s| !s.is_empty());
    while let Some(record) = fields.next() {
        if record.len() < 4 {
            continue;
        }
        let x = record[0] as char;
        let y = record[1] as char;
        let path = String::from_utf8_lossy(&record[3..]).into_owned();
        let old_path = if x == 'R' || x == 'C' || y == 'R' || y == 'C' {
            fields
                .next()
                .map(|s| String::from_utf8_lossy(s).into_owned())
        } else {
            None
        };
        let (staged, unstaged) = if x == '?' {
            (None, Some(FileStatus::Untracked))
        } else {
            (
                FileStatus::from_letter(x).filter(|_| x != ' '),
                FileStatus::from_letter(y).filter(|_| y != ' '),
            )
        };
        entries.insert(
            path.clone(),
            StatusEntry {
                path,
                staged,
                unstaged,
                old_path,
            },
        );
    }
    entries
}

fn parse_blame(bytes: &[u8]) -> Vec<BlameLine> {
    let text = String::from_utf8_lossy(bytes);
    // Porcelain blame gives a commit's author and summary the first time it appears and
    // only its hash afterwards, so what was said about each one has to be remembered.
    let mut commits: BTreeMap<String, BlameLine> = BTreeMap::new();
    let mut lines = Vec::new();
    let mut current: Option<String> = None;
    for line in text.lines() {
        if line.starts_with('\t') {
            if let Some(hash) = &current
                && let Some(info) = commits.get(hash)
            {
                lines.push(info.clone());
            }
            continue;
        }
        let mut words = line.splitn(2, ' ');
        let key = words.next().unwrap_or("");
        let value = words.next().unwrap_or("");
        // A line that starts with a hash begins a new commit's block. Forty hexadecimal
        // digits is SHA-1, which is what git writes here today.
        if key.len() == 40 && key.chars().all(|c| c.is_ascii_hexdigit()) {
            current = Some(key.to_string());
            commits.entry(key.to_string()).or_insert(BlameLine {
                hash: key.to_string(),
                author: String::new(),
                date: String::new(),
                summary: String::new(),
            });
            continue;
        }
        let Some(hash) = &current else {
            continue;
        };
        let Some(info) = commits.get_mut(hash) else {
            continue;
        };
        match key {
            "author" => info.author = value.to_string(),
            "author-time" => {
                if let Ok(secs) = value.parse::<i64>()
                    && let Some(dt) = chrono::DateTime::from_timestamp(secs, 0)
                {
                    info.date = dt.format("%Y-%m-%d").to_string();
                }
            }
            "summary" => info.summary = value.to_string(),
            _ => {}
        }
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A symlink in the checkout must not become a way to read the rest of the machine.
    #[test]
    fn a_github_remote_names_its_repository() {
        for url in [
            "https://github.com/ivankovic/omnidiff.git",
            "https://github.com/ivankovic/omnidiff",
            "git@github.com:ivankovic/omnidiff.git",
            "ssh://git@github.com/ivankovic/omnidiff.git",
        ] {
            assert_eq!(
                github_name(url).as_deref(),
                Some("ivankovic/omnidiff"),
                "{url}"
            );
        }
        assert_eq!(github_name("https://gitlab.com/a/b.git"), None);
        assert_eq!(github_name("https://github.com/a"), None);
    }

    /// git hears its messages in C and keeps the user's collation, which a wrapper sorting
    /// what it fingerprints depends on.
    #[test]
    fn git_gets_messages_in_c_and_the_user_s_collation() {
        let none = locale_for_git(|_| None);
        assert_eq!(none, [("LC_MESSAGES", Some("C".to_string()))]);
        let all = locale_for_git(|k| (k == "LC_ALL").then(|| "de_DE.UTF-8".to_string()));
        assert!(all.contains(&("LC_ALL", None)), "{all:?}");
        assert!(
            all.contains(&("LANG", Some("de_DE.UTF-8".to_string()))),
            "{all:?}"
        );
        assert!(
            all.contains(&("LC_MESSAGES", Some("C".to_string()))),
            "{all:?}"
        );
    }

    #[test]
    fn git_s_complaint_keeps_what_went_wrong_and_what_to_do() {
        assert_eq!(said(b""), "no message");
        assert_eq!(said(b"fatal: one thing\n"), "fatal: one thing");
        assert_eq!(
            said(b"git: it changed:\n--- trusted\n+++ now\n\nthen: git trust\n"),
            "git: it changed:\nthen: git trust"
        );
    }

    /// Only a directory git calls no repository is one; git refusing to run in a repository
    /// is said as git said it, not mistaken for there being none.
    #[test]
    fn a_repository_git_refuses_is_not_called_no_repository() {
        let outside = tempfile::tempdir().unwrap();
        let Err(err) = Repo::open(outside.path()) else {
            panic!("no repository there");
        };
        assert!(err.is::<NotARepository>(), "{err:#}");

        let dir = tempfile::tempdir().unwrap();
        Command::new("git")
            .args(["init", "-q"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        std::fs::write(dir.path().join(".git/config"), "this is not a config\n").unwrap();
        let Err(err) = Repo::open(dir.path()) else {
            panic!("git refuses");
        };
        let said = format!("{err:#}");
        assert!(!err.chain().any(|c| c.is::<NotARepository>()), "{said}");
        assert!(said.contains("git refused to run in"), "{said}");
        assert!(said.contains("bad config"), "git's own words: {said}");
    }

    #[test]
    fn an_address_is_shown_without_its_scheme() {
        for url in [
            "https://github.com/ivankovic/codereview.git",
            "git@github.com:ivankovic/codereview.git",
            "ssh://git@github.com/ivankovic/codereview",
        ] {
            assert_eq!(
                short_address(url),
                "github.com/ivankovic/codereview",
                "{url}"
            );
        }
        assert_eq!(short_address("/srv/git/a.git"), "/srv/git/a");
    }

    #[test]
    fn a_symlink_out_of_the_tree_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        Command::new("git")
            .args(["init", "-q"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        std::fs::write(dir.path().join("real.rs"), "fn a() {}\n").unwrap();
        let outside = dir.path().parent().unwrap().join("secret.txt");
        std::fs::write(&outside, "secret\n").unwrap();
        std::os::unix::fs::symlink(&outside, dir.path().join("link.txt")).unwrap();
        std::os::unix::fs::symlink("nowhere", dir.path().join("dangling")).unwrap();
        let repo = Repo::open(dir.path()).unwrap();
        assert!(
            repo.read_working("link.txt").is_err(),
            "followed a symlink out"
        );
        assert_eq!(repo.read_working("dangling").unwrap(), None);
        assert_eq!(
            repo.read_working("real.rs").unwrap(),
            Some(b"fn a() {}\n".to_vec())
        );
        // And neither link is offered as a file at all.
        let files = repo.list_files().unwrap();
        assert_eq!(files, vec!["real.rs".to_string()]);
        let _ = std::fs::remove_file(outside);
    }

    #[test]
    fn options_and_escapes_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        Command::new("git")
            .args(["init", "-q"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        let repo = Repo::open(dir.path()).unwrap();
        assert!(repo.commit("--output=/tmp/x").is_err());
        assert!(repo.diff_files("HEAD", "-p").is_err());
        assert!(repo.read_working("../etc/passwd").is_err());
        assert!(repo.read_working("/etc/passwd").is_err());
        assert!(repo.show("HEAD", "../x").is_err());
        assert!(repo.blame("/etc/hostname").is_err());
        assert!(repo.read_working("src/../a.rs").is_err());
        // Plain relative paths and a missing file are still fine.
        assert_eq!(repo.read_working("nope.rs").unwrap(), None);
    }

    #[test]
    fn name_status_with_rename() {
        let files = parse_name_status(b"M\0src/a.rs\0R100\0old.rs\0new.rs\0A\0x\0");
        assert_eq!(files.len(), 3);
        assert_eq!(files[1].status, FileStatus::Renamed);
        assert_eq!(files[1].old_path.as_deref(), Some("old.rs"));
        assert_eq!(files[1].path, "new.rs");
        assert_eq!(files[2].label(), "A x");
    }

    #[test]
    fn status_porcelain() {
        let map = parse_status(b" M a.rs\0M  b.rs\0?? c.rs\0R  new.rs\0old.rs\0");
        assert_eq!(map["a.rs"].unstaged, Some(FileStatus::Modified));
        assert_eq!(map["a.rs"].staged, None);
        assert_eq!(map["b.rs"].staged, Some(FileStatus::Modified));
        assert_eq!(map["c.rs"].unstaged, Some(FileStatus::Untracked));
        assert_eq!(map["new.rs"].old_path.as_deref(), Some("old.rs"));
    }

    #[test]
    fn a_zero_context_diff_says_which_lines_are_new() {
        let diff = "diff --git a/a.rs b/a.rs\n--- a/a.rs\n+++ b/a.rs\n@@ -2 +2 @@ fn a\n-x\n+y\n\
                    @@ -5,0 +6,3 @@\n+1\n+2\n+3\n@@ -9,2 +11,0 @@\n-gone\n-gone\n\
                    diff --git a/old.rs b/old.rs\n--- a/old.rs\n+++ /dev/null\n@@ -1 +0,0 @@\n-x\n";
        let lines = parse_unified_zero(diff);
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert_eq!(lines["a.rs"], [2, 6, 7, 8]);
    }

    #[test]
    fn log_records() {
        let raw = b"abc\x1fab\x1fp1 p2\x1fAnn\x1fann@x\x1f2026-09-10T10:00:00+02:00\x1fSubject\x1fbody\nmore\n\0def\x1fde\x1f\x1fBob\x1fb@x\x1f2026-09-09T10:00:00+02:00\x1fRoot\x1f\0";
        let commits = parse_log(raw);
        assert_eq!(commits.len(), 2);
        assert_eq!(commits[0].parents, vec!["p1", "p2"]);
        assert_eq!(commits[0].body, "body\nmore");
        assert_eq!(commits[0].day(), "2026-09-10");
        assert_eq!(commits[1].base(), EMPTY_TREE);
    }
}
