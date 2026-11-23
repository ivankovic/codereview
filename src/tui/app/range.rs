//! The review range, picked from the breadcrumb with `gp`: one of the stretches the tests are
//! seen through, a local branch compared with another as though it were a pull request into
//! it, or one of the forge's pull requests. The forge is asked on a thread of its own, since
//! it is a network away, and so is a pull request's fetch; the screen never waits for either.
//!
//! A pull request or a comparison, once picked, is the range the tests are seen through, and
//! opens over the code on a screen of its own: what it is, the files it changes, each opening
//! its diff across the range, and, for a pull request, its comments, read and never written.

use std::path::Path;
use std::sync::mpsc::{Receiver, channel};

use anyhow::Context;

use super::*;
use crate::forge::{ForgeComment, PullRequest};
use crate::repo::Repo;

/// One row of the range picker.
#[derive(Debug, Clone, PartialEq)]
pub enum RangeChoice {
    Heading(String),
    /// Something said that leads nowhere: that the forge is being asked, or why it cannot be.
    Note(String),
    Stretch(Stretch),
    /// `head` compared with `base`, as a pull request from one into the other.
    Compare {
        base: String,
        head: String,
    },
    Pull(PullRequest),
}

impl RangeChoice {
    fn selectable(&self) -> bool {
        !matches!(self, RangeChoice::Heading(_) | RangeChoice::Note(_))
    }
}

/// What the forge said about its pull requests, or that it has not yet.
#[derive(Debug, Clone)]
enum Pulls {
    /// `origin` is not on a forge the tool knows.
    NoForge,
    Asking,
    Answered(std::result::Result<Vec<PullRequest>, String>),
}

pub struct RangePicker {
    pub rows: ListState<RangeChoice>,
    pulls: Pulls,
    /// The comparisons, worked out when the picker opened.
    compare: Vec<(String, String)>,
}

/// One row of a range's own screen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RangeRow {
    Heading(String),
    Note(String),
    /// An index into the files it changes.
    File(usize),
    /// An index into its comments.
    Comment(usize),
}

impl RangeRow {
    fn selectable(&self) -> bool {
        matches!(self, RangeRow::File(_) | RangeRow::Comment(_))
    }
}

/// A pull request or a comparison, on a screen of its own over the code.
pub struct RangeState {
    pub stretch: Stretch,
    /// The pull request, when it is one.
    pub pull: Option<PullRequest>,
    /// Its two sides as the heading says them.
    pub label: String,
    /// What its diffs are of: from where the two sides parted to the head.
    pub target: DiffTarget,
    pub files: Vec<ChangedFile>,
    pub comments: Vec<ForgeComment>,
    pub rows: ListState<RangeRow>,
}

/// A pull request made ready to review: its commits here, the range it is, and its comments
/// or why they could not be read.
type Fetched = (
    PullRequest,
    Stretch,
    std::result::Result<Vec<ForgeComment>, String>,
);

/// What is being asked on another thread.
#[derive(Default)]
pub struct Asking {
    pulls: Option<Receiver<std::result::Result<Vec<PullRequest>, String>>>,
    pull: Option<Receiver<std::result::Result<Fetched, String>>>,
}

impl App {
    /// The range picker, with the forge asked for its pull requests while it is up.
    pub(super) fn open_range_picker(&mut self) {
        let repo = &self.session.repo;
        let current = repo.head_label();
        let branches = repo.branches().unwrap_or_default();
        let mut compare: Vec<(String, String)> = branches
            .iter()
            .filter(|b| !b.current)
            .take(20)
            .map(|b| (current.clone(), b.name.clone()))
            .collect();
        if let Some(default) = repo.default_branch()
            && default != current
            && branches.iter().any(|b| b.name == current)
        {
            compare.insert(0, (default, current.clone()));
        }
        let pulls = match crate::forge::for_repo(repo) {
            Some(_) => {
                let (tx, rx) = channel();
                let root = self.session.root().to_path_buf();
                std::thread::spawn(move || {
                    let answer = Repo::open(&root)
                        .and_then(|repo| {
                            crate::forge::for_repo(&repo)
                                .context("origin is not on a forge")?
                                .pull_requests()
                        })
                        .map_err(|e| format!("{e:#}"));
                    let _ = tx.send(answer);
                });
                self.asking.pulls = Some(rx);
                Pulls::Asking
            }
            None => Pulls::NoForge,
        };
        let mut picker = RangePicker {
            rows: ListState::new(Vec::new()),
            pulls,
            compare,
        };
        picker.rows = ListState::new(picker_rows(&picker));
        let here = picker
            .rows
            .items
            .iter()
            .position(|r| match (r, &self.stretch) {
                (RangeChoice::Stretch(s), current) => s == current,
                (
                    RangeChoice::Compare { base, head },
                    Stretch::Range {
                        base: b, head: h, ..
                    },
                ) => base == b && head == h,
                _ => false,
            });
        picker.rows.cursor = here.unwrap_or(0);
        super::end_to_end::settle(&mut picker.rows, 1, RangeChoice::selectable);
        self.range_picker = Some(picker);
    }

    pub(super) fn handle_range_picker_key(&mut self, key: KeyEvent) {
        let height = self.measured.main_height.max(1) as isize;
        let Some(picker) = &mut self.range_picker else {
            return;
        };
        let before = picker.rows.cursor;
        if picker.rows.handle_nav(key, height) {
            let direction = picker.rows.cursor as isize - before as isize;
            super::end_to_end::settle(
                &mut picker.rows,
                if direction < 0 { -1 } else { 1 },
                RangeChoice::selectable,
            );
            return;
        }
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => self.range_picker = None,
            KeyCode::Enter => {
                let choice = picker.rows.current().cloned();
                self.range_picker = None;
                match choice {
                    Some(RangeChoice::Stretch(stretch)) => self.set_stretch(stretch),
                    Some(RangeChoice::Compare { base, head }) => {
                        let stretch = Stretch::Range {
                            base,
                            head,
                            pull: None,
                        };
                        self.open_range(stretch, None, Ok(Vec::new()));
                    }
                    Some(RangeChoice::Pull(pr)) => self.fetch_pull(pr),
                    _ => {}
                }
            }
            _ => {}
        }
    }

    /// Makes `stretch` the range the tests are seen through, and reads them again.
    pub(super) fn set_stretch(&mut self, stretch: Stretch) {
        self.stretch = stretch;
        self.refresh_end_to_end(false);
        self.info(format!("marking {}", self.stretch.title()));
    }

    /// Fetches a pull request's commits where they are missing, and its comments, on another
    /// thread; `tick` opens it when they are here.
    fn fetch_pull(&mut self, pr: PullRequest) {
        let (tx, rx) = channel();
        let root = self.session.root().to_path_buf();
        self.info(format!("fetching pull request #{}…", pr.number));
        std::thread::spawn(move || {
            let fetched = Repo::open(&root)
                .and_then(|repo| {
                    let forge =
                        crate::forge::for_repo(&repo).context("origin is not on a forge")?;
                    let stretch = crate::forge::review_range(&repo, forge.as_ref(), &pr)?;
                    let comments = forge.comments(pr.number).map_err(|e| format!("{e:#}"));
                    Ok((pr, stretch, comments))
                })
                .map_err(|e| format!("{e:#}"));
            let _ = tx.send(fetched);
        });
        self.asking.pull = Some(rx);
    }

    /// Takes in what the threads found. True when something on screen changed.
    pub(super) fn take_answers(&mut self) -> bool {
        let mut changed = false;
        if let Some(rx) = &self.asking.pulls
            && let Ok(answer) = rx.try_recv()
        {
            self.asking.pulls = None;
            if let Some(picker) = &mut self.range_picker {
                picker.pulls = Pulls::Answered(answer);
                let at = picker.rows.current().cloned();
                picker.rows = ListState::new(picker_rows(picker));
                if let Some(at) = at.and_then(|c| picker.rows.items.iter().position(|r| *r == c)) {
                    picker.rows.cursor = at;
                }
            }
            changed = true;
        }
        if let Some(rx) = &self.asking.pull
            && let Ok(fetched) = rx.try_recv()
        {
            self.asking.pull = None;
            match fetched {
                Ok((pr, stretch, comments)) => self.open_range(stretch, Some(pr), comments),
                Err(e) => self.error(e),
            }
            changed = true;
        }
        changed
    }

    /// Makes `stretch` the range, and opens its own screen over the code.
    fn open_range(
        &mut self,
        stretch: Stretch,
        pull: Option<PullRequest>,
        comments: std::result::Result<Vec<ForgeComment>, String>,
    ) {
        let span = match crate::spec::span(&self.session, &stretch) {
            Ok(span) => span,
            Err(e) => {
                self.error(format!("{e:#}"));
                return;
            }
        };
        let to = span.to.clone().unwrap_or_default();
        let files = match self.session.repo.diff_files(&span.base, &to) {
            Ok(files) => files,
            Err(e) => {
                self.error(format!("{e:#}"));
                return;
            }
        };
        let (comments, unread) = match comments {
            Ok(c) => (c, None),
            Err(e) => (Vec::new(), Some(e)),
        };
        let mut rows = vec![RangeRow::Heading(format!(
            "Files it changes {}",
            files.len()
        ))];
        if files.is_empty() {
            rows.push(RangeRow::Note("It changes no file.".into()));
        }
        rows.extend((0..files.len()).map(RangeRow::File));
        if pull.is_some() {
            rows.push(RangeRow::Heading(format!("Comments {}", comments.len())));
            match unread {
                Some(e) => rows.push(RangeRow::Note(format!("They could not be read: {e}"))),
                None if comments.is_empty() => {
                    rows.push(RangeRow::Note("Nobody has commented on it.".into()))
                }
                None => rows.extend((0..comments.len()).map(RangeRow::Comment)),
            }
        }
        let label = match &stretch {
            Stretch::Range { base, head, .. } => match &pull {
                Some(pr) => format!("{} into {}", pr.head_ref, pr.base_ref),
                None => format!("{head} against {base}"),
            },
            other => other.title(),
        };
        let mut rows = ListState::new(rows);
        super::end_to_end::settle(&mut rows, 1, RangeRow::selectable);
        let state = RangeState {
            stretch: stretch.clone(),
            pull,
            label,
            target: DiffTarget::Revisions {
                from: span.base.clone(),
                to,
            },
            files,
            comments,
            rows,
        };
        self.stretch = stretch;
        self.refresh_end_to_end(false);
        self.switch_focus(Focus::Code);
        if matches!(self.screen(), Screen::Range(_)) {
            self.screens.pop();
        }
        self.screens.push(Screen::Range(Box::new(state)));
    }

    pub(super) fn handle_range_key(&mut self, key: KeyEvent) {
        let height = self.measured.list_height.max(1) as isize;
        let Some(Screen::Range(state)) = self.screens.last_mut() else {
            return;
        };
        let before = state.rows.cursor;
        if state.rows.handle_nav(key, height) {
            let direction = state.rows.cursor as isize - before as isize;
            super::end_to_end::settle(
                &mut state.rows,
                if direction < 0 { -1 } else { 1 },
                RangeRow::selectable,
            );
            return;
        }
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => self.pop_screen(),
            KeyCode::Char('o') => {
                let url = state.pull.as_ref().map(|p| p.url.clone());
                match url {
                    Some(url) => match super::end_to_end::open_externally(Path::new(&url)) {
                        Ok(()) => self.info(format!("opened {url}")),
                        Err(e) => self.error(format!("cannot open {url}: {e:#}")),
                    },
                    None => self.info("a comparison is not on the forge"),
                }
            }
            KeyCode::Enter | KeyCode::Char('l') | KeyCode::Right => {
                match state.rows.current().cloned() {
                    Some(RangeRow::File(i)) => {
                        let target = state.target.clone();
                        let files = state.files.clone();
                        self.push_diff(target, files, i);
                    }
                    Some(RangeRow::Comment(i)) => {
                        let Some(c) = state.comments.get(i).cloned() else {
                            return;
                        };
                        let Some(path) = c.path else {
                            self.info("that comment is on the pull request as a whole");
                            return;
                        };
                        // Its line counts in the pull request's head: the diff's after side.
                        let target = state.target.clone();
                        let files = state.files.clone();
                        match files.iter().position(|f| f.path == path) {
                            Some(at) => {
                                self.push_diff(target, files, at);
                                if let (Some(Screen::Diff(d)), Some(line)) =
                                    (self.screens.last_mut(), c.line)
                                {
                                    d.view.go_to_after_line(line.saturating_sub(1) as usize);
                                }
                            }
                            None => self.jump_to(&path, c.line.unwrap_or(1) as usize),
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
}

/// The picker's rows: the stretches, the comparisons, and what the forge said.
fn picker_rows(picker: &RangePicker) -> Vec<RangeChoice> {
    let mut rows = vec![RangeChoice::Heading("Stretches".into())];
    rows.extend(Stretch::CHOICES.iter().cloned().map(RangeChoice::Stretch));
    rows.push(RangeChoice::Heading("Comparisons".into()));
    if picker.compare.is_empty() {
        rows.push(RangeChoice::Note("There is no other branch.".into()));
    }
    rows.extend(
        picker
            .compare
            .iter()
            .map(|(base, head)| RangeChoice::Compare {
                base: base.clone(),
                head: head.clone(),
            }),
    );
    rows.push(RangeChoice::Heading("Pull requests".into()));
    match &picker.pulls {
        Pulls::NoForge => rows.push(RangeChoice::Note(
            "origin is not on GitHub, so there are none to ask for.".into(),
        )),
        Pulls::Asking => rows.push(RangeChoice::Note("asking GitHub…".into())),
        Pulls::Answered(Err(e)) => rows.push(RangeChoice::Note(e.clone())),
        Pulls::Answered(Ok(pulls)) if pulls.is_empty() => {
            rows.push(RangeChoice::Note("None is open.".into()))
        }
        Pulls::Answered(Ok(pulls)) => rows.extend(pulls.iter().cloned().map(RangeChoice::Pull)),
    }
    rows
}
