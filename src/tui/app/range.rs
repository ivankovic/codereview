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
use crate::repo::{Repo, Worktree};
use crate::targets::{Base, Head, Heads};

/// One row of the range picker.
#[derive(Debug, Clone, PartialEq)]
pub enum RangeChoice {
    Heading(String),
    /// Something said that leads nowhere: that the forge is being asked, or why it cannot be.
    Note(String),
    Stretch(Stretch),
    Pull(PullRequest),
    /// Something to review, which the next step compares with a base. For a worktree, what
    /// it has checked out, and whether it is this one.
    Head(Head, Option<Worktree>),
    /// What to compare the head with.
    Base(Base),
}

impl RangeChoice {
    fn selectable(&self) -> bool {
        !matches!(self, RangeChoice::Heading(_) | RangeChoice::Note(_))
    }

    /// What the filter matches against.
    fn text(&self) -> String {
        match self {
            RangeChoice::Heading(_) | RangeChoice::Note(_) => String::new(),
            RangeChoice::Stretch(s) => s.name().to_string(),
            RangeChoice::Pull(pr) => format!("#{} {} {}", pr.number, pr.title, pr.author),
            RangeChoice::Head(h, w) => format!(
                "{} {}",
                h.label(),
                w.as_ref()
                    .and_then(|w| w.branch.clone())
                    .unwrap_or_default()
            ),
            RangeChoice::Base(b) => b.label.clone(),
        }
    }
}

/// Where the picker is: choosing what to review, or what to compare a head with.
#[derive(Debug, Clone, PartialEq)]
pub enum Step {
    Head,
    Base(Head),
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
    pub step: Step,
    pub rows: ListState<RangeChoice>,
    pulls: Pulls,
    /// What can be reviewed, read when the picker opened.
    heads: Heads,
    /// What the chosen head can be compared with, in the second step.
    bases: Vec<Base>,
    /// What the rows are narrowed to, as typed after `/`.
    pub filter: String,
    /// Whether keys are going into the filter.
    pub filtering: bool,
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

/// A range's screen in one focus: what it is, the files of it that focus holds, and the
/// comments on them, the ones on the whole pull request with the code.
#[allow(clippy::too_many_arguments)]
fn range_state(
    focus: Focus,
    stretch: Stretch,
    pull: Option<PullRequest>,
    label: String,
    target: DiffTarget,
    sized: Vec<(ChangedFile, Option<omnidiff::Stats>)>,
    comments: std::result::Result<Vec<ForgeComment>, &String>,
) -> RangeState {
    let (comments, unread) = match comments {
        Ok(c) => (c, None),
        Err(e) => (Vec::new(), Some(e.clone())),
    };
    let what = match focus {
        Focus::Docs => "Documents it changes",
        Focus::Data => "Datasets it changes",
        _ => "Files it changes",
    };
    let (files, sizes): (Vec<ChangedFile>, Vec<Option<omnidiff::Stats>>) =
        sized.into_iter().unzip();
    let total: omnidiff::Stats = sizes.iter().flatten().copied().sum();
    let size = if files.is_empty() {
        String::new()
    } else {
        format!(" · {}", crate::diff::size_label(&total))
    };
    let mut rows = vec![RangeRow::Heading(format!("{what} {}{size}", files.len()))];
    if files.is_empty() {
        rows.push(RangeRow::Note(match focus {
            Focus::Docs => "It changes no document.".into(),
            Focus::Data => "It changes no dataset.".into(),
            _ => "It changes no file.".into(),
        }));
    }
    rows.extend((0..files.len()).map(RangeRow::File));
    if pull.is_some() && (focus == Focus::Code || !comments.is_empty()) {
        rows.push(RangeRow::Heading(format!("Comments {}", comments.len())));
        match unread {
            Some(e) => rows.push(RangeRow::Note(format!("They could not be read: {e}"))),
            None if comments.is_empty() => {
                rows.push(RangeRow::Note("Nobody has commented on it.".into()))
            }
            None => rows.extend((0..comments.len()).map(RangeRow::Comment)),
        }
    }
    let mut rows = ListState::new(rows);
    super::end_to_end::settle(&mut rows, 1, RangeRow::selectable);
    RangeState {
        stretch,
        pull,
        label,
        target,
        files,
        sizes,
        comments,
        rows,
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
    /// Each file's size, as omnidiff counts its lines; `None` for one that is not text.
    pub sizes: Vec<Option<omnidiff::Stats>>,
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
        let heads = match crate::targets::heads(repo) {
            Ok(h) => h,
            Err(e) => {
                self.error(format!("{e:#}"));
                return;
            }
        };
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
            step: Step::Head,
            rows: ListState::new(Vec::new()),
            pulls,
            heads,
            bases: Vec::new(),
            filter: String::new(),
            filtering: false,
        };
        picker.rebuild();
        let here = picker
            .rows
            .items
            .iter()
            .position(|r| matches!(r, RangeChoice::Stretch(s) if *s == self.stretch));
        picker.rows.cursor = here.unwrap_or(0);
        super::end_to_end::settle(&mut picker.rows, 1, RangeChoice::selectable);
        self.range_picker = Some(picker);
    }

    pub(super) fn handle_range_picker_key(&mut self, key: KeyEvent) {
        let height = self.measured.main_height.max(1) as isize;
        let Some(picker) = &mut self.range_picker else {
            return;
        };
        // Typing after `/` narrows the rows; Enter keeps the filter and goes back to moving,
        // Esc drops it.
        if picker.filtering {
            match key.code {
                KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                    picker.filter.push(c)
                }
                KeyCode::Backspace => {
                    picker.filter.pop();
                }
                KeyCode::Enter | KeyCode::Down => picker.filtering = false,
                KeyCode::Esc => {
                    picker.filtering = false;
                    picker.filter.clear();
                }
                _ => return,
            }
            picker.rebuild();
            return;
        }
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
            KeyCode::Char('/') => {
                picker.filtering = true;
                picker.rebuild();
            }
            // From the second step, back to the first; from the first, away.
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Backspace
                if matches!(picker.step, Step::Base(_)) =>
            {
                picker.step = Step::Head;
                picker.filter.clear();
                picker.rebuild();
            }
            KeyCode::Esc | KeyCode::Char('q') => self.range_picker = None,
            KeyCode::Enter => {
                let choice = picker.rows.current().cloned();
                match choice {
                    Some(RangeChoice::Stretch(stretch)) => {
                        self.range_picker = None;
                        self.set_stretch(stretch);
                    }
                    Some(RangeChoice::Pull(pr)) => {
                        self.range_picker = None;
                        self.fetch_pull(pr);
                    }
                    Some(RangeChoice::Head(head, _)) => {
                        match crate::targets::bases(&self.session.repo, &head) {
                            Ok(bases) => {
                                if let Some(picker) = &mut self.range_picker {
                                    picker.bases = bases;
                                    picker.step = Step::Base(head);
                                    picker.filter.clear();
                                    picker.rebuild();
                                }
                            }
                            Err(e) => self.error(format!("{e:#}")),
                        }
                    }
                    Some(RangeChoice::Base(base)) => {
                        let Step::Base(head) = picker.step.clone() else {
                            return;
                        };
                        self.range_picker = None;
                        let label = match &head {
                            Head::Worktree { name, .. } => {
                                format!("taking worktree {name} as it is on disk…")
                            }
                            other => format!("comparing {} with {}…", other.label(), base.label),
                        };
                        self.later(label, move |app| {
                            match crate::targets::range(&app.session.repo, &head, &base.rev) {
                                Ok(stretch) => app.open_range(stretch, None, Ok(Vec::new())),
                                Err(e) => app.error(format!("{e:#}")),
                            }
                        });
                    }
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
        self.show_loading(format!("fetching pull request #{}…", pr.number));
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
                picker.rebuild();
            }
            changed = true;
        }
        if let Some(rx) = &self.asking.pull
            && let Ok(fetched) = rx.try_recv()
        {
            self.asking.pull = None;
            self.done_loading();
            match fetched {
                Ok((pr, stretch, comments)) => self.later(
                    format!("reading pull request #{}…", pr.number),
                    move |app| app.open_range(stretch, Some(pr), comments),
                ),
                Err(e) => self.error(e),
            }
            changed = true;
        }
        changed
    }

    /// Makes `stretch` the range, and opens it where it is first read: the tests, on what it
    /// did to them. The code, the documents and the data each get its screen of the files they
    /// hold, a digit away.
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
        let target = DiffTarget::Revisions {
            from: span.base.clone(),
            to,
        };
        let label = match &pull {
            Some(pr) => format!("{} into {}", pr.head_ref, pr.base_ref),
            None => stretch.title(),
        };
        // Each file's size, from omnidiff's diff of it, once for all three focuses.
        let sized: Vec<(ChangedFile, Option<omnidiff::Stats>)> = files
            .into_iter()
            .map(|f| {
                let stats = self.session.diff_stats(&target, &f).ok().flatten();
                (f, stats)
            })
            .collect();
        for focus in [Focus::Code, Focus::Docs, Focus::Data] {
            let held = |path: &str| focus == Focus::Code || focus.holds(path);
            let state = range_state(
                focus,
                stretch.clone(),
                pull.clone(),
                label.clone(),
                target.clone(),
                sized
                    .iter()
                    .filter(|(f, _)| held(&f.path))
                    .cloned()
                    .collect(),
                comments.as_ref().map(|c| {
                    c.iter()
                        .filter(|c| c.path.as_deref().map_or(focus == Focus::Code, held))
                        .cloned()
                        .collect()
                }),
            );
            self.put_range_on(focus, state);
        }
        self.stretch = stretch;
        self.refresh_end_to_end(false);
        self.switch_focus(Focus::Tests);
        // On the overview, which is what the range did to the tests.
        if let Some(Screen::EndToEnd(state)) = self.screens.last_mut() {
            state.rows.cursor = 0;
            state.main = false;
            state.test = None;
        }
        self.info(format!(
            "{}: 2 its code, 3 its documents, 4 its data",
            self.stretch.title()
        ));
    }

    /// Puts a range's screen on `focus`'s stack, over its first screen, in place of the one
    /// an earlier range left there and whatever was opened over that.
    fn put_range_on(&mut self, focus: Focus, state: RangeState) {
        let stack = if focus == self.focus {
            &mut self.screens
        } else {
            &mut self.parked[focus.index()]
        };
        if let Some(at) = stack.iter().position(|s| matches!(s, Screen::Range(_))) {
            stack.truncate(at);
        }
        if stack.is_empty() {
            stack.push(Screen::Explorer);
        }
        stack.push(Screen::Range(Box::new(state)));
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
                        let sizes = state.sizes.clone();
                        self.push_omni(target, files, sizes, i, None);
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
                                let sizes = state.sizes.clone();
                                let row = c.line.map(|l| l.saturating_sub(1) as usize);
                                self.push_omni(target, files, sizes, at, row);
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
impl RangePicker {
    /// The rows for the step the picker is on, narrowed to the filter, the cursor kept on
    /// what it was on where that is still there.
    fn rebuild(&mut self) {
        let at = self.rows.current().cloned();
        let mut rows = match &self.step {
            Step::Head => self.head_rows(),
            Step::Base(_) => {
                let mut rows = vec![RangeChoice::Heading("Against".into())];
                rows.extend(self.bases.iter().cloned().map(RangeChoice::Base));
                rows
            }
        };
        if !self.filter.is_empty() {
            let wanted = self.filter.to_lowercase();
            rows.retain(|r| !r.selectable() || r.text().to_lowercase().contains(&wanted));
            // A heading left with nothing under it says nothing.
            let mut kept = Vec::new();
            for (i, r) in rows.iter().enumerate() {
                let empty_heading = matches!(r, RangeChoice::Heading(_))
                    && !rows[i + 1..]
                        .iter()
                        .take_while(|n| !matches!(n, RangeChoice::Heading(_)))
                        .any(RangeChoice::selectable);
                if !empty_heading && !matches!(r, RangeChoice::Note(_)) {
                    kept.push(r.clone());
                }
            }
            rows = kept;
            if rows.is_empty() {
                rows.push(RangeChoice::Note(format!(
                    "Nothing matches “{}”.",
                    self.filter
                )));
            }
        }
        self.rows = ListState::new(rows);
        if let Some(at) = at.and_then(|c| self.rows.items.iter().position(|r| *r == c)) {
            self.rows.cursor = at;
        }
        super::end_to_end::settle(&mut self.rows, 1, RangeChoice::selectable);
    }

    /// What can be reviewed: the forge's pull requests, every worktree as it is on disk, the
    /// branches local and remote, the tags, and the stretches of this checkout.
    fn head_rows(&self) -> Vec<RangeChoice> {
        let mut rows = vec![RangeChoice::Heading("Pull requests".into())];
        match &self.pulls {
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
        rows.push(RangeChoice::Heading(
            "Worktrees, as they are on disk".into(),
        ));
        rows.extend(
            self.heads
                .worktrees
                .iter()
                .map(|(h, w)| RangeChoice::Head(h.clone(), Some(w.clone()))),
        );
        for (title, heads) in [
            ("Branches", &self.heads.branches),
            ("Remote branches", &self.heads.remote_branches),
            ("Tags", &self.heads.tags),
        ] {
            if heads.is_empty() {
                continue;
            }
            rows.push(RangeChoice::Heading(title.into()));
            rows.extend(heads.iter().cloned().map(|h| RangeChoice::Head(h, None)));
        }
        rows.push(RangeChoice::Heading("Stretches of this checkout".into()));
        rows.extend(Stretch::CHOICES.iter().cloned().map(RangeChoice::Stretch));
        rows
    }
}
