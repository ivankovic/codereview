//! The labelling screen: one sampled test at a time, its file beside the nine questions, each
//! option answered by a key and saved at once. What `codereview kinds label` opens.
//!
//! The file can be read as well as looked at: `c` (or `/`, to search at once) moves the keys to
//! it, with a line cursor, paging, sideways scrolling and search, so that a helper the test
//! calls further down the file can be found and read before the questions are answered.
//!
//! The rules' own answers are hidden until asked for (`r`), so that they do not lead the
//! person whose answers they are to be measured against; shown, each option says whether the
//! two agree and the rules say why.

use std::io::stdout;
use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};

use crate::classify::{Answer, Kind, Question};
use crate::kinds::{Dataset, Status, options};
use crate::theme::Theme;
use crate::tui::style;
use crate::tui::ui::title_block;

/// How wide the questions are.
const PANEL: u16 = 50;

/// How far `h` and `l` scroll the file sideways.
const SIDEWAYS: usize = 8;

/// Which pane the keys go to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Questions,
    Code,
}

pub struct Labeller {
    pub dataset: Dataset,
    /// The sample on screen.
    pub at: usize,
    /// The option under the cursor, an index into [`options`].
    pub cursor: usize,
    /// The first line of the file on screen, 0-based.
    pub scroll: usize,
    /// Which pane the keys go to.
    pub focus: Focus,
    /// The line of the file under its cursor, 0-based, while it has the keys.
    pub line: usize,
    /// How many columns of the file are scrolled off to the left.
    pub sideways: usize,
    /// The search being typed, while it is.
    pub typing: Option<String>,
    /// The last search, which `n` and `N` repeat.
    pub pattern: String,
    /// Whether the rules' answers are shown.
    pub reveal: bool,
    /// The note being written, while it is.
    pub note: Option<String>,
    pub help: bool,
    pub quit: bool,
    pub message: Option<String>,
    theme: Theme,
    lines: Vec<String>,
    highlighted: Vec<Vec<crate::highlight::Segment>>,
    rules: Option<Kind>,
    /// How tall the file's pane was when last drawn.
    height: usize,
}

/// Opens the labelling screen on the dataset in `dir`, at the sample named `start` or at the
/// first one not finished.
pub fn run(dir: &Path, start: Option<&str>) -> Result<()> {
    let dataset = Dataset::open(dir)?;
    anyhow::ensure!(
        !dataset.samples.is_empty(),
        "{} has no samples; draw some with `codereview kinds draw`",
        dir.display()
    );
    let theme = crate::config::Config::load()
        .ok()
        .and_then(|c| Theme::named(&c.theme))
        .unwrap_or_default();
    let mut labeller = Labeller::new(dataset, theme, start)?;
    let mut terminal = ratatui::init();
    let result = (|| -> Result<()> {
        while !labeller.quit {
            terminal.draw(|f| labeller.draw(f))?;
            if !event::poll(Duration::from_millis(500))? {
                continue;
            }
            if let Event::Key(key) = event::read()?
                && matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat)
            {
                labeller.handle_key(key);
            }
        }
        Ok(())
    })();
    ratatui::restore();
    let _ = stdout();
    result
}

impl Labeller {
    pub fn new(dataset: Dataset, theme: Theme, start: Option<&str>) -> Result<Self> {
        let at = match start {
            Some(name) => dataset
                .samples
                .iter()
                .position(|s| s.name == name)
                .with_context(|| format!("no sample named {name}"))?,
            None => dataset
                .samples
                .iter()
                .position(|s| s.status == Status::Pending)
                .unwrap_or(0),
        };
        let mut labeller = Self {
            dataset,
            at,
            cursor: 0,
            scroll: 0,
            focus: Focus::Questions,
            line: 0,
            sideways: 0,
            typing: None,
            pattern: String::new(),
            reveal: false,
            note: None,
            help: false,
            quit: false,
            message: None,
            theme,
            lines: Vec::new(),
            highlighted: Vec::new(),
            rules: None,
            height: 20,
        };
        labeller.load();
        Ok(labeller)
    }

    /// Reads the sample on screen: its file, highlighted, scrolled to the test; the cursor on
    /// its first unanswered option.
    fn load(&mut self) {
        let sample = &self.dataset.samples[self.at];
        let text = match self.dataset.file_text(sample) {
            Ok(text) => text,
            Err(e) => {
                self.message = Some(format!("{e:#}"));
                String::new()
            }
        };
        self.lines = text.lines().map(str::to_string).collect();
        self.highlighted = crate::highlight::highlight(&sample.path, &text, &self.theme.syntax);
        self.rules = self.dataset.replay(sample).ok();
        self.cursor = sample.answers.iter().position(Option::is_none).unwrap_or(0);
        self.centre();
    }

    /// Scrolls the file so that the test starts a few lines down, with the file's cursor on
    /// its first line.
    fn centre(&mut self) {
        let line = self.dataset.samples[self.at].line;
        self.scroll = line.saturating_sub(4);
        self.line = line
            .saturating_sub(1)
            .min(self.lines.len().saturating_sub(1));
        self.sideways = 0;
    }

    /// Puts the file's cursor on `line`, scrolling so that it is in sight.
    fn move_to(&mut self, line: usize) {
        self.line = line.min(self.lines.len().saturating_sub(1));
        if self.line < self.scroll {
            self.scroll = self.line;
        } else if self.line >= self.scroll + self.height {
            self.scroll = self.line + 1 - self.height.max(1);
        }
    }

    /// Whether `line` of the file matches `pattern`: ignoring case unless the pattern has a
    /// capital in it.
    fn matches(line: &str, pattern: &str) -> bool {
        if pattern.chars().any(char::is_uppercase) {
            line.contains(pattern)
        } else {
            line.to_lowercase().contains(&pattern.to_lowercase())
        }
    }

    /// Moves the file's cursor to the next line matching the last search, or the previous one,
    /// wrapping at the ends.
    fn find(&mut self, forward: bool) {
        if self.pattern.is_empty() || self.lines.is_empty() {
            return;
        }
        let n = self.lines.len();
        let found = (1..=n)
            .map(|i| {
                if forward {
                    (self.line + i) % n
                } else {
                    (self.line + n - i % n) % n
                }
            })
            .find(|&i| Self::matches(&self.lines[i], &self.pattern));
        match found {
            Some(i) => {
                let wrapped = if forward {
                    i <= self.line
                } else {
                    i >= self.line
                };
                self.move_to(i);
                let count = self
                    .lines
                    .iter()
                    .filter(|l| Self::matches(l, &self.pattern))
                    .count();
                let at = self.lines[..=i]
                    .iter()
                    .filter(|l| Self::matches(l, &self.pattern))
                    .count();
                self.message = Some(format!(
                    "/{}: {at} of {count}{}",
                    self.pattern,
                    if wrapped { ", from the other end" } else { "" }
                ));
            }
            None => self.message = Some(format!("/{}: not in the file", self.pattern)),
        }
    }

    /// A key while the file has the keys; `false` when it is not one of the file's, so that
    /// the keys that work everywhere still do.
    fn code_key(&mut self, key: KeyEvent) -> bool {
        if let Some(typing) = &mut self.typing {
            match key.code {
                KeyCode::Enter => {
                    self.pattern = std::mem::take(typing);
                    self.typing = None;
                    self.find(true);
                }
                KeyCode::Esc => self.typing = None,
                KeyCode::Backspace => {
                    typing.pop();
                }
                KeyCode::Char(c) => typing.push(c),
                _ => {}
            }
            return true;
        }
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let page = (self.height / 2).max(1);
        match key.code {
            KeyCode::Char('c') if ctrl => return false,
            KeyCode::Esc | KeyCode::Char('c') => self.focus = Focus::Questions,
            KeyCode::Char('j') | KeyCode::Down => self.move_to(self.line + 1),
            KeyCode::Char('k') | KeyCode::Up => self.move_to(self.line.saturating_sub(1)),
            KeyCode::Char('d') if ctrl => self.move_to(self.line + page),
            KeyCode::Char('u') if ctrl => self.move_to(self.line.saturating_sub(page)),
            KeyCode::PageDown => self.move_to(self.line + page),
            KeyCode::PageUp => self.move_to(self.line.saturating_sub(page)),
            KeyCode::Char('g') | KeyCode::Home => self.move_to(0),
            KeyCode::Char('G') | KeyCode::End => self.move_to(usize::MAX),
            KeyCode::Char('l') | KeyCode::Right => self.sideways += SIDEWAYS,
            KeyCode::Char('h') | KeyCode::Left => {
                self.sideways = self.sideways.saturating_sub(SIDEWAYS)
            }
            KeyCode::Char('0') => self.sideways = 0,
            KeyCode::Char('/') => self.typing = Some(String::new()),
            KeyCode::Char('n') => self.find(true),
            KeyCode::Char('N') => self.find(false),
            KeyCode::Char('z') => self.centre(),
            KeyCode::Char('q' | '?' | ']' | '[' | '}' | 'r' | 'e') => return false,
            _ => {}
        }
        true
    }

    fn save(&mut self) {
        if let Err(e) = self.dataset.save() {
            self.message = Some(format!("not saved: {e:#}"));
        }
    }

    /// Answers the option under the cursor, settles whether the sample is finished, saves, and
    /// moves on to the next option.
    fn answer(&mut self, answer: Option<Answer>, advance: bool) {
        let cursor = self.cursor;
        let sample = &mut self.dataset.samples[self.at];
        sample.answers[cursor] = answer;
        if sample.status != Status::Skipped {
            sample.status = if sample.answered() {
                Status::Done
            } else {
                Status::Pending
            };
        }
        let finished = sample.status == Status::Done;
        self.save();
        if finished {
            self.message = Some("every option answered: done (] for the next sample)".into());
        }
        if advance && self.cursor + 1 < options().len() {
            self.cursor += 1;
        }
    }

    /// The options of the question under the cursor, as a range of [`options`].
    fn question_range(&self) -> std::ops::Range<usize> {
        let all = options();
        let q = all[self.cursor].0;
        let from = all.iter().position(|(x, _)| *x == q).expect("an option");
        let to = all.iter().rposition(|(x, _)| *x == q).expect("an option") + 1;
        from..to
    }

    fn go_to(&mut self, at: usize) {
        if at < self.dataset.samples.len() && at != self.at {
            self.at = at;
            self.typing = None;
            self.load();
        }
    }

    pub fn handle_key(&mut self, key: KeyEvent) {
        self.message = None;
        if let Some(note) = &mut self.note {
            match key.code {
                KeyCode::Enter => {
                    let text = std::mem::take(note);
                    self.note = None;
                    self.dataset.samples[self.at].notes = text;
                    self.save();
                    self.message = Some("note saved".into());
                }
                KeyCode::Esc => self.note = None,
                KeyCode::Backspace => {
                    note.pop();
                }
                KeyCode::Char(c) => note.push(c),
                _ => {}
            }
            return;
        }
        if self.help {
            self.help = false;
            return;
        }
        if self.focus == Focus::Code && self.code_key(key) {
            return;
        }
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let page = (self.height / 2).max(1);
        let count = options().len();
        match key.code {
            KeyCode::Char('c') if ctrl => self.quit = true,
            KeyCode::Char('d') if ctrl => self.scroll += page,
            KeyCode::Char('u') if ctrl => self.scroll = self.scroll.saturating_sub(page),
            KeyCode::PageDown => self.scroll += page,
            KeyCode::PageUp => self.scroll = self.scroll.saturating_sub(page),
            KeyCode::Char('q') | KeyCode::Esc => self.quit = true,
            KeyCode::Char('?') => self.help = true,
            KeyCode::Char('j') | KeyCode::Down => self.cursor = (self.cursor + 1).min(count - 1),
            KeyCode::Char('k') | KeyCode::Up => self.cursor = self.cursor.saturating_sub(1),
            KeyCode::Tab => {
                let end = self.question_range().end;
                self.cursor = if end < count { end } else { 0 };
            }
            KeyCode::BackTab => {
                let start = self.question_range().start;
                self.cursor = if start == 0 {
                    0
                } else {
                    let before = start - 1;
                    let q = options()[before].0;
                    options().iter().position(|(x, _)| *x == q).unwrap_or(0)
                };
            }
            KeyCode::Char('y') => self.answer(Some(Answer::Yes), true),
            KeyCode::Char('n') => self.answer(Some(Answer::No), true),
            KeyCode::Char('i') => self.answer(Some(Answer::Inconclusive), true),
            KeyCode::Char('x') | KeyCode::Backspace => self.answer(None, false),
            KeyCode::Char(' ') => {
                let next = match self.dataset.samples[self.at].answers[self.cursor] {
                    None => Some(Answer::Yes),
                    Some(Answer::Yes) => Some(Answer::No),
                    Some(Answer::No) => Some(Answer::Inconclusive),
                    Some(Answer::Inconclusive) => None,
                };
                self.answer(next, false);
            }
            // The rest of this question is no: most options of most questions are.
            KeyCode::Char('N') => {
                let range = self.question_range();
                for i in range.clone() {
                    if self.dataset.samples[self.at].answers[i].is_none() {
                        self.cursor = i;
                        self.answer(Some(Answer::No), false);
                    }
                }
                self.cursor = if range.end < count {
                    range.end
                } else {
                    range.start
                };
            }
            KeyCode::Char('s') => {
                let sample = &mut self.dataset.samples[self.at];
                sample.status = match sample.status {
                    Status::Skipped if sample.answered() => Status::Done,
                    Status::Skipped => Status::Pending,
                    _ => Status::Skipped,
                };
                let word = sample.status.word();
                self.save();
                self.message = Some(format!("now {word}"));
            }
            KeyCode::Char(']') => self.go_to(self.at + 1),
            KeyCode::Char('[') => self.go_to(self.at.saturating_sub(1)),
            KeyCode::Char('}') => {
                let n = self.dataset.samples.len();
                match (1..n)
                    .map(|i| (self.at + i) % n)
                    .find(|&i| self.dataset.samples[i].status == Status::Pending)
                {
                    Some(i) => self.go_to(i),
                    None => self.message = Some("every sample is finished".into()),
                }
            }
            KeyCode::Char('r') => self.reveal = !self.reveal,
            KeyCode::Char('z') => self.centre(),
            KeyCode::Char('c') => self.focus = Focus::Code,
            KeyCode::Char('/') => {
                self.focus = Focus::Code;
                self.typing = Some(String::new());
            }
            KeyCode::Char('e') => self.note = Some(self.dataset.samples[self.at].notes.clone()),
            _ => {}
        }
    }

    pub fn draw(&mut self, frame: &mut Frame) {
        let area = frame.area();
        let [head, body, foot] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Fill(1),
            Constraint::Length(1),
        ])
        .areas(area);
        let [code, panel] =
            Layout::horizontal([Constraint::Fill(1), Constraint::Length(PANEL)]).areas(body);
        self.draw_head(frame, head);
        self.draw_code(frame, code);
        self.draw_questions(frame, panel);
        self.draw_foot(frame, foot);
        if self.help {
            self.draw_help(frame, area);
        }
    }

    fn draw_head(&self, frame: &mut Frame, area: Rect) {
        let samples = &self.dataset.samples;
        let s = &samples[self.at];
        let done = samples.iter().filter(|s| s.status == Status::Done).count();
        let short: String = s.commit.chars().take(7).collect();
        let line: Line = vec![
            " label ".reversed(),
            format!(" {}/{} · {done} done · ", self.at + 1, samples.len()).dim(),
            format!("{} @ {short}", s.repository).bold(),
            format!(" · {}:{}–{} · ", s.path, s.line, s.end).into(),
            Span::styled(
                s.status.word(),
                match s.status {
                    Status::Done => style::fg(self.theme.insert_fg),
                    Status::Skipped => style::dim(&self.theme),
                    Status::Pending => style::accent(&self.theme),
                },
            ),
        ]
        .into();
        frame.render_widget(Paragraph::new(line), area);
    }

    fn draw_code(&mut self, frame: &mut Frame, area: Rect) {
        let s = &self.dataset.samples[self.at];
        let reading = self.focus == Focus::Code;
        // The note goes with the test it is about, where it is always in sight.
        let mut title = format!(" {} ", s.label);
        if !s.notes.is_empty() {
            title.push_str(&format!("· note: {} ", s.notes));
        }
        if reading {
            title.push_str(&format!(
                "· line {} of {} ",
                self.line + 1,
                self.lines.len()
            ));
        }
        let block = title_block(&self.theme, title.into(), reading);
        let inner = block.inner(area);
        frame.render_widget(block, area);
        self.height = inner.height as usize;
        // A file that fits is shown whole; a longer one from where it is scrolled to.
        self.scroll = if self.lines.len() <= self.height {
            0
        } else {
            self.scroll.min(self.lines.len().saturating_sub(1))
        };
        let width = self.lines.len().to_string().len();
        let dim = style::dim(&self.theme);
        let mark = style::accent(&self.theme);
        let cursor = style::cursor(&self.theme);
        let shown = || (self.scroll..self.lines.len()).take(inner.height as usize);
        let found =
            |i: usize| !self.pattern.is_empty() && Self::matches(&self.lines[i], &self.pattern);
        // The numbers and the test's mark stay put while the code scrolls sideways.
        let gutter: Vec<Line> = shown()
            .map(|i| {
                let number = i + 1;
                let inside = (s.line..=s.end).contains(&number);
                let mut number_style = if found(i) { mark.bold() } else { dim };
                if reading && i == self.line {
                    number_style = number_style.patch(cursor);
                }
                Line::from(vec![
                    Span::styled(format!("{number:>width$} "), number_style),
                    Span::styled(if inside { "▌" } else { " " }, mark),
                ])
            })
            .collect();
        let code: Vec<Line> = shown()
            .map(|i| {
                let line: Line = match self.highlighted.get(i) {
                    Some(segments) => segments
                        .iter()
                        .map(|seg| style::segment_span(seg, true, None))
                        .collect::<Vec<_>>()
                        .into(),
                    None => self.lines[i].clone().into(),
                };
                if reading && i == self.line {
                    line.patch_style(cursor)
                } else {
                    line
                }
            })
            .collect();
        let [left, right] =
            Layout::horizontal([Constraint::Length(width as u16 + 2), Constraint::Fill(1)])
                .areas(inner);
        frame.render_widget(Paragraph::new(gutter), left);
        let sideways = self.sideways.min(u16::MAX as usize) as u16;
        frame.render_widget(Paragraph::new(code).scroll((0, sideways)), right);
    }

    fn draw_questions(&self, frame: &mut Frame, area: Rect) {
        let s = &self.dataset.samples[self.at];
        let title = if self.reveal {
            " questions · rules shown (r) "
        } else {
            " questions "
        };
        let block = title_block(&self.theme, title.into(), self.focus == Focus::Questions);
        let inner = block.inner(area);
        frame.render_widget(block, area);
        let dim = style::dim(&self.theme);
        let mut lines: Vec<Line> = Vec::new();
        let mut cursor_row = 0;
        let mut last: Option<Question> = None;
        for (i, (question, option)) in options().into_iter().enumerate() {
            if last != Some(question) {
                last = Some(question);
                let summary = {
                    let said = s.said(question);
                    let answered = options()
                        .iter()
                        .zip(&s.answers)
                        .filter(|((q, _), _)| *q == question)
                        .all(|(_, a)| a.is_some());
                    answered.then(|| crate::classify::summarise(question, &said))
                };
                let mut head = vec![Span::styled(
                    question.title().to_string(),
                    Style::new().bold(),
                )];
                if let Some(summary) = summary {
                    head.push(Span::styled(
                        format!("  {summary}"),
                        style::accent(&self.theme),
                    ));
                }
                lines.push(head.into());
            }
            let mark = match s.answers[i] {
                None => " ",
                Some(Answer::Yes) => "y",
                Some(Answer::No) => "n",
                Some(Answer::Inconclusive) => "?",
            };
            let mut spans = vec![Span::raw(format!("  [{mark}] {option:<14}"))];
            if self.reveal
                && let Some(rules) = &self.rules
                && let Some(said) = rules
                    .of(question)
                    .options
                    .iter()
                    .find(|x| x.option == option)
            {
                let theirs = match said.answer {
                    Answer::Yes => "y",
                    Answer::No => "n",
                    Answer::Inconclusive => "?",
                };
                let (sign, colour) = match s.answers[i] {
                    None => (" ", dim),
                    Some(a) if a == said.answer => ("✓", style::fg(self.theme.insert_fg)),
                    Some(_) => ("✗", style::fg(self.theme.delete_fg).bold()),
                };
                spans.push(Span::styled(format!(" rules {theirs} "), dim));
                spans.push(Span::styled(sign, colour));
            }
            if i == self.cursor {
                cursor_row = lines.len();
                // While the file has the keys the option keeps its place, unlit.
                if self.focus == Focus::Questions {
                    spans = spans
                        .into_iter()
                        .map(|sp| sp.patch_style(style::cursor(&self.theme)))
                        .collect();
                }
            }
            lines.push(spans.into());
        }
        let height = inner.height as usize;
        let offset = cursor_row.saturating_sub(height.saturating_sub(4));
        let shown: Vec<Line> = lines.into_iter().skip(offset).collect();
        frame.render_widget(
            Paragraph::new(shown).wrap(ratatui::widgets::Wrap { trim: false }),
            inner,
        );
    }

    fn draw_foot(&self, frame: &mut Frame, area: Rect) {
        let line: Line = if let Some(note) = &self.note {
            vec![
                " note ".reversed(),
                format!(" {note}▏").into(),
                "  Enter saves, Esc leaves it".dim(),
            ]
            .into()
        } else if let Some(typing) = &self.typing {
            vec![
                " search ".reversed(),
                format!(" /{typing}▏").into(),
                "  Enter finds, Esc leaves it".dim(),
            ]
            .into()
        } else if let Some(message) = &self.message {
            vec![" label ".reversed(), format!(" {message}").into()].into()
        } else if let Some(why) = self.rules_why() {
            // What the rules said of the option under the cursor, once shown.
            vec![" label ".reversed(), format!(" rules: {why}").dim()].into()
        } else if self.focus == Focus::Code {
            vec![
                " file ".reversed(),
                " j k line  Ctrl-d Ctrl-u page  g G ends  h l 0 sideways  / search  n N again  z test  Esc questions"
                    .dim(),
            ]
            .into()
        } else {
            vec![
                " label ".reversed(),
                " y n i answer  Space cycle  x clear  N rest no  Tab question  ] [ } sample  s skip  r rules  e note  ? help  q quit"
                    .dim(),
            ]
            .into()
        };
        frame.render_widget(Paragraph::new(line), area);
    }

    /// Why the rules answer the option under the cursor as they do, while they are shown.
    fn rules_why(&self) -> Option<String> {
        if !self.reveal {
            return None;
        }
        let (question, option) = options()[self.cursor];
        self.rules
            .as_ref()?
            .of(question)
            .options
            .iter()
            .find(|x| x.option == option)
            .map(|x| x.why.clone())
    }

    fn draw_help(&self, frame: &mut Frame, area: Rect) {
        const KEYS: &[(&str, &str)] = &[
            (
                "y n i",
                "the option under the cursor holds, does not, or cannot be told",
            ),
            ("Space", "cycle the answer; x or Backspace clears it"),
            ("N", "every unanswered option of this question: no"),
            ("j k  Tab S-Tab", "the next or previous option; question"),
            ("] [", "the next or previous sample"),
            ("}", "the next sample not finished"),
            ("s", "set the sample aside, or take it back"),
            (
                "r",
                "show or hide the rules' answers, and whether they agree",
            ),
            ("e", "a note on the sample"),
            ("Ctrl-d Ctrl-u  z", "scroll the file; back to the test"),
            ("c  /", "read the file: its keys below; / searches at once"),
            (
                "  j k  Ctrl-d Ctrl-u",
                "the file's cursor by a line; half a page",
            ),
            ("  g G  h l 0", "the top or the end; sideways, and back"),
            (
                "  / n N",
                "search, ignoring case unless it has a capital; again",
            ),
            ("  Esc  c", "back to the questions"),
            ("q", "quit; every answer is already saved"),
        ];
        let width = 76.min(area.width);
        let height = (KEYS.len() as u16 + 4).min(area.height);
        let rect = Rect {
            x: (area.width - width) / 2,
            y: (area.height - height) / 2,
            width,
            height,
        };
        frame.render_widget(Clear, rect);
        let block = title_block(&self.theme, " keys (any key closes) ".into(), true);
        let inner = block.inner(rect);
        frame.render_widget(block, rect);
        let lines: Vec<Line> = KEYS
            .iter()
            .map(|(k, what)| {
                vec![
                    Span::styled(format!("{k:<18}"), style::accent(&self.theme)),
                    (*what).into(),
                ]
                .into()
            })
            .collect();
        frame.render_widget(Paragraph::new(lines), inner);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kinds::Sample;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn key(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
    }

    /// A dataset of two samples, written as a draw would leave them.
    fn dataset() -> (tempfile::TempDir, Dataset) {
        let dir = tempfile::tempdir().unwrap();
        let mut samples = Vec::new();
        for (name, test) in [("one", "parses"), ("two", "prints")] {
            let sample_dir = dir.path().join(name);
            std::fs::create_dir_all(&sample_dir).unwrap();
            std::fs::write(
                sample_dir.join("file.rs.test"),
                format!("fn helper() {{}}\n\n#[test]\nfn {test}() {{\n    assert_eq!(parse_review(\"ab\"), 2);\n}}\n"),
            )
            .unwrap();
            samples.push(Sample {
                name: name.into(),
                repository: "someone/alpha".into(),
                commit: "0123456789abcdef".into(),
                path: "src/lib.rs".into(),
                line: 4,
                end: 6,
                test: test.into(),
                label: test.into(),
                names: vec!["src/review.rs".into()],
                drives: String::new(),
                files: vec![],
                status: Status::Pending,
                answers: vec![None; options().len()],
                notes: String::new(),
            });
        }
        let dataset = Dataset {
            dir: dir.path().to_path_buf(),
            samples,
        };
        dataset.save().unwrap();
        (dir, dataset)
    }

    fn screen(term: &mut Terminal<TestBackend>, l: &mut Labeller) -> String {
        term.draw(|f| l.draw(f)).unwrap();
        let buffer = term.backend().buffer().clone();
        let mut out = String::new();
        for y in 0..buffer.area.height {
            for x in 0..buffer.area.width {
                out.push_str(buffer[(x, y)].symbol());
            }
            out.push('\n');
        }
        out
    }

    #[test]
    fn answers_are_saved_as_they_are_given() {
        let (dir, dataset) = dataset();
        let mut l = Labeller::new(dataset, Theme::default(), None).unwrap();
        let mut term = Terminal::new(TestBackend::new(140, 40)).unwrap();
        let s = screen(&mut term, &mut l);
        assert!(s.contains("someone/alpha @ 0123456"), "{s}");
        assert!(s.contains("What artifact is being tested?"), "{s}");
        assert!(s.contains("▌fn parses() {"), "the test is marked: {s}");
        assert!(!s.contains("rules y"), "the rules are hidden: {s}");

        l.handle_key(key('y'));
        assert_eq!(l.cursor, 1);
        l.handle_key(key('N'));
        let saved = Dataset::open(dir.path()).unwrap();
        let first: Vec<_> = saved.samples[0].answers[..5].to_vec();
        assert_eq!(
            first,
            [
                Some(Answer::Yes),
                Some(Answer::No),
                Some(Answer::No),
                Some(Answer::No),
                Some(Answer::No)
            ]
        );
        assert_eq!(l.cursor, 5, "on to the next question");
        let s = screen(&mut term, &mut l);
        assert!(s.contains("What artifact is being tested?  Code"), "{s}");

        // Every option answered: done.
        for _ in 5..options().len() {
            l.handle_key(key('n'));
        }
        assert_eq!(l.dataset.samples[0].status, Status::Done);
        assert_eq!(
            Dataset::open(dir.path()).unwrap().samples[0].status,
            Status::Done
        );

        // r shows the rules beside, and where they differ.
        l.handle_key(key('r'));
        let s = screen(&mut term, &mut l);
        assert!(s.contains("rules y ✓"), "{s}");
        assert!(
            s.contains("✗"),
            "they say it is dynamic; the answers said not: {s}"
        );
    }

    #[test]
    fn samples_skip_and_move_on() {
        let (dir, dataset) = dataset();
        let mut l = Labeller::new(dataset, Theme::default(), None).unwrap();
        l.handle_key(key('s'));
        assert_eq!(l.dataset.samples[0].status, Status::Skipped);
        l.handle_key(key('}'));
        assert_eq!(l.at, 1, "the next one not finished");
        l.handle_key(key('['));
        assert_eq!(l.at, 0);
        l.handle_key(key('e'));
        for c in "not a test".chars() {
            l.handle_key(key(c));
        }
        l.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        let saved = Dataset::open(dir.path()).unwrap();
        assert_eq!(saved.samples[0].notes, "not a test");
        assert_eq!(saved.samples[0].status, Status::Skipped);
        l.handle_key(key('q'));
        assert!(l.quit);
    }

    #[test]
    fn the_file_is_read_with_a_cursor_and_searched() {
        let (_dir, dataset) = dataset();
        let mut l = Labeller::new(dataset, Theme::default(), None).unwrap();
        let mut term = Terminal::new(TestBackend::new(140, 40)).unwrap();
        screen(&mut term, &mut l);
        l.handle_key(key('c'));
        assert_eq!(l.focus, Focus::Code);
        assert_eq!(l.line, 3, "on the test's first line");
        let s = screen(&mut term, &mut l);
        assert!(s.contains("line 4 of 6"), "{s}");
        assert!(s.contains("Esc questions"), "{s}");

        // j and k move the file's cursor, not the questions'.
        l.handle_key(key('j'));
        assert_eq!((l.line, l.cursor), (4, 0));
        l.handle_key(key('g'));
        assert_eq!(l.line, 0);
        l.handle_key(key('G'));
        assert_eq!(l.line, 5);

        // A search finds the helper, from the other end of the file.
        l.handle_key(key('/'));
        for c in "helper".chars() {
            l.handle_key(key(c));
        }
        let s = screen(&mut term, &mut l);
        assert!(s.contains("/helper▏"), "{s}");
        l.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(l.line, 0);
        let s = screen(&mut term, &mut l);
        assert!(s.contains("/helper: 1 of 1, from the other end"), "{s}");

        // n is the next match here, not an answer.
        l.handle_key(key('n'));
        assert!(l.dataset.samples[0].answers.iter().all(Option::is_none));

        // Sideways, and back.
        l.handle_key(key('l'));
        assert_eq!(l.sideways, SIDEWAYS);
        l.handle_key(key('0'));
        assert_eq!(l.sideways, 0);

        l.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(l.focus, Focus::Questions);
        assert!(!l.quit, "Esc leaves the file, not the screen");
        l.handle_key(key('y'));
        assert_eq!(l.dataset.samples[0].answers[0], Some(Answer::Yes));

        // / from the questions searches at once.
        l.handle_key(key('/'));
        assert_eq!((l.focus, l.typing.as_deref()), (Focus::Code, Some("")));
    }
}
