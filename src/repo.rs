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

impl Repo {
    /// The repository containing `from`, or an error naming the directory when it is not inside
    /// one.
    pub fn open(from: &Path) -> Result<Self> {
        let out = run_git(from, &["rev-parse", "--show-toplevel"])
            .with_context(|| format!("{} is not inside a git repository", from.display()))?;
        let root = PathBuf::from(String::from_utf8_lossy(&out).trim_end());
        Ok(Self { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn git(&self, args: &[&str]) -> Result<Vec<u8>> {
        run_git(&self.root, args)
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

    /// Working tree and index status, keyed by path.
    pub fn status(&self) -> Result<BTreeMap<String, StatusEntry>> {
        let out = self.git(&["status", "--porcelain=v1", "-z", "--untracked-files=all"])?;
        Ok(parse_status(&out))
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

fn run_git(dir: &Path, args: &[&str]) -> Result<Vec<u8>> {
    let output = git_command(dir)
        .args(args)
        .output()
        .with_context(|| format!("cannot run git {}", args.join(" ")))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!(
            "git {} failed: {}",
            args.join(" "),
            stderr.trim().lines().next().unwrap_or("no message")
        );
    }
    Ok(output.stdout)
}

/// Whether git would call this a binary file: a NUL byte in the first 8 KiB, which is its
/// own rule. Text is what this tool can show, diff and index; the rest is skipped.
pub fn is_binary(bytes: &[u8]) -> bool {
    bytes.iter().take(8192).any(|b| *b == 0)
}

/// `git -C dir`, with the messages in one language. `show` tells a missing file from a real
/// failure by what git says, and under another locale it would say it differently.
fn git_command(dir: &Path) -> Command {
    let mut command = Command::new("git");
    command.arg("-C").arg(dir).env("LC_ALL", "C");
    command
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
            if let Some(hash) = &current {
                if let Some(info) = commits.get(hash) {
                    lines.push(info.clone());
                }
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
                if let Ok(secs) = value.parse::<i64>() {
                    if let Some(dt) = chrono::DateTime::from_timestamp(secs, 0) {
                        info.date = dt.format("%Y-%m-%d").to_string();
                    }
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
