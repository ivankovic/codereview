//! The end-to-end tests, the screen a repository opens on: every test that drives the
//! software the way a person does, with the recording its last run left behind, played here.
//!
//! A terminal recording is played into frames by `recording::frames` once, when its test is
//! first selected, and drawn from the frame at the playback position. Nothing plays unasked:
//! a selected test shows where its recording starts, paused, and Space runs it. A video
//! cannot be drawn in a terminal, so it is handed to the system's player.
//!
//! A child module of `app`, like the other screens.

use super::*;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use crate::e2e::{EndToEnd, Opening};
use crate::recording::{Cast, Kind, Recording};
use crate::stretch::{Outcome, Report, Touched};
use crate::tui::viewer::RanBy;

/// One row of the list: what the stretch did, then a file's heading, a test, or a recording
/// whose test is gone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum E2eRow {
    /// The heading over what the stretch of history did.
    Stretch,
    /// Something said that leads nowhere; `true` when it is a warning.
    Note(String, bool),
    /// An index into the report's changes to tests: one that now asks less than it did.
    Change(usize),
    /// The heading over the changed code no end-to-end test ran.
    UnranHead,
    /// An index into the report's files of changed code no end-to-end test ran.
    Unran(usize),
    /// How many more such files there are, folded away.
    MoreUnran(usize),
    /// The stretch's base commit, and whether the tests are recorded there.
    Base,
    /// A test file, and how many end-to-end tests it holds.
    File(String, usize),
    /// An index into the opening's tests.
    Test(usize),
    /// The heading over the recordings of tests no longer here.
    Orphans(usize),
    /// An index into the opening's orphans.
    Orphan(usize),
}

impl E2eRow {
    fn selectable(&self) -> bool {
        matches!(
            self,
            E2eRow::Test(_)
                | E2eRow::Orphan(_)
                | E2eRow::Change(_)
                | E2eRow::Unran(_)
                | E2eRow::MoreUnran(_)
                | E2eRow::Base
        )
    }
}

pub struct EndToEndState {
    /// `None` until the tests have been read: the symbol index is still being built and there
    /// is no `OVERVIEW.md` to stand in for it.
    pub opening: Option<Opening>,
    /// The tests came from `OVERVIEW.md` while the index is built, and are read again once
    /// it is.
    pub from_overview: bool,
    pub rows: ListState<E2eRow>,
    /// What the stretch did, through the end-to-end tests; `None` while the symbols are
    /// read, and why not when it cannot be said.
    pub report: Option<std::result::Result<Report, String>>,
    /// Terminal recordings played into frames, by their path, or why one could not be.
    pub casts: HashMap<String, Played>,
    /// The selected recording, and where playback is in it.
    pub player: Player,
    /// The recording takes the whole screen.
    pub full: bool,
    /// Every file of changed code no end-to-end test ran is listed, not only the first few.
    pub all_unran: bool,
}

/// A terminal recording played into frames, or why it could not be; shared between the list
/// and a test's own screen, since playing one is the slow part.
pub type Played = Arc<std::result::Result<Cast, String>>;

/// One recording and where playback is in it: what the list shows for the selected test, and
/// a test's own screen for its test.
#[derive(Default)]
pub struct Player {
    pub recording: Option<Recording>,
    /// The frames of a terminal recording; `None` for a video or no recording.
    pub cast: Option<Played>,
    /// Where playback is, in milliseconds of the recording.
    pub position_ms: u64,
    /// While playing: when it started, and the position it started from.
    pub playing: Option<(Instant, u64)>,
}

impl Player {
    /// `recording`, paused on its first frame with anything on it.
    pub fn new(recording: Option<Recording>, cast: Option<Played>) -> Self {
        let position_ms = match cast.as_deref() {
            Some(Ok(cast)) => cast.first_shown(),
            _ => 0,
        };
        Self {
            recording,
            cast,
            position_ms,
            playing: None,
        }
    }

    /// The terminal recording, played, or why it could not be.
    pub fn cast(&self) -> Option<&std::result::Result<Cast, String>> {
        self.cast.as_deref()
    }

    /// How long the recording runs: the test's own duration, its last frame or its last
    /// step, whichever is latest.
    pub fn duration_ms(&self) -> u64 {
        let frames = match self.cast() {
            Some(Ok(cast)) => cast.duration_ms,
            _ => 0,
        };
        let Some(recording) = &self.recording else {
            return frames;
        };
        let steps = recording.steps.last().map_or(0, |s| s.at_ms);
        frames.max(steps).max(recording.duration_ms)
    }

    /// The step playback is at: the last one at or before the position.
    pub fn current_step(&self) -> Option<usize> {
        let steps = &self.recording.as_ref()?.steps;
        steps.iter().rposition(|s| s.at_ms <= self.position_ms)
    }

    /// Moves playback on to `now`, stopping at the end. True when the position moved.
    pub fn advance(&mut self, now: Instant) -> bool {
        let Some((started, from)) = self.playing else {
            return false;
        };
        let end = self.duration_ms();
        let at = from + now.duration_since(started).as_millis() as u64;
        let before = self.position_ms;
        self.position_ms = at.min(end);
        if at >= end {
            self.playing = None;
        }
        self.position_ms != before || self.playing.is_none()
    }

    /// Plays from where it is, or pauses; from the start again once it has ended.
    pub fn toggle_play(&mut self, now: Instant) {
        if self.playing.take().is_some() {
            return;
        }
        if self.position_ms >= self.duration_ms() {
            self.position_ms = 0;
        }
        self.playing = Some((now, self.position_ms));
    }

    /// Goes to `at_ms`, playing on from there if it was playing.
    pub fn seek(&mut self, at_ms: u64, now: Instant) {
        self.position_ms = at_ms.min(self.duration_ms());
        if self.playing.is_some() {
            self.playing = Some((now, self.position_ms));
        }
    }

    /// The next step's moment, or the previous one's: a step at the position is passed over
    /// going forward, and going back a step a moment after the one playback is in returns to
    /// its start before the one before it.
    pub fn step_at(&self, forward: bool) -> Option<u64> {
        let steps = &self.recording.as_ref()?.steps;
        let at = self.position_ms;
        if forward {
            steps.iter().map(|s| s.at_ms).find(|t| *t > at)
        } else {
            let earlier: Vec<u64> = steps.iter().map(|s| s.at_ms).filter(|t| *t < at).collect();
            earlier.last().copied().or(Some(0))
        }
    }
}

impl EndToEndState {
    pub fn new(
        opening: Option<Opening>,
        from_overview: bool,
        report: Option<std::result::Result<Report, String>>,
        all_unran: bool,
    ) -> Self {
        let rows = opening
            .as_ref()
            .map(|o| rows_of(o, report.as_ref(), all_unran))
            .unwrap_or_default();
        let mut state = Self {
            opening,
            from_overview,
            rows: ListState::new(rows),
            report,
            casts: HashMap::new(),
            player: Player::default(),
            full: false,
            all_unran,
        };
        // On the first test, whatever the stretch has to say above it: the tests are what a
        // repository opens on.
        if let Some(first) = state
            .rows
            .items
            .iter()
            .position(|r| matches!(r, E2eRow::Test(_)))
        {
            state.rows.cursor = first;
        }
        state.settle(1);
        state
    }

    /// The report, when there is one to show.
    pub fn report(&self) -> Option<&Report> {
        self.report.as_ref()?.as_ref().ok()
    }

    /// What the stretch's report says of `test`: the changed lines it ran, and how it went
    /// before and after.
    pub fn stretch_of(&self, test: &EndToEnd) -> (Option<&Touched>, Option<&Outcome>) {
        let Some(report) = self.report() else {
            return (None, None);
        };
        let same = |path: &str, name: &str| path == test.test.path && name == test.test.name;
        (
            report.touched.iter().find(|t| same(&t.path, &t.name)),
            report.outcomes.iter().find(|o| same(&o.path, &o.name)),
        )
    }

    /// The test under the cursor, if it is one.
    pub fn test(&self) -> Option<&EndToEnd> {
        match self.rows.current()? {
            E2eRow::Test(i) => self.opening.as_ref()?.tests.get(*i),
            _ => None,
        }
    }

    /// The recording of what is under the cursor: a test's, or an orphan.
    pub fn recording(&self) -> Option<&Recording> {
        let opening = self.opening.as_ref()?;
        match self.rows.current()? {
            E2eRow::Test(i) => opening.tests.get(*i)?.recording.as_ref(),
            E2eRow::Orphan(i) => opening.orphans.get(*i),
            _ => None,
        }
    }

    /// The cursor onto a test or an orphan, moving on in `direction` (or back, when there is
    /// nothing that way) past headings.
    fn settle(&mut self, direction: isize) {
        settle(&mut self.rows, direction, E2eRow::selectable);
    }

    /// Which test or orphan the cursor is on, to find it again in a fresh opening.
    fn key(&self) -> Option<(String, String)> {
        let opening = self.opening.as_ref()?;
        match self.rows.current()? {
            E2eRow::Test(i) => opening
                .tests
                .get(*i)
                .map(|t| (t.test.path.clone(), t.test.name.clone())),
            E2eRow::Orphan(i) => opening
                .orphans
                .get(*i)
                .map(|r| (r.file.clone(), r.test.clone())),
            _ => None,
        }
    }

    fn find(&mut self, key: &(String, String)) {
        let Some(opening) = self.opening.as_ref() else {
            return;
        };
        let at = self.rows.items.iter().position(|row| match row {
            E2eRow::Test(i) => opening
                .tests
                .get(*i)
                .is_some_and(|t| t.test.path == key.0 && t.test.name == key.1),
            E2eRow::Orphan(i) => opening
                .orphans
                .get(*i)
                .is_some_and(|r| r.file == key.0 && r.test == key.1),
            _ => false,
        });
        if let Some(at) = at {
            self.rows.cursor = at;
        }
    }
}

/// Puts the cursor of `rows` on a row `selectable` accepts, moving on in `direction` (or
/// back, when there is nothing that way) past the others.
pub(super) fn settle<T>(
    rows: &mut ListState<T>,
    direction: isize,
    selectable: impl Fn(&T) -> bool,
) {
    let items = &rows.items;
    if items.is_empty() || selectable(&items[rows.cursor.min(items.len() - 1)]) {
        return;
    }
    let from = rows.cursor as isize;
    for dir in [direction.signum(), -direction.signum()] {
        let dir = if dir == 0 { 1 } else { dir };
        let mut i = from;
        while i >= 0 && (i as usize) < items.len() {
            if selectable(&items[i as usize]) {
                rows.cursor = i as usize;
                return;
            }
            i += dir;
        }
    }
}

/// `recording`'s frames, from `casts` when it was played before, played and kept there when
/// not. `None` for anything but a terminal recording.
pub(super) fn played(
    root: &std::path::Path,
    casts: &mut HashMap<String, Played>,
    recording: &Recording,
) -> Option<Played> {
    if recording.kind != Some(Kind::Terminal) {
        return None;
    }
    let media = recording.media()?;
    Some(
        casts
            .entry(media)
            .or_insert_with_key(|media| {
                Arc::new(
                    std::fs::read_to_string(root.join(media))
                        .map_err(anyhow::Error::from)
                        .and_then(|text| crate::recording::frames(&text))
                        .map_err(|e| format!("{e:#}")),
                )
            })
            .clone(),
    )
}

/// The list: what the stretch did, each file's tests under its name, then the recordings of
/// tests no longer here.
fn rows_of(
    opening: &Opening,
    report: Option<&std::result::Result<Report, String>>,
    all_unran: bool,
) -> Vec<E2eRow> {
    let mut rows = Vec::new();
    if !opening.missing() {
        match report {
            Some(Ok(report)) => stretch_rows(report, all_unran, &mut rows),
            Some(Err(e)) => {
                rows.push(E2eRow::Stretch);
                rows.push(E2eRow::Note(e.clone(), true));
            }
            None => {}
        }
    }
    let counts = crate::e2e::by_file(&opening.tests);
    let mut last: Option<&str> = None;
    for (i, t) in opening.tests.iter().enumerate() {
        if last != Some(t.test.path.as_str()) {
            last = Some(t.test.path.as_str());
            rows.push(E2eRow::File(
                t.test.path.clone(),
                counts.get(t.test.path.as_str()).copied().unwrap_or(0),
            ));
        }
        rows.push(E2eRow::Test(i));
    }
    if !opening.orphans.is_empty() {
        rows.push(E2eRow::Orphans(opening.orphans.len()));
        rows.extend((0..opening.orphans.len()).map(E2eRow::Orphan));
    }
    rows
}

/// What the stretch did: the tests that ask less than they did, the changed code no
/// end-to-end test ran, and the base the tests can be recorded at to compare with.
fn stretch_rows(report: &Report, all_unran: bool, rows: &mut Vec<E2eRow>) {
    rows.push(E2eRow::Stretch);
    let alarming: Vec<usize> = (0..report.changes.len())
        .filter(|i| report.changes[*i].what.alarming())
        .collect();
    if alarming.is_empty() {
        rows.push(E2eRow::Note(
            "No test was removed, skipped or made to check less.".into(),
            false,
        ));
    }
    rows.extend(alarming.into_iter().map(E2eRow::Change));
    if report.changed == 0 {
        rows.push(E2eRow::Note("It changed no code.".into(), false));
    } else {
        rows.push(E2eRow::UnranHead);
        if report.unran.is_empty() {
            rows.push(E2eRow::Note(
                "Every changed line of code ran in an end-to-end test.".into(),
                false,
            ));
        }
        // A week of changes can be a hundred files; the tests are what the screen is for.
        let shown = if all_unran {
            report.unran.len()
        } else {
            report.unran.len().min(UNRAN_FOLDED)
        };
        rows.extend((0..shown).map(E2eRow::Unran));
        if shown < report.unran.len() {
            rows.push(E2eRow::MoreUnran(report.unran.len() - shown));
        }
        if report.without_coverage > 0 {
            rows.push(E2eRow::Note(
                format!(
                    "{} end-to-end {} left no coverage; what {} ran is not counted.",
                    report.without_coverage,
                    if report.without_coverage == 1 {
                        "test"
                    } else {
                        "tests"
                    },
                    if report.without_coverage == 1 {
                        "it"
                    } else {
                        "they"
                    },
                ),
                true,
            ));
        }
    }
    if report.base.is_some() {
        rows.push(E2eRow::Base);
    }
}

/// How many files of changed code no end-to-end test ran are listed before the rest fold.
const UNRAN_FOLDED: usize = 8;

impl App {
    /// What the stretch did, as it can be said now: not while another thread builds the
    /// symbol index, which a second build here would only race.
    fn read_report(&mut self) -> Option<std::result::Result<Report, String>> {
        if !self.session.has_index() && self.warming.is_some() {
            return None;
        }
        let stretch = self.stretch.clone();
        Some(crate::stretch::report(&mut self.session, &stretch).map_err(|e| format!("{e:#}")))
    }

    /// The end-to-end tests, on top of the stack; the screen already there is replaced, on
    /// the same test.
    pub fn push_end_to_end(&mut self) {
        let (opening, from_overview) = self.read_opening();
        let report = self.read_report();
        let all_unran = match self.screens.last() {
            Some(Screen::EndToEnd(old)) => old.all_unran,
            _ => false,
        };
        let mut state = EndToEndState::new(opening, from_overview, report, all_unran);
        if let Some(Screen::EndToEnd(old)) = self.screens.last() {
            if let Some(key) = old.key() {
                state.find(&key);
            }
            state.full = old.full;
            self.screens.pop();
        }
        self.screens.push(Screen::EndToEnd(Box::new(state)));
        self.load_cast();
    }

    /// The opening as it can be had now: from the index once it is built, from
    /// `OVERVIEW.md` while it is, and not at all when there is neither, rather than building
    /// a second index while the first is on its way.
    fn read_opening(&mut self) -> (Option<Opening>, bool) {
        if self.session.has_index() || self.warming.is_none() {
            return (Some(self.session.end_to_end()), false);
        }
        let Some(facts) = &self.session.facts_file else {
            return (None, false);
        };
        let tests = crate::testsuite::Inventory::from_facts(facts, 0)
            .files
            .into_iter()
            .flat_map(|f| f.tests)
            .collect();
        (Some(crate::e2e::of(&self.session, tests)), true)
    }

    /// Reads the tests again for every end-to-end screen on the stack, keeping each on its
    /// test: after a refresh, and when the index the first reading did without arrives.
    pub(super) fn refresh_end_to_end(&mut self, only_provisional: bool) {
        let stale = self.screens.iter().any(|s| match s {
            Screen::EndToEnd(state) => {
                !only_provisional
                    || state.opening.is_none()
                    || state.from_overview
                    || state.report.is_none()
            }
            _ => false,
        });
        if !stale {
            self.refresh_end_to_end_tests(only_provisional);
            return;
        }
        let (opening, from_overview) = self.read_opening();
        let report = self.read_report();
        for screen in &mut self.screens {
            let Screen::EndToEnd(state) = screen else {
                continue;
            };
            let key = state.key();
            let mut fresh = EndToEndState::new(
                opening.clone(),
                from_overview,
                report.clone(),
                state.all_unran,
            );
            if let Some(key) = key {
                fresh.find(&key);
            }
            fresh.full = state.full;
            fresh.casts = std::mem::take(&mut state.casts);
            **state = fresh;
        }
        self.load_cast();
        self.refresh_end_to_end_tests(only_provisional);
    }

    /// Plays the selected terminal recording into frames, the first time it is selected,
    /// and puts playback at its start.
    fn load_cast(&mut self) {
        let root = self.session.root().to_path_buf();
        let Some(Screen::EndToEnd(state)) = self.screens.last_mut() else {
            return;
        };
        let recording = state.recording().cloned();
        let cast = recording
            .as_ref()
            .and_then(|r| played(&root, &mut state.casts, r));
        state.player = Player::new(recording, cast);
    }

    /// Whether a recording is playing, so the screen has to be drawn again soon.
    pub fn animating(&self) -> bool {
        match self.screen() {
            Screen::EndToEnd(s) => s.player.playing.is_some(),
            Screen::EndToEndTest(s) => s.player.playing.is_some() || s.before.playing.is_some(),
            _ => false,
        }
    }

    /// Moves a playing recording on. True when it moved.
    pub(super) fn advance_playback(&mut self) -> bool {
        match self.screens.last_mut() {
            Some(Screen::EndToEnd(state)) => state.player.advance(Instant::now()),
            Some(Screen::EndToEndTest(state)) => {
                let now = Instant::now();
                state.player.advance(now) | state.before.advance(now)
            }
            _ => false,
        }
    }

    /// Moves the cursor by `delta` rows, past headings, as the mouse wheel does.
    pub(super) fn move_end_to_end(&mut self, delta: isize) {
        let Some(Screen::EndToEnd(state)) = self.screens.last_mut() else {
            return;
        };
        let before = state.rows.cursor;
        state.rows.move_by(delta);
        state.settle(delta);
        if state.rows.cursor != before {
            self.load_cast();
        }
    }

    pub(super) fn handle_end_to_end_key(&mut self, key: KeyEvent) {
        let height = self.measured.list_height.max(1) as isize;
        let now = Instant::now();
        let Some(Screen::EndToEnd(state)) = self.screens.last_mut() else {
            return;
        };
        let before = state.rows.cursor;
        if state.rows.handle_nav(key, height) {
            let direction = state.rows.cursor as isize - before as isize;
            state.settle(if direction < 0 { -1 } else { 1 });
            if state.rows.cursor != before {
                self.load_cast();
            }
            return;
        }
        let missing = state.opening.as_ref().is_some_and(Opening::missing);
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc if state.full => state.full = false,
            KeyCode::Char('q') | KeyCode::Esc => self.pop_screen(),
            KeyCode::Char(' ') | KeyCode::Char('p') => {
                if let Some(said) = play(&mut state.player, now) {
                    self.say(said);
                }
            }
            KeyCode::Char(']') | KeyCode::Char('[') => {
                if let Some(at) = state.player.step_at(key.code == KeyCode::Char(']')) {
                    state.player.seek(at, now);
                }
            }
            KeyCode::Char('0') => state.player.seek(0, now),
            KeyCode::Char('z') => state.full = !state.full,
            KeyCode::Char('a') if missing => {
                let ask = state
                    .opening
                    .as_ref()
                    .map(|o| o.ask.clone())
                    .unwrap_or_default();
                self.ask_agent(ask, Vec::new());
            }
            KeyCode::Char('a') => {
                self.info("the agent is offered the first end-to-end tests when there are none; i asks it anything");
            }
            KeyCode::Char('o') => {
                let recording = state.recording().cloned();
                self.open_recording(recording.as_ref());
            }
            KeyCode::Char('w') => self.next_stretch(),
            KeyCode::Char('b') => self.record_base(),
            KeyCode::Enter | KeyCode::Char('l') | KeyCode::Right
                if matches!(
                    state.rows.current(),
                    Some(
                        E2eRow::Change(_) | E2eRow::Unran(_) | E2eRow::MoreUnran(_) | E2eRow::Base
                    )
                ) =>
            {
                if matches!(state.rows.current(), Some(E2eRow::MoreUnran(_))) {
                    let cursor = state.rows.cursor;
                    let opening = state.opening.clone();
                    let report = state.report.clone();
                    state.all_unran = true;
                    state.rows = ListState::new(
                        opening
                            .as_ref()
                            .map(|o| rows_of(o, report.as_ref(), true))
                            .unwrap_or_default(),
                    );
                    state.rows.cursor = cursor;
                    state.settle(1);
                    return;
                }
                let row = state.rows.current().cloned();
                let report = state.report().cloned();
                if let (Some(row), Some(report)) = (row, report) {
                    self.open_stretch_row(&row, &report);
                }
            }
            // A test opens on its own screen; an orphan, whose test is gone, has nothing
            // more to show than the list does.
            KeyCode::Enter | KeyCode::Char('l') | KeyCode::Right if state.test().is_some() => {
                let t = state.test().expect("a test");
                let (path, name) = (t.test.path.clone(), t.test.name.clone());
                self.push_end_to_end_test(&path, &name);
            }
            KeyCode::Enter | KeyCode::Char('l') | KeyCode::Right | KeyCode::Char('s') => {
                let target = match (state.test(), state.recording()) {
                    (Some(t), _) => Some((t.test.path.clone(), t.test.line)),
                    (None, Some(r)) => Some((r.file.clone(), r.line)),
                    _ => None,
                };
                if let Some((path, line)) = target {
                    self.go_to_source(&path, line);
                }
            }
            _ => {}
        }
    }

    /// The next stretch to mark, as the spec screen cycles them: the last 7 days, the last
    /// commit, uncommitted work.
    pub(super) fn next_stretch(&mut self) {
        let at = Stretch::CHOICES
            .iter()
            .position(|c| *c == self.stretch)
            .unwrap_or(0);
        self.stretch = Stretch::CHOICES[(at + 1) % Stretch::CHOICES.len()].clone();
        match self.screen() {
            Screen::EndToEnd(_) => self.push_end_to_end(),
            // The test's screen, and the list under it, which marks the same stretch.
            Screen::EndToEndTest(_) => self.refresh_end_to_end(false),
            _ => {}
        }
        self.info(format!("marking {}", self.stretch.name()));
    }

    /// What a row of the stretch leads to: a test's change to the diff of its file across
    /// the stretch, a file of changed code to that code with what nothing ran marked, the
    /// base to recording the tests there.
    fn open_stretch_row(&mut self, row: &E2eRow, report: &Report) {
        match row {
            E2eRow::Change(i) => {
                let Some(c) = report.changes.get(*i) else {
                    return;
                };
                if c.what == What::Removed {
                    self.info(format!(
                        "{} is gone; the diff of {} shows it go",
                        c.label, c.path
                    ));
                }
                let target = DiffTarget::Revisions {
                    from: report.span.base.clone(),
                    to: report.span.to.clone().unwrap_or_default(),
                };
                let file = ChangedFile {
                    status: FileStatus::Modified,
                    path: c.path.clone(),
                    old_path: None,
                };
                self.push_diff(target, vec![file], 0);
            }
            E2eRow::Unran(i) => {
                let Some(u) = report.unran.get(*i) else {
                    return;
                };
                let first = u.ranges.first().map_or(1, |r| r.0 as usize);
                self.go_to_source(&u.path, first);
                let marked = RanBy {
                    test: report.span.label.clone(),
                    ranges: u.ranges.clone(),
                    unran: true,
                };
                if let Some(v) = self.viewer.as_mut().filter(|v| v.path == u.path) {
                    v.ran = Some(marked);
                }
            }
            E2eRow::Base => self.record_base(),
            _ => {}
        }
    }

    /// Records the end-to-end tests at the stretch's base commit: a worktree of it, and the
    /// repository's own end-to-end command run there, in the background. Asked for, never
    /// started unasked: it runs the code as it was, and can take minutes.
    pub(super) fn record_base(&mut self) {
        let base = match self.screens.iter().rev().find_map(|s| match s {
            Screen::EndToEnd(state) => Some(state.report().and_then(|r| r.base.clone())),
            _ => None,
        }) {
            Some(Some(base)) => base,
            _ => {
                self.info("this stretch starts before the first commit: there is nothing to record it against");
                return;
            }
        };
        if let Some((commit, run)) = &self.base_run
            && run.lock().unwrap_or_else(|e| e.into_inner()).running
        {
            let short: String = commit.chars().take(7).collect();
            self.info(format!("the tests are already being recorded at {short}"));
            return;
        }
        if base.command.is_none() {
            self.info(
                "no command records the end-to-end tests here: a Makefile e2e target, or a \
                 configured check named for them",
            );
            return;
        }
        let started =
            crate::stretch::prepare(&self.session, &base.commit).and_then(|(worktree, check)| {
                crate::analysis::start(&check, &worktree).map(|run| (check.run, run))
            });
        match started {
            Ok((command, run)) => {
                self.info(format!(
                    "recording the tests at {}: {command}, in a worktree of it",
                    base.short
                ));
                self.base_run = Some((base.commit, run));
            }
            Err(e) => self.error(format!("cannot record the tests at {}: {e:#}", base.short)),
        }
    }

    /// Says how a recording of the base went once it is over, and reads the stretch again
    /// with it. True when it ended.
    pub(super) fn watch_base_run(&mut self) -> bool {
        let Some((commit, run)) = &self.base_run else {
            return false;
        };
        let run = run.lock().unwrap_or_else(|e| e.into_inner()).clone();
        if run.running {
            return false;
        }
        let short: String = commit.chars().take(7).collect();
        self.base_run = None;
        self.refresh_end_to_end(false);
        if run.exit == Some(0) {
            self.info(format!("the tests are recorded at {short}"));
        } else {
            let last = run.output.lines().last().unwrap_or("").to_string();
            self.error(format!(
                "recording the tests at {short} failed ({}): {last}",
                run.exit
                    .map_or("killed".to_string(), |c| format!("exit {c}"))
            ));
        }
        true
    }

    /// What the recording of the base is doing, for the row that offers it.
    pub fn base_run_status(&self) -> Option<String> {
        let (commit, run) = self.base_run.as_ref()?;
        let run = run.lock().unwrap_or_else(|e| e.into_inner());
        let started = chrono::DateTime::parse_from_rfc3339(&run.started).ok()?;
        let secs = (chrono::Utc::now() - started.with_timezone(&chrono::Utc))
            .num_seconds()
            .max(0);
        let short: String = commit.chars().take(7).collect();
        let last = run
            .output
            .lines()
            .rev()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("")
            .trim()
            .to_string();
        Some(format!(
            "recording the tests at {short}: {}:{:02}  {last}",
            secs / 60,
            secs % 60
        ))
    }

    /// The test's source at `line`, when its file is still there.
    pub(super) fn go_to_source(&mut self, path: &str, line: usize) {
        if self.session.files.iter().any(|f| f == path) {
            self.jump_to(path, line.max(1));
        } else {
            self.info(format!("{path} is not in the tree any more"));
        }
    }

    /// Hands a recording to the system's player.
    fn open_recording(&mut self, recording: Option<&Recording>) {
        let Some(recording) = recording else {
            self.info("no recording here");
            return;
        };
        let Some(media) = recording.media() else {
            self.info("this test has no recording");
            return;
        };
        let path = self.session.root().join(&media);
        match open_externally(&path) {
            Ok(()) => self.info(format!("opened {media}")),
            Err(e) => self.error(format!("cannot open {media}: {e:#}")),
        }
    }

    pub(super) fn say(&mut self, said: Said) {
        match said {
            Said::Info(text) => self.info(text),
            Said::Error(text) => self.error(text),
        }
    }
}

/// What a key had to say instead of doing anything.
pub(super) enum Said {
    Info(String),
    Error(String),
}

/// Space on a recording: plays or pauses one that can be played here, or says why not.
pub(super) fn play(player: &mut Player, now: Instant) -> Option<Said> {
    match player.cast() {
        Some(Ok(_)) => {
            player.toggle_play(now);
            None
        }
        Some(Err(e)) => Some(Said::Error(format!("the recording cannot be played: {e}"))),
        None if player
            .recording
            .as_ref()
            .is_some_and(|r| r.kind == Some(Kind::Video)) =>
        {
            Some(Said::Info(
                "a video plays outside the terminal: o opens it".into(),
            ))
        }
        None => Some(Said::Info("this test has no recording to play".into())),
    }
}

/// Hands `path` to the system's own opener, detached: the terminal UI keeps the screen.
pub(super) fn open_externally(path: &std::path::Path) -> Result<()> {
    let program = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    std::process::Command::new(program)
        .arg(path)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|e| anyhow::anyhow!("{program}: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spec::Span;
    use crate::stretch::Unran;

    #[test]
    fn a_long_list_of_changed_code_folds_after_the_first_few() {
        let opening = Opening {
            tests: Vec::new(),
            orphans: Vec::new(),
            total: 0,
            failed: 0,
            unrecorded: 0,
            kind: None,
            ask: String::new(),
        };
        let unran = |i: usize| Unran {
            path: format!("src/{i}.rs"),
            changed: 1,
            lines: 1,
            ranges: vec![(1, 1)],
        };
        let report = Report {
            span: Span {
                stretch: Stretch::Uncommitted,
                label: "uncommitted work".into(),
                from: Some("HEAD".into()),
                to: None,
                base: "HEAD".into(),
            },
            changes: Vec::new(),
            changed: 12,
            unran_lines: 12,
            unran: (0..12).map(unran).collect(),
            touched: Vec::new(),
            without_coverage: 0,
            base: None,
            outcomes: Vec::new(),
        };
        // No tests at all is its own finding, and says nothing of the stretch.
        assert!(rows_of(&opening, Some(&Ok(report.clone())), false).is_empty());
        let mut stretch = Vec::new();
        stretch_rows(&report, false, &mut stretch);
        let files = stretch
            .iter()
            .filter(|r| matches!(r, E2eRow::Unran(_)))
            .count();
        assert_eq!(files, UNRAN_FOLDED);
        assert!(stretch.contains(&E2eRow::MoreUnran(12 - UNRAN_FOLDED)));
        let mut all = Vec::new();
        stretch_rows(&report, true, &mut all);
        assert_eq!(
            all.iter().filter(|r| matches!(r, E2eRow::Unran(_))).count(),
            12
        );
        assert!(!all.iter().any(|r| matches!(r, E2eRow::MoreUnran(_))));
    }
}
