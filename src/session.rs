//! What both front ends share: the open repository, its file list and status, and the two
//! Markdown files, with the operations a review performs on them. Every UI action that changes
//! something goes through here, so the TUI and the web page cannot drift apart in behaviour.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::anchor::{Anchored, anchor_all};
use crate::config::Config;
use crate::diff::FileDiff;
use crate::notes::{GENERAL, Note, NotesFile};
use crate::repo::{ChangedFile, Commit, Repo, StatusEntry};
use crate::review::{Comment, ReviewFile, now_timestamp};
use crate::symbols::{Index, Location, Symbol};
use crate::theme::Theme;

/// Which two versions of a file a diff compares.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DiffTarget {
    /// HEAD against the working tree.
    Working,
    /// HEAD against the index.
    Staged,
    /// A commit against its first parent.
    Commit { hash: String },
    /// Any two revisions; `""` means the working tree.
    Revisions { from: String, to: String },
}

impl DiffTarget {
    pub fn label(&self) -> String {
        match self {
            DiffTarget::Working => "working tree".into(),
            DiffTarget::Staged => "staged".into(),
            DiffTarget::Commit { hash } => hash.chars().take(7).collect(),
            DiffTarget::Revisions { from, to } => format!(
                "{}..{}",
                short(from),
                if to.is_empty() {
                    "working tree"
                } else {
                    short(to)
                }
            ),
        }
    }
}

/// A full hash cut down for a label. `get` rather than a slice: a revision comes from the
/// UIs and the API, and 40 bytes of anything is not 40 characters.
fn short(rev: &str) -> &str {
    if rev.len() == 40 {
        rev.get(..7).unwrap_or(rev)
    } else {
        rev
    }
}

/// A file as the viewer shows it: its text, and the comments placed on it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileView {
    pub path: String,
    pub text: String,
    pub binary: bool,
    pub comments: Vec<Anchored>,
    pub notes: Vec<Note>,
}

pub struct Session {
    pub repo: Repo,
    pub review: ReviewFile,
    pub notes: NotesFile,
    pub files: Vec<String>,
    pub status: BTreeMap<String, StatusEntry>,
    /// Author written into new comments; `None` leaves the field out.
    pub author: Option<String>,
    pub timestamps: bool,
    pub config: Config,
    /// Where `config` is saved; `None` when there is no config directory at all.
    pub config_path: Option<PathBuf>,
    pub theme: Theme,
    /// Built on first use; `refresh` brings it up to date.
    symbols: Option<Index>,
}

impl Session {
    pub fn open(from: &Path) -> Result<Self> {
        Self::open_with_config(from, Config::path())
    }

    /// Opens the repository with the configuration read from (and saved to) `config_path`.
    pub fn open_with_config(from: &Path, config_path: Option<PathBuf>) -> Result<Self> {
        let repo = Repo::open(from)?;
        let mut review = ReviewFile::load(repo.root())?;
        review.ensure_default_sections();
        let notes = NotesFile::load(repo.root())?;
        let files = repo.list_files()?;
        let status = repo.status()?;
        let author = repo.user_name();
        let config = match &config_path {
            Some(p) => Config::load_from(p)?,
            None => Config::default(),
        };
        let theme = Theme::named(&config.theme).unwrap_or_default();
        Ok(Self {
            repo,
            review,
            notes,
            files,
            status,
            author,
            timestamps: config.timestamps,
            config,
            config_path,
            theme,
            symbols: None,
        })
    }

    /// The symbol index, built on first use over every file in the repository.
    pub fn index(&mut self) -> &Index {
        if self.symbols.is_none() {
            self.symbols = Some(Index::build(self.repo.root(), &self.files));
        }
        self.symbols.as_ref().expect("just built")
    }

    pub fn has_index(&self) -> bool {
        self.symbols.is_some()
    }

    pub fn definitions(&mut self, name: &str) -> Vec<Symbol> {
        self.index().definitions(name)
    }

    pub fn references(&mut self, name: &str) -> Vec<Location> {
        self.index().references(name)
    }

    pub fn symbols_in(&mut self, path: &str) -> Vec<Symbol> {
        self.index().symbols_in(path)
    }

    pub fn search_symbols(&mut self, query: &str) -> Vec<Symbol> {
        self.index().search(query)
    }

    /// The identifier at (0-based row, byte column) of the working tree file.
    pub fn identifier_at(&self, path: &str, row: usize, column: usize) -> Option<String> {
        let bytes = self.repo.read_working(path).ok()??;
        let text = String::from_utf8_lossy(&bytes);
        crate::symbols::identifier_at(path, &text, row, column)
    }

    /// Starts the configured agent in the repository root.
    pub fn spawn_agent(&self) -> Result<crate::agent::Agent> {
        crate::agent::spawn(&self.config.agent, self.repo.root())
    }

    fn save_config(&self) -> Result<()> {
        match &self.config_path {
            Some(p) => self.config.save_to(p),
            None => {
                anyhow::bail!("no config directory: set HOME, XDG_CONFIG_HOME or CODEREVIEW_CONFIG")
            }
        }
    }

    /// Switches to a built-in theme and remembers it in the config file.
    pub fn set_theme(&mut self, name: &str) -> Result<()> {
        let theme = Theme::named(name).with_context(|| format!("no theme named {name}"))?;
        self.config.theme = theme.name.clone();
        self.theme = theme;
        self.save_config()
    }

    pub fn set_layout(&mut self, layout: &str) -> Result<()> {
        anyhow::ensure!(
            matches!(layout, "auto" | "side-by-side" | "unified"),
            "no layout named {layout:?}"
        );
        self.config.layout = layout.to_string();
        self.save_config()
    }

    /// Takes over a theme another session saved, without writing the config again.
    pub fn adopt_theme(&mut self, theme: Theme) {
        self.config.theme = theme.name.clone();
        self.theme = theme;
    }

    /// Takes over a layout another session saved, without writing the config again.
    pub fn adopt_layout(&mut self, layout: &str) {
        self.config.layout = layout.to_string();
    }

    /// The repository's directory name, to tell repositories apart.
    pub fn name(&self) -> String {
        self.root()
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.root().display().to_string())
    }

    pub fn root(&self) -> &Path {
        self.repo.root()
    }

    /// Re-reads the file list, status and both Markdown files from disk.
    pub fn refresh(&mut self) -> Result<()> {
        self.files = self.repo.list_files()?;
        self.status = self.repo.status()?;
        self.review = ReviewFile::load(self.repo.root())?;
        self.review.ensure_default_sections();
        self.notes = NotesFile::load(self.repo.root())?;
        if let Some(index) = &mut self.symbols {
            index.refresh(&self.files);
        }
        Ok(())
    }

    /// The working tree file with its comments placed on the current text.
    pub fn file_view(&self, path: &str) -> Result<FileView> {
        let bytes = self
            .repo
            .read_working(path)?
            .with_context(|| format!("{path} is not in the working tree"))?;
        Ok(self.view_of(path, &bytes))
    }

    /// The file at `rev` (`""` for the index) with comments placed on that text.
    pub fn file_view_at(&self, rev: &str, path: &str) -> Result<FileView> {
        let bytes = self
            .repo
            .show(rev, path)?
            .with_context(|| format!("{path} does not exist at {}", short(rev)))?;
        Ok(self.view_of(path, &bytes))
    }

    fn view_of(&self, path: &str, bytes: &[u8]) -> FileView {
        let binary = is_binary(bytes);
        let text = if binary {
            String::new()
        } else {
            String::from_utf8_lossy(bytes).into_owned()
        };
        let comments = anchor_all(&self.review.comments_for(path), &text);
        let notes = self.notes.notes_for(path).into_iter().cloned().collect();
        FileView {
            path: path.to_string(),
            text,
            binary,
            comments,
            notes,
        }
    }

    /// Adds a pending comment on `path` lines `first..=last`, anchored to the current text of
    /// `first` when the file is readable. `lines` of `None` comments on the path as a whole,
    /// which is how a file or a directory is commented on; such a comment has no anchor.
    ///
    /// `source_line` is the text of `first` in the version the comment was written against;
    /// when `None`, the working tree's line is used.
    pub fn add_comment(
        &mut self,
        path: &str,
        lines: Option<(usize, usize)>,
        text: &str,
        source_line: Option<&str>,
    ) -> Result<Comment> {
        let mut comment = match lines {
            Some((first, last)) => Comment::new(path, first, last, text.trim()),
            None => Comment::on_path(path, text.trim()),
        };
        comment.author = self.author.clone();
        comment.timestamp = self.timestamps.then(now_timestamp);
        if let Some((first, _)) = lines {
            match source_line {
                Some(src) => comment = comment.with_anchor_from(src),
                None => {
                    if let Ok(Some(bytes)) = self.repo.read_working(path) {
                        let content = String::from_utf8_lossy(&bytes);
                        if let Some(src) = content.lines().nth(first.saturating_sub(1)) {
                            comment = comment.with_anchor_from(src);
                        }
                    }
                }
            }
        }
        self.review.add(comment.clone());
        self.review.save(self.repo.root())?;
        Ok(comment)
    }

    pub fn edit_comment(&mut self, target: &Comment, text: &str) -> Result<()> {
        let mut updated = target.clone();
        updated.text = text.trim().to_string();
        self.review.replace(target, updated);
        self.review.save(self.repo.root())
    }

    pub fn toggle_comment(&mut self, target: &Comment) -> Result<()> {
        self.review.toggle(target);
        self.review.save(self.repo.root())
    }

    pub fn delete_comment(&mut self, target: &Comment) -> Result<()> {
        self.review.remove(target);
        self.review.save(self.repo.root())
    }

    pub fn add_note(&mut self, target: Option<&str>, text: &str) -> Result<Note> {
        let note = Note {
            target: target.unwrap_or(GENERAL).to_string(),
            text: text.trim().to_string(),
        };
        self.notes.add(note.clone());
        self.notes.save(self.repo.root())?;
        Ok(note)
    }

    pub fn edit_note(&mut self, target: &Note, text: &str) -> Result<()> {
        let mut updated = target.clone();
        updated.text = text.trim().to_string();
        self.notes.replace(target, updated);
        self.notes.save(self.repo.root())
    }

    pub fn delete_note(&mut self, target: &Note) -> Result<()> {
        self.notes.remove(target);
        self.notes.save(self.repo.root())
    }

    /// Every comment placed on the current working tree, for the review list.
    pub fn all_anchored(&self) -> Vec<Anchored> {
        let mut by_path: BTreeMap<&str, Vec<&Comment>> = BTreeMap::new();
        for c in self.review.comments() {
            by_path.entry(c.path.as_str()).or_default().push(c);
        }
        let mut out = Vec::new();
        for (path, comments) in by_path {
            let text = self
                .repo
                .read_working(path)
                .ok()
                .flatten()
                .map(|b| String::from_utf8_lossy(&b).into_owned())
                .unwrap_or_default();
            out.extend(anchor_all(&comments, &text));
        }
        out
    }

    /// Rewrites every moved comment's line numbers to where it is now. Returns how many moved.
    pub fn reanchor(&mut self) -> Result<usize> {
        let anchored = self.all_anchored();
        let mut moved = 0;
        for a in anchored {
            if a.state == crate::anchor::AnchorState::Moved {
                let mut updated = a.comment.clone();
                updated.line = a.line;
                updated.end_line = a.end_line;
                if self.review.replace(&a.comment, updated) {
                    moved += 1;
                }
            }
        }
        if moved > 0 {
            self.review.save(self.repo.root())?;
        }
        Ok(moved)
    }

    pub fn log(&self, skip: usize, limit: usize) -> Result<Vec<Commit>> {
        self.repo.log(None, None, skip, limit)
    }

    pub fn file_log(&self, path: &str, skip: usize, limit: usize) -> Result<Vec<Commit>> {
        self.repo.log(None, Some(path), skip, limit)
    }

    pub fn commit_files(&self, hash: &str) -> Result<Vec<ChangedFile>> {
        self.repo.commit_files(hash)
    }

    /// Files that differ under `target`.
    pub fn changed_files(&self, target: &DiffTarget) -> Result<Vec<ChangedFile>> {
        match target {
            DiffTarget::Working => {
                let mut out: Vec<ChangedFile> = self
                    .status
                    .values()
                    .filter_map(|s| {
                        let status = s.unstaged?;
                        Some(ChangedFile {
                            status,
                            path: s.path.clone(),
                            old_path: s.old_path.clone(),
                        })
                    })
                    .collect();
                out.sort_by(|a, b| a.path.cmp(&b.path));
                Ok(out)
            }
            DiffTarget::Staged => {
                let mut out: Vec<ChangedFile> = self
                    .status
                    .values()
                    .filter_map(|s| {
                        let status = s.staged?;
                        Some(ChangedFile {
                            status,
                            path: s.path.clone(),
                            old_path: s.old_path.clone(),
                        })
                    })
                    .collect();
                out.sort_by(|a, b| a.path.cmp(&b.path));
                Ok(out)
            }
            DiffTarget::Commit { hash } => self.repo.commit_files(hash),
            DiffTarget::Revisions { from, to } => self.repo.diff_files(from, to),
        }
    }

    /// The two versions of `file` that `target` compares, and their diff.
    pub fn diff(&self, target: &DiffTarget, file: &ChangedFile) -> Result<FileDiff> {
        let old_path = file.old_path.as_deref().unwrap_or(&file.path);
        let (before, after) = match target {
            DiffTarget::Working => (
                self.head_or_index(old_path)?,
                self.repo.read_working(&file.path)?,
            ),
            DiffTarget::Staged => (
                self.show_if_commits("HEAD", old_path)?,
                self.repo.show("", &file.path)?,
            ),
            DiffTarget::Commit { hash } => {
                let commit = self.repo.commit(hash)?;
                (
                    self.repo.show(commit.base(), old_path)?,
                    self.repo.show(hash, &file.path)?,
                )
            }
            DiffTarget::Revisions { from, to } => (
                self.repo.show(from, old_path)?,
                if to.is_empty() {
                    self.repo.read_working(&file.path)?
                } else {
                    self.repo.show(to, &file.path)?
                },
            ),
        };
        let before = before.unwrap_or_default();
        let after = after.unwrap_or_default();
        if is_binary(&before) || is_binary(&after) {
            return Ok(FileDiff::compute(Path::new(&file.path), "", ""));
        }
        Ok(FileDiff::compute(
            Path::new(&file.path),
            &String::from_utf8_lossy(&before),
            &String::from_utf8_lossy(&after),
        ))
    }

    /// The last committed or staged version: what an unstaged change is against.
    fn head_or_index(&self, path: &str) -> Result<Option<Vec<u8>>> {
        match self.repo.show("", path)? {
            Some(bytes) => Ok(Some(bytes)),
            None => self.show_if_commits("HEAD", path),
        }
    }

    fn show_if_commits(&self, rev: &str, path: &str) -> Result<Option<Vec<u8>>> {
        if self.repo.has_commits() {
            self.repo.show(rev, path)
        } else {
            Ok(None)
        }
    }

    /// The change marker for a path in the tree: `M`, `A`, `?`, or none.
    pub fn status_letter(&self, path: &str) -> Option<char> {
        let s = self.status.get(path)?;
        s.unstaged.or(s.staged).map(|st| st.letter())
    }
}

/// The repositories directly below `dir` (child directories with a `.git` entry, hidden ones
/// left out), sorted by name; what `codereview` opens when run in a directory of checkouts
/// such as `~/src`.
pub fn repos_below(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut repos: Vec<PathBuf> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.is_dir()
                && p.join(".git").exists()
                && !p
                    .file_name()
                    .is_some_and(|n| n.to_string_lossy().starts_with('.'))
        })
        .collect();
    repos.sort_by_cached_key(|p| {
        p.file_name()
            .map(|n| n.to_string_lossy().to_lowercase())
            .unwrap_or_default()
    });
    repos
}

/// The sessions the UIs work on. Each of `paths` names a repository, a directory inside one,
/// a directory holding repositories (all of them are opened; `notify` hears how many), or a
/// file, which opens its repository at that file. A repository named twice is opened once,
/// in first-seen order. No paths means `from` itself, resolved the same way. `config_path`
/// overrides the config file, for tests.
pub fn open_targets(
    from: &Path,
    paths: &[String],
    config_path: Option<PathBuf>,
    mut notify: impl FnMut(String),
) -> Result<Vec<(Session, Option<String>)>> {
    let mut targets: Vec<(Session, Option<String>)> = Vec::new();
    let mut add = |session: Session, file: Option<String>| match targets
        .iter_mut()
        .find(|(s, _)| s.root() == session.root())
    {
        Some((_, open)) => {
            if open.is_none() {
                *open = file;
            }
        }
        None => targets.push((session, file)),
    };
    let open = |dir: &Path| Session::open_with_config(dir, config_path.clone());
    let named: Vec<PathBuf> = if paths.is_empty() {
        vec![from.to_path_buf()]
    } else {
        paths.iter().map(|p| from.join(p)).collect()
    };
    for path in &named {
        if path.is_dir() {
            match open(path) {
                Ok(session) => add(session, None),
                Err(e) => {
                    let below = repos_below(path);
                    if below.is_empty() {
                        return Err(e.context(format!(
                            "{} has no repositories directly below it either",
                            path.display()
                        )));
                    }
                    notify(format!(
                        "opening {} repositories under {}",
                        below.len(),
                        path.canonicalize()
                            .unwrap_or_else(|_| path.clone())
                            .display()
                    ));
                    for dir in below {
                        add(open(&dir)?, None);
                    }
                }
            }
        } else if path.is_file() {
            let parent = path
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .map(Path::to_path_buf)
                .unwrap_or_else(|| from.to_path_buf());
            let session = open(&parent)?;
            let full = path.canonicalize()?;
            let file = full
                .strip_prefix(session.root())
                .map(|p| p.to_string_lossy().replace('\\', "/"))
                .with_context(|| {
                    format!(
                        "{} is not inside {}",
                        path.display(),
                        session.root().display()
                    )
                })?;
            add(session, Some(file));
        } else {
            bail!("{}: no such file or directory", path.display());
        }
    }
    Ok(targets)
}

/// A NUL in the first 8 KiB, git's own heuristic.
pub fn is_binary(bytes: &[u8]) -> bool {
    bytes.iter().take(8192).any(|b| *b == 0)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::process::Command;

    /// A session on `root` whose config file lives inside `.git`, where `ls-files` cannot see
    /// it, so tests never touch the real config and never change the file list.
    pub(crate) fn scratch_session(root: &Path) -> Session {
        Session::open_with_config(root, Some(root.join(".git").join("codereview-config.toml")))
            .unwrap()
    }

    /// A scratch repository with two commits and one unstaged edit.
    /// A revision comes from a URL: forty bytes of anything must not be sliced as if they
    /// were forty characters.
    #[test]
    fn a_label_survives_an_odd_revision() {
        let forty = "\u{3b1}".repeat(20);
        assert_eq!(forty.len(), 40);
        let target = DiffTarget::Revisions {
            from: forty.clone(),
            to: forty.clone(),
        };
        assert!(!target.label().is_empty());
        assert_eq!(short(&forty), forty);
        assert_eq!(short(&"a".repeat(40)), "aaaaaaa");
    }

    #[test]
    fn targets_fan_out_over_a_directory_of_repositories() {
        let parent = tempfile::tempdir().unwrap();
        for name in ["beta", "Alpha", ".hidden", "plain"] {
            let dir = parent.path().join(name);
            std::fs::create_dir(&dir).unwrap();
            if name != "plain" {
                Command::new("git")
                    .args(["init", "-q"])
                    .current_dir(&dir)
                    .output()
                    .unwrap();
            }
        }
        std::fs::write(parent.path().join("Alpha/x.rs"), "fn x() {}\n").unwrap();
        let below = repos_below(parent.path());
        let names: Vec<String> = below
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            names,
            vec!["Alpha", "beta"],
            "sorted without regard to case"
        );
        let config = parent.path().join("config.toml");
        let mut notes = Vec::new();
        let targets =
            open_targets(parent.path(), &[], Some(config.clone()), |m| notes.push(m)).unwrap();
        assert_eq!(targets.len(), 2);
        assert!(targets[0].0.root().ends_with("Alpha"));
        assert_eq!(notes.len(), 1, "{notes:?}");
        assert!(notes[0].starts_with("opening 2 repositories under"));
        // Named paths: the parent again, a file inside one repository (opened at it, once),
        // and a directory that is neither.
        let args: Vec<String> = ["Alpha/x.rs", ".", "Alpha"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let targets = open_targets(parent.path(), &args, Some(config.clone()), |_| {}).unwrap();
        assert_eq!(targets.len(), 2);
        assert_eq!(targets[0].1.as_deref(), Some("x.rs"));
        assert!(targets[1].1.is_none());
        let err = open_targets(parent.path(), &["plain".into()], Some(config), |_| {})
            .err()
            .unwrap();
        assert!(
            format!("{err:#}").contains("no repositories directly below"),
            "{err:#}"
        );
    }

    pub(crate) fn scratch_repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let git = |args: &[&str]| {
            let out = Command::new("git")
                .arg("-C")
                .arg(dir.path())
                .args(args)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "git {args:?}: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        };
        git(&["init", "-q", "-b", "main"]);
        git(&["config", "user.email", "t@example.com"]);
        git(&["config", "user.name", "Tester"]);
        std::fs::write(dir.path().join("a.rs"), "fn a() {}\n").unwrap();
        git(&["add", "."]);
        git(&["commit", "-q", "-m", "first"]);
        std::fs::write(dir.path().join("a.rs"), "fn a() {}\nfn b() {}\n").unwrap();
        std::fs::write(dir.path().join("Makefile"), "all:\n\techo hi\n").unwrap();
        git(&["add", "."]);
        git(&["commit", "-q", "-m", "second"]);
        std::fs::write(dir.path().join("a.rs"), "fn a() {}\nfn b() {}\nfn c() {}\n").unwrap();
        std::fs::write(dir.path().join("new.py"), "print(1)\n").unwrap();
        dir
    }

    #[test]
    fn session_end_to_end() {
        let dir = scratch_repo();
        let mut s = scratch_session(dir.path());
        assert_eq!(s.files, vec!["Makefile", "a.rs", "new.py"]);
        assert_eq!(s.status_letter("a.rs"), Some('M'));
        assert_eq!(s.status_letter("new.py"), Some('?'));
        assert_eq!(s.author.as_deref(), Some("Tester"));

        let c = s.add_comment("a.rs", Some((2, 2)), "why b?", None).unwrap();
        assert_eq!(c.anchor.as_deref(), Some("fn b() {}"));
        let view = s.file_view("a.rs").unwrap();
        assert_eq!(view.comments.len(), 1);
        assert_eq!(view.comments[0].state, crate::anchor::AnchorState::Exact);

        // Insert a line above: the comment moves.
        std::fs::write(dir.path().join("a.rs"), "// top\nfn a() {}\nfn b() {}\n").unwrap();
        let view = s.file_view("a.rs").unwrap();
        assert_eq!(view.comments[0].state, crate::anchor::AnchorState::Moved);
        assert_eq!(view.comments[0].line, Some(3));
        assert_eq!(s.reanchor().unwrap(), 1);
        assert!(
            std::fs::read_to_string(dir.path().join("REVIEW.md"))
                .unwrap()
                .contains("on line 3:")
        );

        // A comment on a whole file, and one on a directory: no line, no anchor.
        let whole = s.add_comment("a.rs", None, "needs tests", None).unwrap();
        assert_eq!((whole.line, whole.anchor.as_deref()), (None, None));
        let on_dir = s
            .add_comment("src", None, "too many modules", None)
            .unwrap();
        assert_eq!(on_dir.location(), "src");
        let view = s.file_view("a.rs").unwrap();
        assert_eq!(view.comments.len(), 2);
        assert!(s.all_anchored().iter().any(|a| a.comment == on_dir));
        s.delete_comment(&whole).unwrap();
        s.delete_comment(&on_dir).unwrap();

        s.toggle_comment(&s.review.comments()[0].clone()).unwrap();
        assert_eq!(s.review.pending_count(), 0);

        let log = s.log(0, 10).unwrap();
        assert_eq!(log.len(), 2);
        assert_eq!(log[0].subject, "second");
        let files = s.commit_files(&log[0].hash).unwrap();
        assert_eq!(files.len(), 2);
        let file_log = s.file_log("Makefile", 0, 10).unwrap();
        assert_eq!(file_log.len(), 1);

        let d = s
            .diff(
                &DiffTarget::Commit {
                    hash: log[0].hash.clone(),
                },
                &files[1],
            )
            .unwrap();
        assert!(d.structural);
        assert_eq!(d.after.line_ops[1], crate::diff::Op::Insert);

        let working = s.changed_files(&DiffTarget::Working).unwrap();
        assert_eq!(working.len(), 2);
        let d = s.diff(&DiffTarget::Working, &working[0]).unwrap();
        assert_eq!(d.before.line_count(), 2);
        assert_eq!(d.after.line_count(), 3);

        s.add_note(Some("a.rs"), "entry point").unwrap();
        s.add_note(None, "small repo").unwrap();
        assert_eq!(s.file_view("a.rs").unwrap().notes.len(), 1);
        assert!(
            std::fs::read_to_string(dir.path().join("NOTES.md"))
                .unwrap()
                .contains("## General\n- small repo")
        );
    }

    #[test]
    fn empty_repository_is_fine() {
        let dir = tempfile::tempdir().unwrap();
        Command::new("git")
            .args(["-C", dir.path().to_str().unwrap(), "init", "-q"])
            .status()
            .unwrap();
        std::fs::write(dir.path().join("x.txt"), "hi\n").unwrap();
        let s = Session::open(dir.path()).unwrap();
        assert_eq!(s.files, vec!["x.txt"]);
        assert!(s.log(0, 5).unwrap().is_empty());
        let working = s.changed_files(&DiffTarget::Working).unwrap();
        assert_eq!(working.len(), 1);
        let d = s.diff(&DiffTarget::Working, &working[0]).unwrap();
        assert_eq!(d.after.line_ops, vec![crate::diff::Op::Insert]);
    }
}
