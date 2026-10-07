//! The screens beyond the explorer: the log, the changes list, a diff, the review list, the
//! notes list, and a list of places to jump to. Each is a piece of state on the app's screen
//! stack, what puts it there, and the keys that drive it while it is on top.
//!
//! A child module of `app`, so these go on using the app's own methods without everything
//! having to be made public for them.

use super::*;
use std::collections::HashMap;

pub struct LogState {
    /// `None` for the whole repository, or the file whose history this is.
    pub path: Option<String>,
    pub commits: ListState<Commit>,
    /// Files of the selected commit, loaded on demand.
    pub files: ListState<ChangedFile>,
    pub files_of: Option<String>,
    pub focus_files: bool,
    pub exhausted: bool,
}

pub struct ChangesState {
    pub target: DiffTarget,
    pub files: ListState<ChangedFile>,
}

pub struct ReviewState {
    pub items: ListState<Anchored>,
    pub show_completed: bool,
}

pub struct NotesState {
    pub items: ListState<Note>,
}

pub struct DiffState {
    pub view: DiffView,
    /// The list the file was picked from, for `]` / `[`.
    pub siblings: Vec<ChangedFile>,
    pub index: usize,
}

/// One place to jump to: a definition, an occurrence, or a symbol.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hit {
    pub path: String,
    /// 1-based.
    pub line: usize,
    /// 0-based byte column.
    pub column: usize,
    /// `function total`, or empty for a plain occurrence.
    pub label: String,
    pub text: String,
}

pub struct LocationsState {
    pub title: String,
    pub items: ListState<Hit>,
}

/// The specification: what a stretch of history did to the tests, then every test, folded by
/// level, area and file. One list of rows, headings and folds among them, so the cursor walks
/// the whole page and Enter opens or closes whatever it is on.
pub struct SpecState {
    pub stretch: Stretch,
    pub span: crate::spec::Span,
    pub total: usize,
    pub ignored: usize,
    pub rows: ListState<SpecRow>,
    /// What the rows are drawn from, kept to narrow them, fold them and search them.
    pub spec: crate::spec::Spec,
    /// The one area the tests are narrowed to, if any.
    pub area: Option<String>,
    /// Folds someone opened or closed, by key; the rest are open when they are small.
    pub open: HashMap<String, bool>,
    /// Words the tests are searched for; empty shows the folds instead.
    pub query: String,
}

#[derive(Debug, Clone)]
pub enum SpecRow {
    /// A section's title; alarming when it lists tests that now ask less.
    Heading(String, bool),
    /// A sentence under a heading; a finding when what it says is missing matters.
    Text(String, bool),
    /// A change, and how deep in the folds it is.
    Change(TestChange, u8),
    /// A part of the code, with what the tests do for it; Enter narrows to it.
    Area(crate::spec::Area),
    /// What a fold holds and how many; Enter opens or closes it.
    Fold {
        key: String,
        label: String,
        count: usize,
        skipped: usize,
        depth: u8,
        open: bool,
        /// A level's fold, drawn in the accent colour like a heading.
        level: bool,
    },
    Test(crate::spec::SpecTest, u8),
}

/// A group small enough to read whole: its fold starts open, and it is not split further.
const SMALL: usize = 12;

/// What a level's empty section says: an absent security or end-to-end suite is a finding.
fn level_says(level: Level) -> (&'static str, &'static str) {
    match level {
        Level::Security => (
            "Tests whose name or path is about signing in, secrets, permissions or attacks.",
            "No test is named for security: nothing here says what an attacker cannot do.",
        ),
        Level::EndToEnd => (
            "Tests that drive the whole system the way a person would: through a browser, or a real server.",
            "No test drives the whole system: nothing here shows it working the way someone would use it.",
        ),
        Level::Integration => (
            "Tests that reach the system through its public interface.",
            "None found.",
        ),
        Level::Unit => ("Everything else: one piece at a time.", "None found."),
    }
}

/// Items grouped by a key, in the order the keys sort.
fn grouped<T: Clone>(items: &[T], key: impl Fn(&T) -> String) -> Vec<(String, Vec<T>)> {
    let mut groups: std::collections::BTreeMap<String, Vec<T>> = Default::default();
    for item in items {
        groups.entry(key(item)).or_default().push(item.clone());
    }
    groups.into_iter().collect()
}

impl SpecState {
    fn is_open(&self, key: &str, default: bool) -> bool {
        self.open.get(key).copied().unwrap_or(default)
    }

    /// The rows again, after a fold, an area or the search changed, with the cursor kept on
    /// the same row as near as the new rows allow.
    pub fn rebuild(&mut self) {
        let (cursor, scroll) = (self.rows.cursor, self.rows.scroll);
        self.rows = ListState::new(spec_rows(self));
        self.rows.cursor = cursor.min(self.rows.items.len().saturating_sub(1));
        self.rows.scroll = scroll.min(self.rows.cursor);
    }

    /// A fold inside a level or a kind of change, closed until someone opens it: it is only
    /// there because what holds it was too long to show whole.
    fn fold(
        &self,
        rows: &mut Vec<SpecRow>,
        key: String,
        label: String,
        count: usize,
        skipped: usize,
        depth: u8,
    ) -> bool {
        let open = self.is_open(&key, false);
        rows.push(SpecRow::Fold {
            key,
            label,
            count,
            skipped,
            depth,
            open,
            level: false,
        });
        open
    }

    /// Tests by the area each is filed in, then by file; a group of one is not a fold, and a
    /// small enough set is listed as it is.
    fn by_area(
        &self,
        rows: &mut Vec<SpecRow>,
        tests: &[crate::spec::SpecTest],
        key: &str,
        depth: u8,
    ) {
        let areas = grouped(tests, |t| t.home.clone());
        if tests.len() <= SMALL || areas.len() == 1 {
            return self.by_file(rows, tests, key, depth);
        }
        for (area, these) in areas {
            let key = format!("{key}:{area}");
            let skipped = these.iter().filter(|t| t.test.ignored).count();
            if self.fold(rows, key.clone(), area, these.len(), skipped, depth) {
                self.by_file(rows, &these, &key, depth + 1);
            }
        }
    }

    fn by_file(
        &self,
        rows: &mut Vec<SpecRow>,
        tests: &[crate::spec::SpecTest],
        key: &str,
        depth: u8,
    ) {
        let files = grouped(tests, |t| t.test.path.clone());
        if tests.len() <= SMALL || files.len() == 1 {
            rows.extend(tests.iter().cloned().map(|t| SpecRow::Test(t, depth)));
            return;
        }
        for (path, these) in files {
            let key = format!("{key}:{path}");
            let skipped = these.iter().filter(|t| t.test.ignored).count();
            if self.fold(rows, key, path, these.len(), skipped, depth) {
                rows.extend(these.into_iter().map(|t| SpecRow::Test(t, depth + 1)));
            }
        }
    }
}

/// The rows of the specification, in the order the page shows them: what the stretch did
/// (the tests that ask less in full, the rest a fold per kind of change), the areas, then the
/// tests, folded by level, area and file, or what matches the search when there is one.
pub fn spec_rows(state: &SpecState) -> Vec<SpecRow> {
    let spec = &state.spec;
    let area = state.area.as_deref();
    let mut rows = vec![SpecRow::Heading(format!("In {}", spec.span.label), false)];
    if spec.changes.is_empty() {
        rows.push(SpecRow::Text(
            "Nothing: no test was added, changed or removed.".into(),
            false,
        ));
    }
    let (alarming, rest): (Vec<_>, Vec<_>) = spec
        .changes
        .iter()
        .cloned()
        .partition(|c| c.what.alarming());
    if !alarming.is_empty() {
        rows.push(SpecRow::Heading(
            format!("Asks less than before: {}", alarming.len()),
            true,
        ));
        rows.extend(alarming.into_iter().map(|c| SpecRow::Change(c, 1)));
    }
    // Everything else, a fold per kind of change, closed until someone opens one.
    let mut kinds: Vec<(What, Vec<TestChange>)> = Vec::new();
    for c in rest {
        match kinds.iter_mut().find(|(w, _)| *w == c.what) {
            Some((_, cs)) => cs.push(c),
            None => kinds.push((c.what, vec![c])),
        }
    }
    kinds.sort_by_key(|(w, _)| *w);
    for (what, cs) in kinds {
        let key = format!("kind:{}", what.word());
        let open = state.is_open(&key, false);
        rows.push(SpecRow::Fold {
            key: key.clone(),
            label: what.word().to_string(),
            count: cs.len(),
            skipped: 0,
            depth: 0,
            open,
            level: false,
        });
        if !open {
            continue;
        }
        let files = grouped(&cs, |c| c.path.clone());
        if cs.len() <= SMALL || files.len() == 1 {
            rows.extend(cs.into_iter().map(|c| SpecRow::Change(c, 1)));
            continue;
        }
        for (path, these) in files {
            if state.fold(&mut rows, format!("{key}:{path}"), path, these.len(), 0, 1) {
                rows.extend(these.into_iter().map(|c| SpecRow::Change(c, 2)));
            }
        }
    }

    rows.push(SpecRow::Text(String::new(), false));
    match area {
        Some(a) => rows.push(SpecRow::Heading(
            format!("Area {a}: the tests that count towards it (Esc: every area)"),
            false,
        )),
        None => {
            rows.push(SpecRow::Heading(
                "Tests by area: security, end to end, integration, unit; files no test names"
                    .into(),
                false,
            ));
            rows.extend(spec.areas.iter().cloned().map(SpecRow::Area));
        }
    }

    let pool: Vec<crate::spec::SpecTest> = spec
        .groups
        .iter()
        .flat_map(|g| g.files.iter().flat_map(|f| f.tests.iter()))
        .filter(|t| area.is_none_or(|a| t.areas.iter().any(|x| x == a)))
        .cloned()
        .collect();

    // Searching shows what matches instead of the folds: every word must be in the test's
    // sentence, its name or its file.
    let words: Vec<String> = state
        .query
        .split_whitespace()
        .map(str::to_lowercase)
        .collect();
    if !words.is_empty() {
        let hits: Vec<_> = pool
            .iter()
            .filter(|t| {
                let text =
                    format!("{} {} {}", t.test.label, t.test.name, t.test.path).to_lowercase();
                words.iter().all(|w| text.contains(w.as_str()))
            })
            .cloned()
            .collect();
        rows.push(SpecRow::Text(String::new(), false));
        rows.push(SpecRow::Heading(
            format!(
                "{} {} \"{}\" (Esc clears)",
                hits.len(),
                if hits.len() == 1 {
                    "test matches"
                } else {
                    "tests match"
                },
                state.query.trim()
            ),
            false,
        ));
        for group in &spec.groups {
            let these: Vec<_> = hits
                .iter()
                .filter(|t| t.level == group.level)
                .cloned()
                .collect();
            if these.is_empty() {
                continue;
            }
            rows.push(SpecRow::Heading(
                format!("{} {}", group.title, these.len()),
                false,
            ));
            rows.extend(these.into_iter().map(|t| SpecRow::Test(t, 1)));
        }
        return rows;
    }

    for group in &spec.groups {
        let tests: Vec<_> = pool
            .iter()
            .filter(|t| t.level == group.level)
            .cloned()
            .collect();
        rows.push(SpecRow::Text(String::new(), false));
        if tests.is_empty() {
            rows.push(SpecRow::Heading(format!("{} 0", group.title), false));
            match area {
                Some(a) => rows.push(SpecRow::Text(format!("None counts towards {a}."), false)),
                None => {
                    let finding = matches!(group.level, Level::Security | Level::EndToEnd);
                    rows.push(SpecRow::Text(level_says(group.level).1.into(), finding));
                }
            }
            continue;
        }
        let key = format!("level:{}", group.title);
        let open = state.is_open(&key, tests.len() <= SMALL);
        rows.push(SpecRow::Fold {
            key: key.clone(),
            label: group.title.clone(),
            count: tests.len(),
            skipped: tests.iter().filter(|t| t.test.ignored).count(),
            depth: 0,
            open,
            level: true,
        });
        if !open {
            continue;
        }
        rows.push(SpecRow::Text(level_says(group.level).0.into(), false));
        match area {
            Some(_) => state.by_file(&mut rows, &tests, &key, 1),
            None => state.by_area(&mut rows, &tests, &key, 1),
        }
    }
    rows
}

impl App {
    /// The specification for `stretch`, on top of the stack; the one already there is
    /// replaced, so cycling stretches does not pile screens up, and keeps its folds and its
    /// search.
    pub(super) fn push_spec(&mut self, stretch: Stretch) {
        let spec = self.session.spec(&stretch);
        let Some(spec) = self.report(spec) else {
            return;
        };
        let (open, query) = match self.screen() {
            Screen::Spec(s) => (s.open.clone(), s.query.clone()),
            _ => (HashMap::new(), String::new()),
        };
        let mut state = SpecState {
            stretch,
            span: spec.span.clone(),
            total: spec.total,
            ignored: spec.ignored,
            rows: ListState::new(Vec::new()),
            spec,
            area: None,
            open,
            query,
        };
        state.rows = ListState::new(spec_rows(&state));
        if matches!(self.screen(), Screen::Spec(_)) {
            self.screens.pop();
        }
        self.screens.push(Screen::Spec(Box::new(state)));
    }

    /// Searches the specification on top for `query`, as it is typed.
    pub(super) fn search_spec(&mut self, query: String) {
        if let Some(Screen::Spec(state)) = self.screens.last_mut() {
            state.query = query;
            state.rows = ListState::new(spec_rows(state));
        }
    }

    pub(super) fn handle_spec_key(&mut self, key: KeyEvent) {
        let height = self.measured.list_height.max(1) as isize;
        let Screen::Spec(state) = self.screens.last_mut().expect("screen") else {
            return;
        };
        if state.rows.handle_nav(key, height) {
            return;
        }
        match key.code {
            // The first ways back are out of the search, then out of the area.
            KeyCode::Char('q') | KeyCode::Esc if !state.query.is_empty() => {
                state.query.clear();
                state.rows = ListState::new(spec_rows(state));
            }
            KeyCode::Char('q') | KeyCode::Esc if state.area.is_some() => {
                state.area = None;
                state.rows = ListState::new(spec_rows(state));
            }
            KeyCode::Char('q') | KeyCode::Esc => self.pop_screen(),
            KeyCode::Char('/') => {
                let was = state.query.clone();
                self.prompt = Some(Prompt::new(
                    PromptKind::SpecSearch { was: was.clone() },
                    "search the tests",
                    &was,
                ));
            }
            // The files a test names, to jump to.
            KeyCode::Char('f') => {
                let Some(SpecRow::Test(t, _)) = state.rows.current().cloned() else {
                    self.info("f lists the files a test names; put the cursor on a test");
                    return;
                };
                if t.reaches.is_empty() {
                    self.info("this test names no code directly");
                    return;
                }
                let hits = t
                    .reaches
                    .iter()
                    .map(|path| Hit {
                        path: path.clone(),
                        line: 1,
                        column: 0,
                        label: crate::spec::area_of(path),
                        text: String::new(),
                    })
                    .collect();
                self.push_locations(format!("what {} names", t.test.label), hits);
            }
            KeyCode::Char('s') => {
                if matches!(state.stretch, Stretch::Commit(_)) {
                    self.info("this is one commit; L and the log pick another");
                    return;
                }
                let at = Stretch::CHOICES
                    .iter()
                    .position(|c| *c == state.stretch)
                    .unwrap_or(0);
                let next = Stretch::CHOICES[(at + 1) % Stretch::CHOICES.len()].clone();
                self.push_spec(next);
            }
            // A fold opens or closes; a change opens on the diff of its file across the
            // stretch, where what happened to it is shown; a test opens where it is.
            KeyCode::Enter | KeyCode::Char('l') | KeyCode::Right => {
                match state.rows.current().cloned() {
                    Some(SpecRow::Fold { key, open, .. }) => {
                        state.open.insert(key, !open);
                        state.rebuild();
                    }
                    Some(SpecRow::Change(c, _)) => {
                        let target = DiffTarget::Revisions {
                            from: state.span.base.clone(),
                            to: state.span.to.clone().unwrap_or_default(),
                        };
                        let file = ChangedFile {
                            status: FileStatus::Modified,
                            path: c.path.clone(),
                            old_path: None,
                        };
                        self.push_diff(target, vec![file], 0);
                    }
                    Some(SpecRow::Test(t, _)) => self.jump_to(&t.test.path, t.test.line.max(1)),
                    Some(SpecRow::Area(a)) => {
                        state.area = Some(a.name);
                        state.rows = ListState::new(spec_rows(state));
                    }
                    _ => {}
                }
            }
            // Closes the fold the cursor is in, from anywhere inside it.
            KeyCode::Char('h') | KeyCode::Left => {
                let at = state.rows.cursor;
                let depth = |r: &SpecRow| match r {
                    SpecRow::Fold { depth, .. } => Some(*depth),
                    SpecRow::Test(_, d) | SpecRow::Change(_, d) => Some(*d),
                    _ => None,
                };
                let Some(mine) = state.rows.current().and_then(depth) else {
                    return;
                };
                // The nearest open fold above that is shallower, without crossing into
                // another section.
                let mut parent = None;
                for i in (0..at).rev() {
                    match &state.rows.items[i] {
                        SpecRow::Heading(..) => break,
                        SpecRow::Fold {
                            depth, open: true, ..
                        } if *depth < mine => {
                            parent = Some(i);
                            break;
                        }
                        _ => {}
                    }
                }
                let target = match state.rows.current() {
                    Some(SpecRow::Fold { open: true, .. }) => Some(at),
                    _ => parent,
                };
                if let Some(i) = target {
                    if let SpecRow::Fold { key, .. } = &state.rows.items[i] {
                        state.open.insert(key.clone(), false);
                        state.rows.cursor = i;
                        state.rebuild();
                    }
                }
            }
            KeyCode::Char('o') => match state.rows.current().cloned() {
                Some(SpecRow::Change(c, _)) if c.what != What::Removed => {
                    if state.span.to.is_some() {
                        self.info("o opens the working tree; this change is in a commit");
                    } else {
                        self.jump_to(&c.path, c.line.max(1));
                    }
                }
                Some(SpecRow::Test(t, _)) => self.jump_to(&t.test.path, t.test.line.max(1)),
                _ => {}
            },
            _ => {}
        }
    }

    /// The tests that name something the open file defines, as places to jump to: the way
    /// from a file to what checks it.
    pub(super) fn push_tests_of_file(&mut self) {
        let Some(path) = self.viewer.as_ref().map(|v| v.path.clone()) else {
            self.info("open a file first");
            return;
        };
        let tests = self.session.tests_reaching(&path);
        let name = path.rsplit('/').next().unwrap_or(&path).to_string();
        if tests.is_empty() {
            self.info(format!("no test names anything in {name}"));
            return;
        }
        let hits = tests
            .into_iter()
            .map(|t| Hit {
                path: t.test.path.clone(),
                line: t.test.line,
                column: 0,
                label: t.level.title().to_lowercase(),
                text: t.test.label,
            })
            .collect::<Vec<_>>();
        let title = format!(
            "{} {} {name}",
            hits.len(),
            if hits.len() == 1 {
                "test names"
            } else {
                "tests name"
            }
        );
        // Not `push_locations`: that starts on the hit nearest the cursor in this file, and
        // these are mostly in other files.
        self.screens.push(Screen::Locations(LocationsState {
            title,
            items: ListState::new(hits),
        }));
    }

    pub(super) fn push_log(&mut self, path: Option<String>) {
        let commits = match &path {
            Some(p) => self.session.file_log(p, 0, LOG_PAGE),
            None => self.session.log(0, LOG_PAGE),
        };
        let Some(commits) = self.report(commits) else {
            return;
        };
        if commits.is_empty() {
            self.info(match &path {
                Some(p) => format!("{p} has no commits"),
                None => "no commits yet".to_string(),
            });
            return;
        }
        let exhausted = commits.len() < LOG_PAGE;
        let mut state = LogState {
            path,
            commits: ListState::new(commits),
            files: ListState::new(Vec::new()),
            files_of: None,
            focus_files: false,
            exhausted,
        };
        let outcome = Self::load_commit_files(&mut self.session, &mut state);
        self.report(outcome);
        self.screens.push(Screen::Log(state));
    }

    /// The files of the commit under the cursor. Takes the session rather than the whole
    /// app, so a caller holding the screen it belongs to can still call it: the two are
    /// different fields.
    fn load_commit_files(session: &mut Session, state: &mut LogState) -> Result<()> {
        let Some(commit) = state.commits.current() else {
            return Ok(());
        };
        if state.files_of.as_deref() == Some(commit.hash.as_str()) {
            return Ok(());
        }
        let hash = commit.hash.clone();
        let mut files = session.commit_files(&hash)?;
        if let Some(path) = &state.path {
            // Put the file whose history this is first.
            files.sort_by_key(|f| f.path != *path);
        }
        state.files = ListState::new(files);
        state.files_of = Some(hash);
        Ok(())
    }

    /// The next page of commits, when the cursor has reached the end of what is loaded.
    fn load_more_commits(session: &mut Session, state: &mut LogState) -> Result<()> {
        if state.exhausted {
            return Ok(());
        }
        let skip = state.commits.items.len();
        let more = match &state.path {
            Some(p) => session.file_log(p, skip, LOG_PAGE)?,
            None => session.log(skip, LOG_PAGE)?,
        };
        if more.len() < LOG_PAGE {
            state.exhausted = true;
        }
        state.commits.items.extend(more);
        Ok(())
    }

    pub(super) fn push_changes(&mut self, target: DiffTarget) {
        let Some(files) = self.report(self.session.changed_files(&target)) else {
            return;
        };
        if files.is_empty() {
            self.info(format!(
                "nothing {}",
                if target == DiffTarget::Staged {
                    "staged"
                } else {
                    "changed in the working tree"
                }
            ));
            return;
        }
        self.screens.push(Screen::Changes(ChangesState {
            target,
            files: ListState::new(files),
        }));
    }

    pub(super) fn push_diff(
        &mut self,
        target: DiffTarget,
        siblings: Vec<ChangedFile>,
        index: usize,
    ) {
        let Some(file) = siblings.get(index).cloned() else {
            return;
        };
        let Some(diff) = self.report(self.session.diff(&target, &file)) else {
            return;
        };
        let comments = self.session.anchor_in(&file.path, &diff.after.text);
        let view = DiffView::new(target, file, diff, comments, &self.theme.syntax);
        self.screens.push(Screen::Diff(Box::new(DiffState {
            view,
            siblings,
            index,
        })));
    }

    pub(super) fn replace_diff(&mut self, index: usize) {
        let Screen::Diff(d) = self.screen() else {
            return;
        };
        if index >= d.siblings.len() {
            return;
        }
        let target = d.view.target.clone();
        let siblings = d.siblings.clone();
        self.screens.pop();
        self.push_diff(target, siblings, index);
    }

    pub(super) fn push_review(&mut self) {
        let mut items = self.session.all_anchored();
        items.sort_by(|a, b| {
            b.comment
                .is_pending()
                .cmp(&a.comment.is_pending())
                .then_with(|| a.comment.path.cmp(&b.comment.path))
                .then_with(|| a.line.cmp(&b.line))
        });
        self.screens.push(Screen::Review(ReviewState {
            items: ListState::new(items),
            show_completed: false,
        }));
    }

    pub(super) fn refresh_review(&mut self) {
        let all = self.session.all_anchored();
        if let Screen::Review(r) = self.screen_mut() {
            let cursor = r.items.cursor;
            let mut items = all;
            items.sort_by(|a, b| {
                b.comment
                    .is_pending()
                    .cmp(&a.comment.is_pending())
                    .then_with(|| a.comment.path.cmp(&b.comment.path))
                    .then_with(|| a.line.cmp(&b.line))
            });
            r.items = ListState::new(items);
            // The cursor indexes the rows on screen, which is fewer than the items when
            // completed comments are hidden.
            let visible = Self::review_visible(r).len();
            r.items.cursor = cursor.min(visible.saturating_sub(1));
        }
    }

    pub(super) fn push_notes(&mut self) {
        let items: Vec<Note> = self.session.notes.notes().into_iter().cloned().collect();
        self.screens.push(Screen::Notes(NotesState {
            items: ListState::new(items),
        }));
    }

    pub(super) fn refresh_notes(&mut self) {
        let items: Vec<Note> = self.session.notes.notes().into_iter().cloned().collect();
        if let Screen::Notes(n) = self.screen_mut() {
            let cursor = n.items.cursor;
            n.items = ListState::new(items);
            n.items.cursor = cursor.min(n.items.items.len().saturating_sub(1));
        }
    }

    /// Visible review items, honouring the completed filter.
    pub fn review_visible(r: &ReviewState) -> Vec<&Anchored> {
        r.items
            .items
            .iter()
            .filter(|a| r.show_completed || a.comment.is_pending())
            .collect()
    }

    pub(super) fn push_locations(&mut self, title: String, items: Vec<Hit>) {
        // Start on the hit nearest the cursor when the list is for the current file.
        let mut list = ListState::new(items);
        if let Some(v) = &self.viewer {
            let line = v.current_line().unwrap_or(0) + 1;
            if let Some(i) = list
                .items
                .iter()
                .enumerate()
                .filter(|(_, h)| h.path == v.path)
                .min_by_key(|(_, h)| h.line.abs_diff(line))
                .map(|(i, _)| i)
            {
                list.cursor = i;
            }
        }
        self.screens
            .push(Screen::Locations(LocationsState { title, items: list }));
    }

    pub(super) fn handle_locations_key(&mut self, key: KeyEvent) {
        let height = self.measured.list_height.max(1) as isize;
        let Screen::Locations(state) = self.screens.last_mut().expect("screen") else {
            return;
        };
        if state.items.handle_nav(key, height) {
            return;
        }
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => self.pop_screen(),
            KeyCode::Enter | KeyCode::Char('o') | KeyCode::Char('l') => {
                if let Some(hit) = state.items.current().cloned() {
                    self.push_jump();
                    self.jump_to_col(&hit.path, hit.line, hit.column);
                }
            }
            _ => {}
        }
    }

    pub(super) fn handle_log_key(&mut self, key: KeyEvent) {
        let height = self.measured.list_height.max(1) as isize;
        let mut load_more = false;
        let Screen::Log(state) = self.screens.last_mut().expect("screen") else {
            return;
        };
        // One of the two lists has the keys; which one is what Tab switches.
        let moved = if state.focus_files {
            state.files.handle_nav(key, height)
        } else {
            let was_at_end = state.commits.cursor + 1 >= state.commits.items.len();
            let moved = state.commits.handle_nav(key, height);
            // Only a key that goes down can reach past the end and want another page.
            load_more = moved
                && was_at_end
                && matches!(
                    key.code,
                    KeyCode::Char('j')
                        | KeyCode::Down
                        | KeyCode::Char('G')
                        | KeyCode::End
                        | KeyCode::PageDown
                );
            moved
        };
        if !moved {
            match key.code {
                KeyCode::Char('q') | KeyCode::Esc => {
                    if state.focus_files {
                        state.focus_files = false;
                    } else {
                        self.pop_screen();
                    }
                    return;
                }
                KeyCode::Tab => {
                    if !state.files.items.is_empty() {
                        state.focus_files = !state.focus_files;
                    }
                    return;
                }
                KeyCode::Enter | KeyCode::Char('l') | KeyCode::Right => {
                    if state.focus_files || state.path.is_some() {
                        let Some(commit) = state.commits.current() else {
                            return;
                        };
                        let hash = commit.hash.clone();
                        let siblings = state.files.items.clone();
                        let index = if state.path.is_some() && !state.focus_files {
                            0
                        } else {
                            state.files.cursor
                        };
                        self.push_diff(DiffTarget::Commit { hash }, siblings, index);
                    } else if !state.files.items.is_empty() {
                        state.focus_files = true;
                    }
                    return;
                }
                KeyCode::Char('w') => {
                    // The file at this commit against the working tree.
                    let Some(commit) = state.commits.current() else {
                        return;
                    };
                    let hash = commit.hash.clone();
                    let file = if state.focus_files {
                        state.files.current().cloned()
                    } else {
                        state.path.as_ref().map(|p| ChangedFile {
                            status: FileStatus::Modified,
                            path: p.clone(),
                            old_path: None,
                        })
                    };
                    let Some(file) = file else {
                        self.info("pick a file first");
                        return;
                    };
                    self.push_diff(
                        DiffTarget::Revisions {
                            from: hash,
                            to: String::new(),
                        },
                        vec![file],
                        0,
                    );
                    return;
                }
                KeyCode::Char('H') => {
                    if state.focus_files {
                        if let Some(f) = state.files.current() {
                            let path = f.path.clone();
                            self.push_log(Some(path));
                        }
                    }
                    return;
                }
                _ => return,
            }
        }
        // What is under the cursor decides which files are shown, and reaching the end of
        // the list asks git for the next page. Both want the session, which is a different
        // field from the screen stack, so neither has to take the screen apart to get it.
        let mut failed = None;
        if let Some(Screen::Log(state)) = self.screens.last_mut() {
            if load_more {
                if let Err(e) = Self::load_more_commits(&mut self.session, state) {
                    failed = Some(e);
                }
            }
            if let Err(e) = Self::load_commit_files(&mut self.session, state) {
                failed = failed.or(Some(e));
            }
        }
        if let Some(e) = failed {
            self.error(format!("{e:#}"));
        }
    }

    pub(super) fn handle_changes_key(&mut self, key: KeyEvent) {
        let height = self.measured.list_height.max(1) as isize;
        let Screen::Changes(state) = self.screens.last_mut().expect("screen") else {
            return;
        };
        if state.files.handle_nav(key, height) {
            return;
        }
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => self.pop_screen(),
            KeyCode::Char('s') => {
                let target = if state.target == DiffTarget::Staged {
                    DiffTarget::Working
                } else {
                    DiffTarget::Staged
                };
                self.screens.pop();
                self.push_changes(target);
            }
            KeyCode::Enter | KeyCode::Char('l') | KeyCode::Right => {
                let target = state.target.clone();
                let siblings = state.files.items.clone();
                let index = state.files.cursor;
                self.push_diff(target, siblings, index);
            }
            KeyCode::Char('H') => {
                if let Some(f) = state.files.current() {
                    let path = f.path.clone();
                    self.push_log(Some(path));
                }
            }
            KeyCode::Char('o') => {
                if let Some(f) = state.files.current() {
                    let path = f.path.clone();
                    self.jump_to(&path, 1);
                }
            }
            _ => {}
        }
    }

    pub(super) fn handle_diff_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let height = self.measured.main_height.max(1);
        let half = (height / 2).max(1) as isize;
        let Screen::Diff(d) = self.screens.last_mut().expect("screen") else {
            return;
        };
        let v = &mut d.view;
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => {
                if v.search.is_some() && key.code == KeyCode::Esc {
                    v.search = None;
                } else {
                    self.pop_screen();
                }
            }
            KeyCode::Char('j') | KeyCode::Down => v.move_by(1),
            KeyCode::Char('k') | KeyCode::Up => v.move_by(-1),
            KeyCode::Char('d') if ctrl => v.move_by(half),
            KeyCode::Char('u') if ctrl => v.move_by(-half),
            KeyCode::Char('e') if ctrl => v.scroll_by(1, height),
            KeyCode::Char('y') if ctrl => v.scroll_by(-1, height),
            KeyCode::PageDown => v.move_by(height as isize),
            KeyCode::PageUp => v.move_by(-(height as isize)),
            KeyCode::Char('g') | KeyCode::Home => v.go_top(),
            KeyCode::Char('G') | KeyCode::End => v.go_bottom(),
            KeyCode::Char('h') | KeyCode::Left => v.hscroll = v.hscroll.saturating_sub(8),
            KeyCode::Char('l') | KeyCode::Right => v.hscroll += 8,
            KeyCode::Char('0') => v.hscroll = 0,
            KeyCode::Char('n') | KeyCode::Char('}') => v.next_change(true, false),
            KeyCode::Char('p') | KeyCode::Char('{') => v.next_change(false, false),
            KeyCode::Char('v') => {
                self.layout = self.layout.next();
                let outcome = self.session.set_layout(self.layout.name());
                self.report(outcome);
            }
            KeyCode::Char(']') => {
                let i = d.index + 1;
                if i < d.siblings.len() {
                    self.replace_diff(i);
                } else {
                    self.info("last file");
                }
            }
            KeyCode::Char('[') => {
                if d.index > 0 {
                    let i = d.index - 1;
                    self.replace_diff(i);
                } else {
                    self.info("first file");
                }
            }
            KeyCode::Char('c') => self.start_comment(),
            KeyCode::Char('a') => self.ask_about_context(),
            KeyCode::Char('x') => self.toggle_comment(),
            KeyCode::Char('e') => self.edit_comment(),
            KeyCode::Char('D') => self.delete_comment(),
            KeyCode::Char('u') => self.undo_delete(),
            KeyCode::Char('/') => {
                self.prompt = Some(Prompt::new(PromptKind::Search, "search (after side)", ""));
            }
            KeyCode::Char('>') => v.next_match(true),
            KeyCode::Char('<') => v.next_match(false),
            KeyCode::Char(':') => {
                self.prompt = Some(Prompt::new(
                    PromptKind::GoToLine,
                    "go to line (after side)",
                    "",
                ));
            }
            KeyCode::Char('H') => {
                let path = v.file.path.clone();
                self.push_log(Some(path));
            }
            KeyCode::Char('o') => {
                let line = v.after_line().unwrap_or(0) + 1;
                let path = v.file.path.clone();
                self.jump_to(&path, line);
            }
            _ => {}
        }
    }

    pub(super) fn handle_review_key(&mut self, key: KeyEvent) {
        let height = self.measured.list_height.max(1) as isize;
        let Screen::Review(state) = self.screens.last_mut().expect("screen") else {
            return;
        };
        let visible = Self::review_visible(state).len();
        if state.items.handle_nav_within(key, height, visible) {
            return;
        }
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => self.pop_screen(),
            KeyCode::Char('s') => {
                state.show_completed = !state.show_completed;
                state.items.cursor = 0;
            }
            KeyCode::Char('x') => self.toggle_comment(),
            KeyCode::Char('d') => self.delete_comment(),
            KeyCode::Char('e') => self.edit_comment(),
            KeyCode::Char('a') => self.ask_about_context(),
            KeyCode::Char('A') => {
                let text = prompts::address_all().to_string();
                self.prompt = Some(Prompt::new(
                    PromptKind::Agent {
                        context: Vec::new(),
                    },
                    "ask the agent",
                    &text,
                ));
            }
            KeyCode::Enter | KeyCode::Char('o') => {
                if let Some(a) = Self::review_visible(state).get(state.items.cursor) {
                    let (path, line) = (a.comment.path.clone(), a.line);
                    match line {
                        Some(line) => self.jump_to(&path, line),
                        // A comment on a whole path: show the file, or the directory in the tree.
                        None => self.jump_to_path(&path),
                    }
                }
            }
            KeyCode::Char('Z') => {
                let moved = self.session.reanchor();
                if let Some(n) = self.report(moved) {
                    self.refresh_review();
                    self.info(format!("{n} comment(s) re-anchored in REVIEW.md"));
                }
            }
            _ => {}
        }
    }

    pub(super) fn handle_notes_key(&mut self, key: KeyEvent) {
        let height = self.measured.list_height.max(1) as isize;
        let Screen::Notes(state) = self.screens.last_mut().expect("screen") else {
            return;
        };
        if state.items.handle_nav(key, height) {
            return;
        }
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => self.pop_screen(),
            KeyCode::Char('d') => self.delete_note(),
            KeyCode::Char('e') => self.edit_note(),
            KeyCode::Char('n') => {
                let path = state
                    .items
                    .current()
                    .filter(|n| !n.is_general())
                    .map(|n| n.target.clone());
                let label = match &path {
                    Some(p) => format!("note on {p}"),
                    None => "general note".to_string(),
                };
                self.prompt = Some(Prompt::new(PromptKind::Note { path }, label, ""));
            }
            KeyCode::Enter | KeyCode::Char('o') => {
                if let Some(n) = state.items.current().filter(|n| !n.is_general()) {
                    let path = n.target.clone();
                    self.jump_to(&path, 1);
                }
            }
            _ => {}
        }
    }
}
