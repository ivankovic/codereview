//! What both front ends share: the open repository, its file list and status, and the two
//! Markdown files, with the operations a review performs on them. Every UI action that changes
//! something goes through here, so the TUI and the web page cannot drift apart in behaviour.

use std::cell::RefCell;
use std::collections::hash_map::DefaultHasher;
use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::anchor::{Anchored, anchor, anchor_through};
use crate::config::Config;
use crate::diff::FileDiff;
use crate::facts::{CheckFact, Facts};
use crate::notes::{GENERAL, Note, NotesFile};
use crate::repo::{ChangedFile, Commit, FileStatus, Repo, StatusEntry, is_binary};
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
    /// The commit the after side is, when it is one: what a line number on that side counts
    /// in. The working tree and the index are not commits.
    pub fn after_revision(&self) -> Option<&str> {
        match self {
            DiffTarget::Commit { hash } => Some(hash),
            DiffTarget::Revisions { to, .. } if !to.is_empty() => Some(to),
            _ => None,
        }
    }

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

/// The hash of a text, for keys that stand for content.
fn hash_of(text: &str) -> u64 {
    let mut h = DefaultHasher::new();
    text.hash(&mut h);
    h.finish()
}

/// A file as the viewer shows it: its text, and the comments placed on it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileView {
    pub path: String,
    /// The first `MAX_VIEW_LINES` lines of the file: past that a file is a fixture or a
    /// dump, not something a reviewer reads, and drawing it whole is what freezes a phone.
    pub text: String,
    /// How many lines the file has, which is more than `text` holds when it was cut.
    #[serde(default)]
    pub total_lines: usize,
    pub binary: bool,
    pub comments: Vec<Anchored>,
    pub notes: Vec<Note>,
}

/// How many lines of a file either front end shows. The same line as highlighting draws.
pub const MAX_VIEW_LINES: usize = 20_000;

pub struct Session {
    pub repo: Repo,
    pub review: ReviewFile,
    pub notes: NotesFile,
    pub files: Vec<String>,
    pub status: BTreeMap<String, StatusEntry>,
    /// Author written into new comments; `None` leaves the field out.
    pub author: Option<String>,
    // Timestamps and the colour scheme live in `config`; `theme` below is the resolved
    // form of `config.theme`, kept because every drawn frame reads it.
    pub config: Config,
    /// Where `config` is saved; `None` when there is no config directory at all.
    pub config_path: Option<PathBuf>,
    pub theme: Theme,
    /// Built on first use; `refresh` brings it up to date.
    symbols: Option<Index>,
    /// How each line of a file at a commit maps to the text a comment is being placed in, by
    /// path and commit, with a hash of that text. The diff is the costly part, and a file is
    /// placed again every time a page or a screen redraws it. `None` remembers a commit or a
    /// path that is not there, so it is not asked for again.
    line_maps: RefCell<HashMap<(String, String), (u64, LineMap)>>,
    /// What was worked out from the repository, kept until it changes: each with the
    /// fingerprint, or the index generation, it was worked out at.
    memo: RefCell<Memo>,
    /// Goes up whenever the index is built, replaced, or changed by a refresh, so what was
    /// read off it — the ranking, the tests — is read again only then.
    index_generation: u64,
    /// `OVERVIEW.md` as it was when the repository was opened: what to show at once, before
    /// the index that would work it out afresh is built.
    pub facts_file: Option<Facts>,
    /// The fingerprint the repository has held, and since when, for writing `OVERVIEW.md`
    /// once it has settled; and the fingerprint it was last written at.
    settle: Option<(u64, Instant)>,
    facts_written_at: Option<u64>,
}

type LineMap = Option<Arc<Vec<Option<usize>>>>;

/// How long a repository must hold still before `OVERVIEW.md` is written: a save in an
/// editor is not a reason to rewrite it, a pause is.
const SETTLE: Duration = Duration::from_secs(10);

/// How many of the ranked lists `OVERVIEW.md` keeps: as many as the page shows.
const LISTED: usize = 8;

/// The tests found, and the leaned-on files no test names.
type TestsFound = (Vec<crate::testsuite::TestCase>, Vec<crate::symbols::Reach>);

#[derive(Default)]
struct Memo {
    overview: Option<(u64, crate::overview::Overview)>,
    reach: Option<(u64, Vec<crate::symbols::Reach>)>,
    tests: Option<(u64, TestsFound)>,
    /// The overview's TODO count, per file, brought up to date rather than taken again.
    todos: crate::overview::TodoCounts,
    /// The last few diffs computed, by path and the content of both sides: the engine is
    /// syntax-aware and a couple of hundred milliseconds on a large file, and the same diff
    /// is asked for on every open of a modified file and every look at its diff.
    diffs: VecDeque<((String, u64, u64), crate::diff::FileDiff)>,
}

/// How many diffs the session keeps.
const DIFFS_REMEMBERED: usize = 16;

/// Where facts shown from `OVERVIEW.md` come from, for a page to say so while fresh ones
/// are on their way.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AsOf {
    pub commit: Option<String>,
    /// Commits made since; the file is that far behind.
    pub commits_since: usize,
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
        let facts_file = Facts::load(repo.root())?;
        // A name that matches nothing is a typo in a hand-edited file, and silently
        // showing something else would hide it.
        let theme = Theme::named(&config.theme).with_context(|| {
            format!(
                "no colour scheme named {:?}; the config file is {}",
                config.theme,
                config_path
                    .as_deref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| "not in use".into())
            )
        })?;
        Ok(Self {
            repo,
            review,
            notes,
            files,
            status,
            author,

            config,
            config_path,
            theme,
            symbols: None,
            line_maps: RefCell::new(HashMap::new()),
            memo: RefCell::new(Memo::default()),
            index_generation: 0,
            facts_file,
            settle: None,
            facts_written_at: None,
        })
    }

    /// The symbol index, built on first use over every file in the repository — here, on
    /// this thread, holding whatever holds the session. A front end that can wait elsewhere
    /// builds one with [`Index::build`] and hands it to [`Self::install_index`].
    pub fn index(&mut self) -> &Index {
        if self.symbols.is_none() {
            self.symbols = Some(Index::build(self.repo.root(), &self.files));
            self.index_generation += 1;
        }
        self.symbols.as_ref().expect("just built")
    }

    /// The index if it is built, without building it.
    pub fn index_if_ready(&self) -> Option<&Index> {
        self.symbols.as_ref()
    }

    /// Takes an index built elsewhere, unless one was built here meanwhile.
    pub fn install_index(&mut self, index: Index) {
        if self.symbols.is_none() {
            self.symbols = Some(index);
            self.index_generation += 1;
        }
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

    /// Writes `change` to the config file, applied to what the file holds now rather than to
    /// this session's copy of it: every served repository has a session and they share one
    /// file, so writing a whole copy back would undo whatever another saved since this one
    /// read it. This session's copy becomes what was written.
    fn update_config(&mut self, change: impl FnOnce(&mut Config)) -> Result<()> {
        let path = match &self.config_path {
            Some(p) => p.clone(),
            None => Config::path()
                .context("no config directory: set HOME, XDG_CONFIG_HOME or CODEREVIEW_CONFIG")?,
        };
        let mut config = Config::load_from(&path)?;
        change(&mut config);
        config.save_to(&path)?;
        self.config = config;
        Ok(())
    }

    /// Switches to a built-in theme and remembers it in the config file.
    pub fn set_theme(&mut self, name: &str) -> Result<()> {
        let theme = Theme::named(name).with_context(|| format!("no theme named {name}"))?;
        let saved = theme.name.clone();
        self.update_config(|c| c.theme = saved)?;
        self.theme = theme;
        Ok(())
    }

    /// Saves a check found in this repository to the config file, scoped to this repository:
    /// added to an entry that runs the same thing for other repositories, or as a new one.
    pub fn save_check(&mut self, check: &crate::config::Check) -> Result<()> {
        let root = self
            .root()
            .canonicalize()
            .unwrap_or_else(|_| self.root().to_path_buf())
            .display()
            .to_string();
        self.update_config(|c| {
            let same = c.analysis.iter_mut().find(|e| {
                e.name == check.name
                    && e.run == check.run
                    && e.reads == check.reads
                    && e.kind == check.kind
                    && !e.repos.is_empty()
            });
            match same {
                Some(entry) => {
                    if !entry.repos.contains(&root) {
                        entry.repos.push(root);
                    }
                }
                None => c.analysis.push(crate::config::Check {
                    repos: vec![root],
                    ..check.clone()
                }),
            }
        })
    }

    pub fn set_layout(&mut self, layout: &str) -> Result<()> {
        anyhow::ensure!(
            matches!(layout, "auto" | "side-by-side" | "unified"),
            "no layout named {layout:?}"
        );
        self.update_config(|c| c.layout = layout.to_string())
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

    /// Remembers whether the terminal UI's side panels hide, in the config file.
    pub fn set_sidebar(&mut self, sidebar: crate::config::SidebarConfig) -> Result<()> {
        self.update_config(|c| c.sidebar = sidebar)
    }

    /// Takes over side panel settings another session saved, without writing them again.
    pub fn adopt_sidebar(&mut self, sidebar: crate::config::SidebarConfig) {
        self.config.sidebar = sidebar;
    }

    /// What `path` defines, read from the working tree and parsed on its own: the outline of
    /// one file needs nothing from the others, so it does not wait for the whole index.
    pub fn outline(&self, path: &str) -> Result<Vec<crate::symbols::Symbol>> {
        let bytes = self.repo.read_working(path)?.unwrap_or_default();
        let text = String::from_utf8_lossy(&bytes);
        Ok(crate::symbols::symbols_of(path, &text))
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

    /// What a repository looks like when you open it: where to start reading, and how it is
    /// doing. Everything that costs a git command or a file read; the ranking that needs the
    /// symbol index is [`Self::leans_on`], so that one cold index does not hold up the rest.
    pub fn overview(&self) -> Result<crate::overview::Overview> {
        let fingerprint = self.fingerprint(None)?;
        if let Some((at, overview)) = &self.memo.borrow().overview {
            if *at == fingerprint {
                return Ok(overview.clone());
            }
        }
        let overview = crate::overview::of(self)?;
        self.memo.borrow_mut().overview = Some((fingerprint, overview.clone()));
        Ok(overview)
    }

    /// `TODO`, `FIXME`, `HACK` and `XXX` in the tracked files, leaving `skip` out. Counted in
    /// full the first time, and after that only in the files that moved since: a repository
    /// of gigabytes of fixtures is a few seconds to grep and milliseconds to bring up to
    /// date.
    pub fn todos(&self, skip: &BTreeSet<String>) -> usize {
        let mut memo = self.memo.borrow_mut();
        memo.todos.update(self, skip);
        memo.todos.total()
    }

    /// Which files the rest of the tree leans on, highest first. Builds the symbol index on
    /// first use, which is seconds on a large repository and nothing afterwards; the ranking
    /// is read off it once per generation.
    pub fn leans_on(&mut self, most: usize) -> Vec<crate::symbols::Reach> {
        self.index();
        let generation = self.index_generation;
        let cached = self
            .memo
            .borrow()
            .reach
            .as_ref()
            .and_then(|(g, r)| (*g == generation).then(|| r.clone()));
        let mut reach = match cached {
            Some(r) => r,
            None => {
                let reach = self.index().reach();
                self.memo.borrow_mut().reach = Some((generation, reach.clone()));
                reach
            }
        };
        reach.truncate(most);
        reach
    }

    /// Every test the repository defines, grouped by file, and the files the tree leans on
    /// that no test names, at most `most` of them. Builds the symbol index on first use, and
    /// walks it once per generation.
    pub fn tests(&mut self, most: usize) -> crate::testsuite::Inventory {
        self.index();
        let generation = self.index_generation;
        let cached = self
            .memo
            .borrow()
            .tests
            .as_ref()
            .and_then(|(g, t)| (*g == generation).then(|| t.clone()));
        let (tests, untested) = match cached {
            Some(t) => t,
            None => {
                let index = self.index();
                let found = (index.tests(), index.untested());
                self.memo.borrow_mut().tests = Some((generation, found.clone()));
                found
            }
        };
        let mut untested = untested;
        untested.truncate(most);
        crate::testsuite::Inventory::of(tests, untested)
    }

    /// Where what `OVERVIEW.md` says stands relative to now, for showing it while the index
    /// that would say it afresh is built: `None` once the index is ready; with no file, as of
    /// no commit at all.
    pub fn as_of(&self) -> Option<AsOf> {
        if self.has_index() {
            return None;
        }
        let Some(facts) = self.facts_file.as_ref() else {
            return Some(AsOf {
                commit: None,
                commits_since: 0,
            });
        };
        let commits_since = facts
            .commit
            .as_deref()
            .and_then(|c| {
                let range = format!("{c}..HEAD");
                let out = self
                    .repo
                    .git_output(&["rev-list", "--count", &range])
                    .ok()?;
                String::from_utf8_lossy(&out).trim().parse().ok()
            })
            .unwrap_or(0);
        Some(AsOf {
            commit: facts.commit.clone(),
            commits_since,
        })
    }

    /// The facts about this repository, worked out now, for `OVERVIEW.md`: needs the index.
    /// `checks` is what the checks last did when the caller knows; `None` keeps what the file
    /// says about them. The About section and any heading of someone else's are taken over
    /// from the file as it is on disk now, not as it was read at open: an agent may have
    /// written its explanation there since.
    pub fn facts(&mut self, checks: Option<Vec<CheckFact>>) -> Result<Facts> {
        let overview = self.overview()?;
        let leans_on = self.leans_on(LISTED);
        let inventory = self.tests(LISTED);
        let mut facts = Facts {
            commit: self.repo.short_hash("HEAD").ok(),
            branch: self.repo.head_label(),
            when: chrono::Local::now().format("%Y-%m-%d %H:%M").to_string(),
            files: self.files.len(),
            tests: inventory.total,
            test_files: inventory.files.len(),
            skipped: inventory.ignored,
            checks: checks.clone().unwrap_or_default(),
            entries: overview
                .entries
                .iter()
                .map(|e| (e.path.clone(), e.why.clone()))
                .collect(),
            leans_on: leans_on.into_iter().map(|r| (r.path, r.names)).collect(),
            tests_by_file: inventory
                .files
                .iter()
                .map(|f| {
                    (
                        f.path.clone(),
                        f.tests
                            .iter()
                            .map(|t| (t.label.clone(), t.ignored))
                            .collect(),
                    )
                })
                .collect(),
            untested: inventory
                .untested
                .into_iter()
                .map(|r| (r.path, r.names))
                .collect(),
            about: Vec::new(),
            kept: Vec::new(),
        };
        if let Some(on_disk) = Facts::load(self.repo.root())? {
            facts.carry_over(&on_disk, checks.is_some());
        }
        Ok(facts)
    }

    /// Keeps `OVERVIEW.md` current. Called every few seconds by a front end, it writes the
    /// file once the repository has held still for ten seconds, the index is built, and the
    /// file has not been written at this fingerprint yet. Returns whether it wrote. Costs one
    /// `git status` per call, and the facts only when they are due.
    pub fn keep_facts(&mut self, checks: Option<Vec<CheckFact>>) -> Result<bool> {
        if !self.has_index() {
            return Ok(false);
        }
        let fingerprint = self.fingerprint(None)?;
        let now = Instant::now();
        match self.settle {
            Some((held, since)) if held == fingerprint => {
                if now.duration_since(since) < SETTLE {
                    return Ok(false);
                }
            }
            _ => {
                self.settle = Some((fingerprint, now));
                return Ok(false);
            }
        }
        if self.facts_written_at == Some(fingerprint) {
            return Ok(false);
        }
        let wrote = self.write_facts(checks)?;
        self.facts_written_at = Some(fingerprint);
        Ok(wrote)
    }

    /// Writes `OVERVIEW.md` now, building the index if it is not built. Returns whether the
    /// file changed.
    pub fn write_facts(&mut self, checks: Option<Vec<CheckFact>>) -> Result<bool> {
        let facts = self.facts(checks)?;
        let wrote = facts.save(self.repo.root())?;
        self.facts_file = Some(facts);
        Ok(wrote)
    }

    /// How far the code has moved since `when` (RFC 3339): commits made after it, and files
    /// in the working tree written after it. `REVIEW.md`, `NOTES.md` and `except` — the
    /// artefact a check writes, which is always newer than the run that wrote it — are left
    /// out, since none of them is code a result could be out of date with.
    pub fn changed_since(
        &self,
        when: &str,
        except: Option<&str>,
    ) -> Result<crate::analysis::Staleness> {
        let then: std::time::SystemTime = chrono::DateTime::parse_from_rfc3339(when)
            .with_context(|| format!("{when:?} is not a time"))?
            .into();
        let commits = self.repo.commits_since(then)?;
        let root = self.repo.root();
        let files = self
            .repo
            .status()?
            .into_keys()
            .filter(|p| {
                p != crate::review::FILE_NAME
                    && p != crate::notes::FILE_NAME
                    && Some(p.as_str()) != except
            })
            .filter(|p| {
                std::fs::metadata(root.join(p))
                    .and_then(|m| m.modified())
                    .is_ok_and(|m| m > then)
            })
            .count();
        Ok(crate::analysis::Staleness { commits, files })
    }

    /// A fingerprint of everything a front end draws: the status of every path in the working
    /// tree, the branch, and the size and modification time of the two Markdown files and of
    /// one file being looked at. It moves whenever anything on screen would look different,
    /// whoever made the change, and costs one `git status`, so a page can ask for it every few
    /// seconds and re-read only when it has moved. It is a number to compare and nothing else:
    /// it says that something changed, never what.
    pub fn fingerprint(&self, open: Option<&str>) -> Result<u64> {
        let mut hasher = DefaultHasher::new();
        for (path, entry) in self.repo.status()? {
            // The tool writes OVERVIEW.md itself; that is not the repository changing.
            if path == crate::facts::FILE_NAME {
                continue;
            }
            path.hash(&mut hasher);
            entry.staged.map(FileStatus::letter).hash(&mut hasher);
            entry.unstaged.map(FileStatus::letter).hash(&mut hasher);
            entry.old_path.hash(&mut hasher);
        }
        // A branch switch can leave the status untouched while every file reads differently.
        self.repo.head_label().hash(&mut hasher);
        // Status only says *that* a file is modified, so a file written twice looks the same
        // twice over. The three files a page has on screen are stamped as well.
        let root = self.repo.root();
        stamp(&root.join(crate::review::FILE_NAME)).hash(&mut hasher);
        stamp(&root.join(crate::notes::FILE_NAME)).hash(&mut hasher);
        // Resolved rather than joined: `open` comes from a request.
        if let Some(path) = open {
            if let Some(full) = self.repo.working_path(path)? {
                stamp(&full).hash(&mut hasher);
            }
        }
        Ok(hasher.finish())
    }

    /// Commits the change under review: the whole working tree when `all`, otherwise whatever
    /// is already staged. The session is re-read afterwards, since every file's status has
    /// just changed, so a caller can draw the result without asking for a refresh.
    pub fn commit(&mut self, message: &str, all: bool) -> Result<Commit> {
        let status = self.repo.status()?;
        let ready = status.values().any(|e| {
            if all {
                e.staged.is_some() || e.unstaged.is_some()
            } else {
                e.staged.is_some()
            }
        });
        if !ready {
            bail!(
                "{}",
                if all {
                    "nothing to commit: the working tree is clean"
                } else {
                    "nothing is staged to commit"
                }
            );
        }
        let commit = self.repo.create_commit(message, all)?;
        self.refresh()?;
        Ok(commit)
    }

    /// Re-reads the file list, status and both Markdown files from disk.
    pub fn refresh(&mut self) -> Result<()> {
        self.files = self.repo.list_files()?;
        self.status = self.repo.status()?;
        self.review = ReviewFile::load(self.repo.root())?;
        self.review.ensure_default_sections();
        self.notes = NotesFile::load(self.repo.root())?;
        self.facts_file = Facts::load(self.repo.root())?;
        if let Some(index) = &mut self.symbols {
            if index.refresh(&self.files) {
                self.index_generation += 1;
            }
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
        // Comments are placed on the whole file, so a comment past the cut is still found
        // where it is rather than reported as gone.
        let comments = self.anchor_in(path, &text);
        let notes = self.notes.notes_for(path).into_iter().cloned().collect();
        let total_lines = text.lines().count();
        let text = if total_lines > MAX_VIEW_LINES {
            text.lines()
                .take(MAX_VIEW_LINES)
                .collect::<Vec<_>>()
                .join("\n")
                + "\n"
        } else {
            text
        };
        FileView {
            path: path.to_string(),
            text,
            total_lines,
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
    /// Adds a comment. `at` is the commit whose version of the file the line numbers count in,
    /// when they were read off a commit's diff; `None` means the working tree, whose lines are
    /// also HEAD's when the file is committed and unchanged. Either way, a comment on lines
    /// that are in a commit records it, so it can be followed through what changes later.
    pub fn add_comment(
        &mut self,
        path: &str,
        lines: Option<(usize, usize)>,
        text: &str,
        source_line: Option<&str>,
        at: Option<&str>,
    ) -> Result<Comment> {
        let mut comment = match lines {
            Some((first, last)) => Comment::new(path, first, last, text.trim()),
            None => Comment::on_path(path, text.trim()),
        };
        comment.author = self.author.clone();
        comment.timestamp = self.config.timestamps.then(now_timestamp);
        let at = at.filter(|rev| !rev.is_empty());
        if let Some((first, _)) = lines {
            comment.commit = match at {
                Some(rev) => self.repo.short_hash(rev).ok(),
                None if self.repo.committed_as_is(path) => self.repo.short_hash("HEAD").ok(),
                None => None,
            };
            let content = match source_line {
                Some(src) => Some(src.to_string()),
                None => {
                    let bytes = match at {
                        Some(rev) => self.repo.show(rev, path),
                        None => self.repo.read_working(path),
                    };
                    bytes.ok().flatten().and_then(|b| {
                        let text = String::from_utf8_lossy(&b).into_owned();
                        text.lines()
                            .nth(first.saturating_sub(1))
                            .map(str::to_string)
                    })
                }
            };
            if let Some(src) = content {
                comment = comment.with_anchor_from(&src);
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
            let text = self.working_text(path);
            out.extend(self.anchor_comments(path, &comments, &text));
        }
        out
    }

    fn working_text(&self, path: &str) -> String {
        self.repo
            .read_working(path)
            .ok()
            .flatten()
            .map(|b| String::from_utf8_lossy(&b).into_owned())
            .unwrap_or_default()
    }

    /// Every comment on `path`, placed in `text`: the working tree, or the after side of a
    /// diff. A comment that names a commit is followed from that commit's version of the file
    /// with omnidiff; one that does not, or whose commit is not in the repository any more, is
    /// found by the text of its line.
    pub fn anchor_in(&self, path: &str, text: &str) -> Vec<Anchored> {
        self.anchor_comments(path, &self.review.comments_for(path), text)
    }

    fn anchor_comments(&self, path: &str, comments: &[&Comment], text: &str) -> Vec<Anchored> {
        let lines: Vec<&str> = text.lines().collect();
        comments
            .iter()
            .map(|c| {
                let map = c
                    .commit
                    .as_deref()
                    .filter(|_| c.line.is_some())
                    .and_then(|commit| self.line_map(path, commit, text));
                match map {
                    Some(map) if c.line.is_some_and(|l| l <= map.len()) => {
                        anchor_through(c, &lines, &map)
                    }
                    _ => anchor(c, &lines),
                }
            })
            .collect()
    }

    /// Where each line of `path` at `commit` is in `text`, from the cache when `text` is what
    /// it was worked out for.
    fn line_map(&self, path: &str, commit: &str, text: &str) -> LineMap {
        let key = (path.to_string(), commit.to_string());
        let hash = {
            let mut h = DefaultHasher::new();
            text.hash(&mut h);
            h.finish()
        };
        if let Some((h, map)) = self.line_maps.borrow().get(&key) {
            if *h == hash {
                return map.clone();
            }
        }
        let map = self
            .repo
            .show(commit, path)
            .ok()
            .flatten()
            .filter(|bytes| !is_binary(bytes))
            .map(|bytes| {
                let before = String::from_utf8_lossy(&bytes);
                if before == text {
                    Arc::new((0..before.lines().count()).map(Some).collect())
                } else {
                    Arc::new(crate::diff::follow_lines(Path::new(path), &before, text))
                }
            });
        let mut cache = self.line_maps.borrow_mut();
        // A long session over a busy repository would otherwise keep every version it saw.
        if cache.len() > 256 {
            cache.clear();
        }
        cache.insert(key, (hash, map.clone()));
        map
    }

    /// Rewrites every moved comment's line numbers to where it is now. Returns how many moved.
    /// A comment that named a commit now counts its lines in the working tree, so it names
    /// HEAD if the file is committed as it is and nothing otherwise, and takes the text its
    /// line has now, which a followed line may have changed.
    pub fn reanchor(&mut self) -> Result<usize> {
        let anchored = self.all_anchored();
        let mut moved = 0;
        for a in anchored {
            if a.state == crate::anchor::AnchorState::Moved {
                let mut updated = a.comment.clone();
                updated.line = a.line;
                updated.end_line = a.end_line;
                if updated.commit.is_some() {
                    let path = updated.path.clone();
                    updated.commit = if self.repo.committed_as_is(&path) {
                        self.repo.short_hash("HEAD").ok()
                    } else {
                        None
                    };
                    let text = self.working_text(&path);
                    let now = a.line.and_then(|l| text.lines().nth(l - 1));
                    if let Some(src) = now {
                        updated = updated.with_anchor_from(src);
                    }
                }
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
    /// The files git reports as changed on one side of the index, by path. `side` picks
    /// which side: what is staged, or what is not.
    fn status_files(&self, side: impl Fn(&StatusEntry) -> Option<FileStatus>) -> Vec<ChangedFile> {
        let mut out: Vec<ChangedFile> = self
            .status
            .values()
            .filter_map(|s| {
                Some(ChangedFile {
                    status: side(s)?,
                    path: s.path.clone(),
                    old_path: s.old_path.clone(),
                })
            })
            .collect();
        out.sort_by(|a, b| a.path.cmp(&b.path));
        out
    }

    pub fn changed_files(&self, target: &DiffTarget) -> Result<Vec<ChangedFile>> {
        match target {
            DiffTarget::Working => Ok(self.status_files(|s| s.unstaged)),
            DiffTarget::Staged => Ok(self.status_files(|s| s.staged)),
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
        let before = String::from_utf8_lossy(&before);
        let after = String::from_utf8_lossy(&after);
        let key = (file.path.clone(), hash_of(&before), hash_of(&after));
        if let Some((_, diff)) = self.memo.borrow().diffs.iter().find(|(k, _)| *k == key) {
            return Ok(diff.clone());
        }
        let diff = FileDiff::compute(Path::new(&file.path), &before, &after);
        let mut memo = self.memo.borrow_mut();
        if memo.diffs.len() >= DIFFS_REMEMBERED {
            memo.diffs.pop_front();
        }
        memo.diffs.push_back((key, diff.clone()));
        Ok(diff)
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
/// Size and modification time, or nothing at all when the file is not there. Two files that
/// differ in neither are taken to be unchanged, which is what every build tool assumes.
fn stamp(path: &Path) -> Option<(u64, Option<std::time::Duration>)> {
    let meta = std::fs::metadata(path).ok()?;
    let modified = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok());
    Some((meta.len(), modified))
}

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

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::anchor::AnchorState;
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

    /// The end of a review that went well. `scratch_repo` leaves `a.rs` edited and `new.py`
    /// untracked, which is the shape the working-tree view shows.
    #[test]
    fn a_review_can_end_in_a_commit() {
        let dir = scratch_repo();
        let mut session = scratch_session(dir.path());
        let git = |args: &[&str]| {
            let out = Command::new("git")
                .arg("-C")
                .arg(dir.path())
                .args(args)
                .output()
                .unwrap();
            assert!(out.status.success(), "git {args:?}");
        };
        let commits = || {
            Command::new("git")
                .arg("-C")
                .arg(dir.path())
                .args(["rev-list", "--count", "HEAD"])
                .output()
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
                .unwrap()
        };
        let before = commits();

        // A message is required, and git is never run without one.
        assert!(session.commit("   ", true).is_err());
        // Nothing is staged yet, and the staged view commits only what is.
        let refused = session.commit("too early", false).unwrap_err().to_string();
        assert!(refused.contains("nothing is staged"), "{refused}");
        assert_eq!(commits(), before, "nothing was committed");

        // The staged view: one file staged by hand is all that goes in.
        git(&["add", "new.py"]);
        let made = session.commit("add the script", false).unwrap();
        assert_eq!(made.subject, "add the script");
        assert!(
            session.status.contains_key("a.rs"),
            "the unstaged edit is still waiting: {:?}",
            session.status
        );
        // The session was re-read, so the file it just committed is tracked now.
        assert!(session.files.contains(&"new.py".to_string()));

        // The working-tree view: everything that is left.
        let made = session.commit("finish the review", true).unwrap();
        assert_eq!(made.subject, "finish the review");
        assert!(session.status.is_empty(), "{:?}", session.status);
        assert_ne!(commits(), before);

        // And now there is nothing to commit at all.
        let refused = session.commit("again", true).unwrap_err().to_string();
        assert!(refused.contains("nothing to commit"), "{refused}");
    }

    /// A page polls the fingerprint to notice a change it did not make itself, so it has to
    /// move for every kind of change and stand still when there was none.
    #[test]
    fn the_fingerprint_moves_only_when_something_did() {
        let dir = scratch_repo();
        let mut session = scratch_session(dir.path());
        let taken = |s: &Session| s.fingerprint(Some("a.rs")).unwrap();

        let first = taken(&session);
        assert_eq!(first, taken(&session), "nothing happened in between");

        // The open file is written again. Its status was already `modified`, so this moves
        // nothing but the file's own stamp: the case a status alone would miss.
        std::fs::write(
            dir.path().join("a.rs"),
            "fn a() {}\nfn b() {}\nfn c() {}\nfn d() {}\n",
        )
        .unwrap();
        let edited = taken(&session);
        assert_ne!(first, edited, "the open file was written");

        // A file nobody is looking at: this one the status carries.
        std::fs::write(dir.path().join("fresh.txt"), "hello\n").unwrap();
        let added = taken(&session);
        assert_ne!(edited, added, "a file appeared");

        // And a comment, which rewrites REVIEW.md.
        session
            .add_comment("a.rs", Some((2, 2)), "why b?", None, None)
            .unwrap();
        assert_ne!(added, taken(&session), "REVIEW.md was written");

        // A path from a request cannot lead out of the repository.
        assert!(session.fingerprint(Some("../outside")).is_err());
        // One that is simply not there is no error: the file may have just been deleted.
        assert!(session.fingerprint(Some("gone.rs")).is_ok());
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

    /// A file past the view's line limit is shown to that limit and says how long it is,
    /// with a comment past the cut still placed where it is.
    #[test]
    fn a_long_file_is_cut_at_the_limit_and_says_so() {
        let dir = scratch_repo();
        let long: String = (1..=MAX_VIEW_LINES + 5)
            .map(|n| format!("line {n}\n"))
            .collect();
        std::fs::write(dir.path().join("long.txt"), &long).unwrap();
        let mut s = scratch_session(dir.path());
        let last = MAX_VIEW_LINES + 5;
        s.add_comment("long.txt", Some((last, last)), "the end", None, None)
            .unwrap();
        let view = s.file_view("long.txt").unwrap();
        assert_eq!(view.total_lines, MAX_VIEW_LINES + 5);
        assert_eq!(view.text.lines().count(), MAX_VIEW_LINES);
        assert!(view.text.ends_with(&format!("line {MAX_VIEW_LINES}\n")));
        let comment = &view.comments[0];
        assert_eq!(comment.line, Some(last), "{comment:?}");
        assert_eq!(comment.state, crate::anchor::AnchorState::Exact);
        // A short file is whole, and says nothing.
        let view = s.file_view("a.rs").unwrap();
        assert_eq!(view.total_lines, view.text.lines().count());
    }

    #[test]
    fn session_end_to_end() {
        let dir = scratch_repo();
        let mut s = scratch_session(dir.path());
        assert_eq!(s.files, vec!["Makefile", "a.rs", "new.py"]);
        assert_eq!(s.status_letter("a.rs"), Some('M'));
        assert_eq!(s.status_letter("new.py"), Some('?'));
        assert_eq!(s.author.as_deref(), Some("Tester"));

        let c = s
            .add_comment("a.rs", Some((2, 2)), "why b?", None, None)
            .unwrap();
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
        let whole = s
            .add_comment("a.rs", None, "needs tests", None, None)
            .unwrap();
        assert_eq!((whole.line, whole.anchor.as_deref()), (None, None));
        let on_dir = s
            .add_comment("src", None, "too many modules", None, None)
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

    /// A comment left on a committed file names the commit, and is followed from there
    /// through later edits by the diff of that version against the file now.
    #[test]
    fn a_comment_on_a_commit_follows_its_line() {
        let dir = scratch_repo();
        let git = |args: &[&str]| {
            let out = Command::new("git")
                .arg("-C")
                .arg(dir.path())
                .args(args)
                .output()
                .unwrap();
            assert!(out.status.success(), "git {args:?}");
            String::from_utf8_lossy(&out.stdout).trim().to_string()
        };
        let v1 = "fn parse_review() {\n    let x = 1;\n}\n\nfn write_review() {\n    save();\n}\n";
        std::fs::write(dir.path().join("lib.rs"), v1).unwrap();
        git(&["add", "lib.rs"]);
        git(&["commit", "-q", "-m", "lib"]);
        let head = git(&["rev-parse", "--short", "HEAD"]);
        let mut s = scratch_session(dir.path());

        // On the file as committed: HEAD is recorded. On a file with edits: nothing is.
        let on_x = s
            .add_comment("lib.rs", Some((2, 2)), "why 1", None, None)
            .unwrap();
        assert_eq!(on_x.commit.as_deref(), Some(head.as_str()));
        let on_save = s
            .add_comment("lib.rs", Some((6, 6)), "save where", None, None)
            .unwrap();
        let edited = s
            .add_comment("a.rs", Some((1, 1)), "a", None, None)
            .unwrap();
        assert_eq!(edited.commit, None, "a.rs has changes in the working tree");
        assert!(
            s.review.render().contains(&format!("(at {head})")),
            "{}",
            s.review.render()
        );

        // Lines above, the commented line edited, and the other function's body gone.
        let v2 = "// a header\n// and more\nfn parse_review() {\n    let x = 10;\n}\n\nfn write_review() {\n}\n";
        std::fs::write(dir.path().join("lib.rs"), v2).unwrap();
        let view = s.file_view("lib.rs").unwrap();
        let placed = |c: &Comment| {
            view.comments
                .iter()
                .find(|a| &a.comment == c)
                .unwrap()
                .clone()
        };
        let x = placed(&on_x);
        assert_eq!((x.line, x.state), (Some(4), AnchorState::Moved), "{x:?}");
        let save = placed(&on_save);
        assert_eq!(save.state, AnchorState::Stale, "{save:?}");
        assert_eq!(
            save.line,
            Some(8),
            "just below the line above it that is left"
        );

        // Re-anchoring writes where it is now; the file has changes, so it names no commit,
        // and it takes the text its line has now.
        assert!(s.reanchor().unwrap() >= 1);
        let now = s
            .review
            .comments()
            .into_iter()
            .find(|c| c.text == "why 1")
            .unwrap()
            .clone();
        assert_eq!((now.line, now.commit.as_deref()), (Some(4), None));
        assert_eq!(now.anchor.as_deref(), Some("let x = 10;"));

        // A comment left in an old commit's diff counts its lines in that commit.
        let first = git(&["rev-parse", "--short", "HEAD~1"]);
        let old = s
            .add_comment("a.rs", Some((2, 2)), "b was here", None, Some(&first))
            .unwrap();
        assert_eq!(old.commit.as_deref(), Some(first.as_str()));
        assert_eq!(old.anchor.as_deref(), Some("fn b() {}"));
    }

    /// `OVERVIEW.md` is what the tool found out, written once the repository has held still,
    /// and read back on a fresh open; what people write in it stays.
    #[test]
    fn the_overview_file_is_kept_and_read_back() {
        let dir = scratch_repo();
        std::fs::write(
            dir.path().join("lib.rs"),
            "pub fn parse_review() {}\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn it_parses() { parse_review(); }\n}\n",
        )
        .unwrap();
        let mut s = scratch_session(dir.path());
        assert!(s.facts_file.is_none());
        // Nothing is written before the index is built, or before the repository settles.
        assert!(!s.keep_facts(None).unwrap());
        s.index();
        assert!(!s.keep_facts(None).unwrap(), "just started holding still");
        s.settle = s.settle.map(|(f, _)| (f, Instant::now() - SETTLE));
        assert!(
            s.keep_facts(None).unwrap(),
            "held still long enough: written"
        );
        assert!(
            !s.keep_facts(None).unwrap(),
            "and not again at the same fingerprint"
        );
        let text = std::fs::read_to_string(dir.path().join("OVERVIEW.md")).unwrap();
        assert!(text.contains("### lib.rs\n- it parses\n"), "{text}");
        assert!(text.contains("- 4 files, 1 test in 1 file"), "{text}");
        assert!(text.contains("As of commit "), "{text}");
        assert_eq!(
            s.as_of().map(|a| a.commit),
            None,
            "the index is ready: nothing is as of"
        );

        // Someone writes under About; the next write keeps it.
        let with_about = text.replace(
            "\n## Facts",
            "\n## About\nA scratch repository.\n\n## Facts",
        );
        std::fs::write(dir.path().join("OVERVIEW.md"), &with_about).unwrap();
        std::fs::write(dir.path().join("b.rs"), "pub fn other_thing() {}\n").unwrap();
        s.refresh().unwrap();
        assert!(s.write_facts(None).unwrap());
        let text = std::fs::read_to_string(dir.path().join("OVERVIEW.md")).unwrap();
        assert!(text.contains("## About\nA scratch repository.\n"), "{text}");
        assert!(
            text.contains("- 6 files"),
            "b.rs, and OVERVIEW.md itself: {text}"
        );

        // A fresh session reads it, and says how far behind it is until its index is built.
        let fresh = scratch_session(dir.path());
        let facts = fresh.facts_file.as_ref().expect("read at open");
        assert_eq!(facts.about, vec!["A scratch repository.".to_string()]);
        let as_of = fresh.as_of().expect("no index yet");
        assert_eq!(as_of.commit, s.repo.short_hash("HEAD").ok());
        assert_eq!(as_of.commits_since, 0);
    }

    /// Every served repository has a session and they share one config file. A check saved
    /// from one and a theme saved from another must both be there afterwards, and the same
    /// check found in a second repository joins the first one's entry rather than repeating it.
    #[test]
    fn sessions_sharing_a_config_file_keep_each_others_changes() {
        let a = scratch_repo();
        let b = scratch_repo();
        let config = a.path().join(".git").join("shared-config.toml");
        let mut one = Session::open_with_config(a.path(), Some(config.clone())).unwrap();
        let mut two = Session::open_with_config(b.path(), Some(config.clone())).unwrap();
        let check = crate::config::Check {
            name: "make test".into(),
            run: "make test".into(),
            ..Default::default()
        };
        one.save_check(&check).unwrap();
        two.set_theme("Nord").unwrap();
        two.save_check(&check).unwrap();

        let saved = Config::load_from(&config).unwrap();
        assert_eq!(saved.theme, "Nord");
        assert_eq!(saved.analysis.len(), 1, "{:?}", saved.analysis);
        assert_eq!(saved.analysis[0].repos.len(), 2, "{:?}", saved.analysis);
        assert!(crate::analysis::applies(&saved.analysis[0], a.path()));
        assert!(crate::analysis::applies(&saved.analysis[0], b.path()));
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
