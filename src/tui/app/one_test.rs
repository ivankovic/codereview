//! One end-to-end test on a screen of its own: where a reviewer goes from what the software
//! does into the code and the documentation. Its recording at the top, then its steps, each
//! with the line of the test that took it; the code its run executed, what only it ran first;
//! the documentation it demonstrates; and the smaller tests that reach the same code.
//!
//! A child module of `app`, like the other screens.

use super::end_to_end::{Played, Player, open_externally, play, played, settle};
use super::*;
use std::collections::HashMap;
use std::time::Instant;

use crate::e2e::Detail;
use crate::stretch::Comparison;
use crate::tui::viewer::RanBy;

/// One row of a test's screen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TestRow {
    Heading(String),
    /// A line that says something, and leads nowhere.
    Text(String),
    /// An index into the recording's steps.
    Step(usize),
    /// An index into the steps as they ran before the stretch and after it.
    Pair(usize),
    /// The test's own line that took step `n`, under it.
    Source(usize),
    /// An index into the files the run executed.
    Ran(usize),
    /// An index into the documentation.
    Doc(usize),
    /// An index into the smaller tests.
    Smaller(usize),
}

impl TestRow {
    fn selectable(&self) -> bool {
        matches!(
            self,
            TestRow::Step(_)
                | TestRow::Pair(_)
                | TestRow::Ran(_)
                | TestRow::Doc(_)
                | TestRow::Smaller(_)
        )
    }
}

pub struct EndToEndTestState {
    /// The test, by file and name, to read it again with.
    pub key: (String, String),
    /// `None` while the symbols are still being read, or when the test is not there any more.
    pub detail: Option<Detail>,
    pub rows: ListState<TestRow>,
    pub player: Player,
    /// The test as it ran before the stretch and as it runs after, when the tests are
    /// recorded at the stretch's base.
    pub comparison: Option<Comparison>,
    /// The recording from before the stretch, played beside the one from after.
    pub before: Player,
    /// The recording takes the whole screen.
    pub full: bool,
    /// Whether this repository has a make target that records coverage, to say so.
    pub coverage_target: bool,
}

impl EndToEndTestState {
    fn new(
        key: (String, String),
        detail: Option<Detail>,
        cast: Option<Played>,
        compared: Option<(Comparison, Option<Played>)>,
        coverage_target: bool,
    ) -> Self {
        let recording = detail.as_ref().and_then(|d| d.test.recording.clone());
        let (comparison, before_cast) = match compared {
            Some((c, cast)) if c.before.is_some() => (Some(c), cast),
            _ => (None, None),
        };
        let before = Player::new(
            comparison.as_ref().and_then(|c| c.before.clone()),
            before_cast,
        );
        let mut state = Self {
            rows: ListState::new(rows_of(
                detail.as_ref(),
                comparison.as_ref(),
                coverage_target,
            )),
            key,
            detail,
            player: Player::new(recording, cast),
            comparison,
            before,
            full: false,
            coverage_target,
        };
        settle(&mut state.rows, 1, TestRow::selectable);
        state
    }

    /// The index among the after side's steps of each pair's after step.
    pub fn after_index(&self, pair: usize) -> Option<usize> {
        let steps = &self.comparison.as_ref()?.steps;
        steps.get(pair)?.after.as_ref()?;
        Some(steps[..pair].iter().filter(|p| p.after.is_some()).count())
    }
}

/// The rows: the steps — before and after the stretch when both are recorded — the code it
/// ran, the documentation, the smaller tests.
fn rows_of(
    detail: Option<&Detail>,
    comparison: Option<&Comparison>,
    coverage_target: bool,
) -> Vec<TestRow> {
    let Some(d) = detail else {
        return vec![TestRow::Text("reading the tests…".into())];
    };
    let mut rows = vec![TestRow::Heading("Steps".into())];
    if let Some(c) = comparison {
        let short = c.base.as_ref().map_or("", |b| b.short.as_str());
        rows[0] = TestRow::Heading(format!("Steps, as they ran at {short} → as they run now"));
        let mut after = 0;
        for (i, pair) in c.steps.iter().enumerate() {
            rows.push(TestRow::Pair(i));
            if pair.after.is_some() {
                if d.sources.get(after).is_some_and(Option::is_some) {
                    rows.push(TestRow::Source(after));
                }
                after += 1;
            }
        }
        if c.steps.is_empty() {
            rows.push(TestRow::Text("Neither run recorded any steps.".into()));
        }
    } else {
        match &d.test.recording {
            Some(r) if !r.steps.is_empty() => {
                for i in 0..r.steps.len() {
                    rows.push(TestRow::Step(i));
                    if d.sources.get(i).is_some_and(Option::is_some) {
                        rows.push(TestRow::Source(i));
                    }
                }
            }
            Some(_) => rows.push(TestRow::Text("Its run recorded no steps.".into())),
            None => rows.push(TestRow::Text(
                "No recording: its last run left none behind, or it has not run here.".into(),
            )),
        }
    }

    rows.push(TestRow::Heading("Code it ran".into()));
    match &d.ran {
        Some(ran) if !ran.is_empty() => rows.extend((0..ran.len()).map(TestRow::Ran)),
        Some(_) => rows.push(TestRow::Text(
            "Its coverage names none of the repository's code.".into(),
        )),
        None => {
            rows.push(TestRow::Text(
                "Its run left no coverage: a coverage.lcov beside test.json says which code a \
                 test ran."
                    .into(),
            ));
            if coverage_target {
                rows.push(TestRow::Text("make e2e-coverage records it here.".into()));
            }
        }
    }

    rows.push(TestRow::Heading("Documentation".into()));
    if d.docs.is_empty() {
        rows.push(TestRow::Text(
            "No section of the documentation says what this test shows.".into(),
        ));
    }
    rows.extend((0..d.docs.len()).map(TestRow::Doc));

    rows.push(TestRow::Heading(
        "Smaller tests that reach the same code".into(),
    ));
    if d.smaller.is_empty() {
        rows.push(TestRow::Text(
            if d.ran.is_none() {
                "Known once its run leaves coverage."
            } else {
                "No other test names code it ran."
            }
            .into(),
        ));
    }
    rows.extend((0..d.smaller.len()).map(TestRow::Smaller));
    rows
}

impl App {
    /// The end-to-end test in `path` called `name`, on a screen of its own over the list.
    pub fn push_end_to_end_test(&mut self, path: &str, name: &str) {
        let key = (path.to_string(), name.to_string());
        let detail = self.read_detail(&key);
        let cast = self.cast_for(detail.as_ref());
        let compared = self.read_comparison(&key);
        let state = EndToEndTestState::new(key, detail, cast, compared, self.has_coverage_target());
        if matches!(self.screen(), Screen::EndToEndTest(_)) {
            self.screens.pop();
        }
        self.screens.push(Screen::EndToEndTest(Box::new(state)));
    }

    /// The test's detail as it can be had now: not while another thread builds the symbol
    /// index, which a second build here would only race.
    fn read_detail(&mut self, key: &(String, String)) -> Option<Detail> {
        if !self.session.has_index() && self.warming.is_some() {
            return None;
        }
        crate::e2e::detail(&mut self.session, &key.0, &key.1)
    }

    /// The test across the stretch, and the frames of its recording from before it, when the
    /// tests are recorded at the stretch's base; not while the symbols are read.
    fn read_comparison(&mut self, key: &(String, String)) -> Option<(Comparison, Option<Played>)> {
        if !self.session.has_index() && self.warming.is_some() {
            return None;
        }
        let stretch = self.stretch.clone();
        let c = crate::stretch::compare(&mut self.session, &stretch, &key.0, &key.1).ok()?;
        let before = c.before.as_ref()?;
        let worktree =
            crate::stretch::worktree_path(&self.session, &c.base.as_ref()?.commit).ok()?;
        let cast = played(&worktree, &mut HashMap::new(), before);
        Some((c, cast))
    }

    /// The frames of `detail`'s recording, from the list's cache when it played them already.
    fn cast_for(&mut self, detail: Option<&Detail>) -> Option<Played> {
        let recording = detail?.test.recording.as_ref()?;
        let root = self.session.root().to_path_buf();
        let cache = self.screens.iter_mut().rev().find_map(|s| match s {
            Screen::EndToEnd(state) => Some(&mut state.casts),
            _ => None,
        });
        match cache {
            Some(casts) => played(&root, casts, recording),
            None => played(&root, &mut HashMap::new(), recording),
        }
    }

    fn has_coverage_target(&self) -> bool {
        std::fs::read_to_string(self.session.root().join("Makefile"))
            .is_ok_and(|m| m.lines().any(|l| l.starts_with("e2e-coverage:")))
    }

    /// Reads every test screen on the stack again, keeping each where it was: after a
    /// refresh, and, for the ones still waiting for it, when the index arrives.
    pub(super) fn refresh_end_to_end_tests(&mut self, only_provisional: bool) {
        let keys: Vec<(String, String)> = self
            .every_screen_mut()
            .filter_map(|s| match s {
                Screen::EndToEndTest(state) if !only_provisional || state.detail.is_none() => {
                    Some(state.key.clone())
                }
                _ => None,
            })
            .collect();
        for key in keys {
            let detail = self.read_detail(&key);
            let cast = self.cast_for(detail.as_ref());
            let compared = self.read_comparison(&key);
            let coverage_target = self.has_coverage_target();
            for screen in self.every_screen_mut() {
                let Screen::EndToEndTest(state) = screen else {
                    continue;
                };
                if state.key != key {
                    continue;
                }
                let mut fresh = EndToEndTestState::new(
                    key.clone(),
                    detail.clone(),
                    cast.clone(),
                    compared.clone(),
                    coverage_target,
                );
                fresh.rows.cursor = state
                    .rows
                    .cursor
                    .min(fresh.rows.items.len().saturating_sub(1));
                settle(&mut fresh.rows, 1, TestRow::selectable);
                fresh.full = state.full;
                if fresh.player.recording.as_ref().map(|r| &r.dir)
                    == state.player.recording.as_ref().map(|r| &r.dir)
                {
                    fresh.player.position_ms = state.player.position_ms;
                }
                if fresh.before.recording.as_ref().map(|r| &r.dir)
                    == state.before.recording.as_ref().map(|r| &r.dir)
                {
                    fresh.before.position_ms = state.before.position_ms;
                }
                **state = fresh;
            }
        }
    }

    /// Moves the cursor by `delta` rows, past what leads nowhere, as the mouse wheel does.
    pub(super) fn move_end_to_end_test(&mut self, delta: isize) {
        let Some(Screen::EndToEndTest(state)) = self.screens.last_mut() else {
            return;
        };
        state.rows.move_by(delta);
        settle(&mut state.rows, delta, TestRow::selectable);
    }

    pub(super) fn handle_end_to_end_test_key(&mut self, key: KeyEvent) {
        let height = self.measured.list_height.max(1) as isize;
        let now = Instant::now();
        let Some(Screen::EndToEndTest(state)) = self.screens.last_mut() else {
            return;
        };
        let before = state.rows.cursor;
        if state.rows.handle_nav(key, height) {
            let direction = state.rows.cursor as isize - before as isize;
            settle(
                &mut state.rows,
                if direction < 0 { -1 } else { 1 },
                TestRow::selectable,
            );
            return;
        }
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc if state.full => state.full = false,
            KeyCode::Char('q') | KeyCode::Esc => self.pop_screen(),
            // Both recordings, when there are two: the same test, before and after, side by
            // side and in step.
            KeyCode::Char(' ') | KeyCode::Char('p') => {
                if state.before.cast().is_some_and(|c| c.is_ok())
                    && state.before.playing.is_some() == state.player.playing.is_some()
                {
                    state.before.toggle_play(now);
                }
                if let Some(said) = play(&mut state.player, now) {
                    self.say(said);
                }
            }
            KeyCode::Char(']') | KeyCode::Char('[') => {
                let forward = key.code == KeyCode::Char(']');
                if let Some(at) = state.player.step_at(forward) {
                    state.player.seek(at, now);
                }
                if let Some(at) = state.before.step_at(forward) {
                    state.before.seek(at, now);
                }
            }
            KeyCode::Char('0') => {
                state.player.seek(0, now);
                state.before.seek(0, now);
            }
            KeyCode::Char('z') => state.full = !state.full,
            KeyCode::Char('w') => self.next_stretch(),
            KeyCode::Char('o') => {
                let recording = state.player.recording.clone();
                match recording.as_ref().and_then(|r| r.media()) {
                    Some(media) => {
                        let path = self.session.root().join(&media);
                        match open_externally(&path) {
                            Ok(()) => self.info(format!("opened {media}")),
                            Err(e) => self.error(format!("cannot open {media}: {e:#}")),
                        }
                    }
                    None => self.info("this test has no recording"),
                }
            }
            // The test's own source: at the step under the cursor, or where it starts.
            KeyCode::Char('s') => {
                let Some(d) = &state.detail else {
                    return;
                };
                let pair_step = match state.rows.current() {
                    Some(TestRow::Pair(i)) => state.after_index(*i),
                    _ => None,
                };
                let line = match state.rows.current() {
                    Some(TestRow::Pair(_)) => pair_step
                        .and_then(|j| d.test.recording.as_ref()?.steps.get(j))
                        .map(|s| s.line)
                        .filter(|l| *l > 0)
                        .unwrap_or(d.test.test.line),
                    Some(TestRow::Step(i)) => d
                        .test
                        .recording
                        .as_ref()
                        .and_then(|r| r.steps.get(*i))
                        .map(|s| s.line)
                        .filter(|l| *l > 0)
                        .unwrap_or(d.test.test.line),
                    _ => d.test.test.line,
                };
                let path = d.test.test.path.clone();
                self.go_to_source(&path, line);
            }
            KeyCode::Enter | KeyCode::Char('l') | KeyCode::Right => {
                let Some(d) = &state.detail else {
                    return;
                };
                match state.rows.current().cloned() {
                    Some(TestRow::Step(i)) => {
                        if let Some(step) = d.test.recording.as_ref().and_then(|r| r.steps.get(i)) {
                            let at = step.at_ms;
                            state.player.seek(at, now);
                        }
                    }
                    Some(TestRow::Pair(i)) => {
                        let Some(pair) = state.comparison.as_ref().and_then(|c| c.steps.get(i))
                        else {
                            return;
                        };
                        let (was, is) = (
                            pair.before.as_ref().map(|s| s.at_ms),
                            pair.after.as_ref().map(|s| s.at_ms),
                        );
                        if let Some(at) = is {
                            state.player.seek(at, now);
                        }
                        if let Some(at) = was {
                            state.before.seek(at, now);
                        }
                    }
                    Some(TestRow::Ran(i)) => {
                        let Some(file) = d.ran.as_ref().and_then(|r| r.get(i)).cloned() else {
                            return;
                        };
                        let ran = RanBy {
                            test: d.test.test.label.clone(),
                            ranges: file.ranges.clone(),
                            unran: false,
                        };
                        let first = file.ranges.first().map_or(1, |r| r.0 as usize);
                        self.go_to_source(&file.path, first);
                        if let Some(v) = self.viewer.as_mut().filter(|v| v.path == file.path) {
                            v.ran = Some(ran);
                        }
                    }
                    Some(TestRow::Doc(i)) => {
                        if let Some(doc) = d.docs.get(i) {
                            let (path, line) = (doc.section.path.clone(), doc.section.line);
                            self.go_to_source(&path, line);
                        }
                    }
                    Some(TestRow::Smaller(i)) => {
                        if let Some(t) = d.smaller.get(i) {
                            let (path, line) = (t.test.test.path.clone(), t.test.test.line);
                            self.go_to_source(&path, line);
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
}
