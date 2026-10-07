//! The end-to-end tests: the list on the left, and on the right the selected test — how its
//! last run went, its recording played from frames, and its steps against the recording's
//! clock.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::e2e::Opening;
use crate::recording::{Colour, Kind, Recording, Run};
use crate::stretch::{Outcome, Touched};
use crate::theme::Theme;
use crate::tui::app::{App, E2eRow, EndToEndState, EndToEndTestState, Player, Screen, TestRow};
use crate::tui::style;

use super::{list_lines, title_block, wrap_text};

pub(super) fn draw_end_to_end(frame: &mut Frame, app: &mut App, area: Rect) {
    let theme = app.theme.clone();
    let base_status = app.base_run_status();
    let Screen::EndToEnd(state) = app.screens.last_mut().expect("screen") else {
        return;
    };
    let title: Line = match &state.opening {
        Some(o) if !o.missing() => {
            let mut parts = vec![format!(
                "{} end-to-end {} of {} tests",
                o.tests.len(),
                if o.tests.len() == 1 { "test" } else { "tests" },
                o.total
            )];
            if o.failed > 0 {
                parts.push(format!("{} failed", o.failed));
            }
            if o.unrecorded > 0 {
                parts.push(format!("{} without a recording", o.unrecorded));
            }
            let mut title: Vec<Span> = vec![
                " end to end ".into(),
                format!("{} ", parts.join(" · ")).dim(),
            ];
            if let Some(r) = state.report() {
                title.push(format!("· marking {} (w) ", r.span.label).dim());
            }
            title.into()
        }
        _ => " end to end ".into(),
    };
    let block = title_block(&theme, title, true);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let Some(opening) = &state.opening else {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                " reading the tests…",
                style::dim(&theme),
            ))),
            inner,
        );
        return;
    };
    if opening.missing() {
        draw_missing(frame, &theme, opening, inner);
        return;
    }
    if state.full {
        draw_recording(frame, &theme, &state.player, inner, true, "");
        return;
    }
    // Wider while the stretch has something to say, since its rows are sentences.
    let share = if state.report().is_some() { 2 } else { 1 };
    let list_width = (inner.width * share / (share + 2)).clamp(24.min(inner.width), 64);
    let [list, gap, detail] = Layout::horizontal([
        Constraint::Length(list_width),
        Constraint::Length(1),
        Constraint::Fill(1),
    ])
    .areas(inner);
    let _ = gap;
    app.measured.list_height = list.height as usize;
    draw_list(frame, &theme, state, base_status.as_deref(), list);
    draw_detail(frame, &theme, state, detail);
}

/// The repository has none: the first finding, what one would look like, and the offer.
fn draw_missing(frame: &mut Frame, theme: &Theme, opening: &Opening, area: Rect) {
    let width = area.width.saturating_sub(2) as usize;
    let dim = style::dim(theme);
    let mut lines: Vec<Line> = vec![
        Line::default(),
        Line::from(Span::styled(
            " No end-to-end tests.",
            style::fg(theme.moved).bold(),
        )),
        Line::default(),
    ];
    let what = match opening.kind.as_deref() {
        Some(kind) => format!("{kind}, it seems"),
        None => "this software".to_string(),
    };
    let how = match opening.kind.as_deref() {
        Some("a web page") => {
            "For a web page, one opens it in a real headless browser, taps and types the way a \
             person would, and checks what is on the screen."
        }
        Some("a terminal program") => {
            "For a terminal program, one runs the built program in a pseudo-terminal, presses \
             keys the way a person would, and checks what is on the screen."
        }
        Some(_) => {
            "For a command, one runs the built program with the arguments and input a person \
             would give it, and checks what it prints and what it leaves on disk."
        }
        None => {
            "This looks like a library, which nobody drives directly: an end-to-end test of \
             one is a program that uses it the way its users do."
        }
    };
    let paragraphs = [
        format!(
            "Nothing here drives {what}, the way a person does and checks what comes back, so \
             nothing shows that it works. Reading its code is the slow way to find out."
        ),
        how.to_string(),
        "Each test leaves a recording of its run, and that recording is what this screen \
         shows: the software doing what the test says."
            .to_string(),
    ];
    for p in paragraphs {
        lines.extend(
            wrap_text(&p, width)
                .into_iter()
                .map(|l| Line::from(format!(" {l}"))),
        );
        lines.push(Line::default());
    }
    if opening.total > 0 {
        lines.push(Line::from(Span::styled(
            format!(
                " The repository has {} {} of other kinds: P lists them.",
                opening.total,
                if opening.total == 1 { "test" } else { "tests" }
            ),
            dim,
        )));
        lines.push(Line::default());
    }
    lines.push(Line::from(vec![
        "  a ".bold(),
        " ask the agent to write the first ones, each leaving a recording to watch here".into(),
    ]));
    lines.push(Line::from(vec!["  q ".bold(), " the code".into()]));
    frame.render_widget(Paragraph::new(lines), area);
}

fn draw_list(
    frame: &mut Frame,
    theme: &Theme,
    state: &mut EndToEndState,
    base_status: Option<&str>,
    area: Rect,
) {
    let Some(opening) = &state.opening else {
        return;
    };
    state.rows.ensure_visible(area.height as usize);
    let state = &*state;
    let report = state.report();
    let width = area.width as usize;
    let dim = style::dim(theme);
    let lines = list_lines(
        style::cursor(theme),
        state.rows.items.iter(),
        state.rows.cursor,
        state.rows.scroll,
        area.height as usize,
        true,
        |row| match row {
            E2eRow::Stretch => Line::from(Span::styled(
                clip(
                    &format!(
                        "In {}",
                        report.map_or("the stretch", |r| r.span.label.as_str())
                    ),
                    width,
                ),
                style::accent(theme).bold(),
            )),
            E2eRow::Note(text, warn) => Line::from(Span::styled(
                clip(&format!(" {text}"), width),
                if *warn { style::fg(theme.moved) } else { dim },
            )),
            E2eRow::Change(i) => {
                let Some(c) = report.and_then(|r| r.changes.get(*i)) else {
                    return Line::default();
                };
                let what = format!(" ✗ {} ", c.what.word());
                Line::from(vec![
                    Span::styled(what.clone(), style::fg(theme.delete_fg).bold()),
                    Span::raw(clip(&c.label, width.saturating_sub(what.chars().count()))),
                ])
            }
            E2eRow::UnranHead => {
                let (n, of) = report.map_or((0, 0), |r| (r.unran_lines, r.changed));
                Line::from(Span::styled(
                    clip(
                        &format!("No end-to-end test ran {n} of {of} changed lines"),
                        width,
                    ),
                    style::accent(theme).bold(),
                ))
            }
            E2eRow::Unran(i) => {
                let Some(u) = report.and_then(|r| r.unran.get(*i)) else {
                    return Line::default();
                };
                let said = format!("  {} of {}", u.lines, u.changed);
                Line::from(vec![
                    Span::styled(" ▌", style::fg(theme.moved)),
                    Span::raw(clip(&u.path, width.saturating_sub(said.len() + 2))),
                    Span::styled(said, dim),
                ])
            }
            E2eRow::MoreUnran(n) => Line::from(Span::styled(
                clip(
                    &format!(" … {n} more {}", if *n == 1 { "file" } else { "files" }),
                    width,
                ),
                dim,
            )),
            E2eRow::Base => {
                let Some(b) = report.and_then(|r| r.base.as_ref()) else {
                    return Line::default();
                };
                let (text, look) = match (base_status, b.recorded, &b.command) {
                    (Some(status), _, _) => (format!(" ⟳ {status}"), style::accent(theme)),
                    (None, n, _) if n > 0 => (
                        format!(
                            " Before {}: recorded, {n} {}",
                            b.short,
                            if n == 1 { "test" } else { "tests" }
                        ),
                        dim,
                    ),
                    (None, _, Some(_)) => (
                        format!(" Before {}: not recorded, b records it", b.short),
                        style::fg(theme.moved),
                    ),
                    (None, _, None) => (
                        format!(" Before {}: no command records the tests", b.short),
                        dim,
                    ),
                };
                Line::from(Span::styled(clip(&text, width), look))
            }
            E2eRow::File(path, n) => Line::from(vec![
                Span::styled(
                    clip(path, width.saturating_sub(4)),
                    style::accent(theme).bold(),
                ),
                Span::styled(format!(" {n}"), dim),
            ]),
            E2eRow::Test(i) => {
                let t = &opening.tests[*i];
                let (mark, colour) = match &t.recording {
                    Some(r) if r.passed => ("✓", style::fg(theme.insert_fg)),
                    Some(_) => ("✗", style::fg(theme.delete_fg).bold()),
                    None => ("○", dim),
                };
                let lock = if t.security { "⚿ " } else { "" };
                let label_style = if t.test.ignored { dim } else { Style::new() };
                let skipped = if t.test.ignored { " (skipped)" } else { "" };
                let badges = badges(theme, state.stretch_of(t));
                let used: usize = badges.iter().map(|b| b.width()).sum();
                let mut spans = vec![
                    Span::styled(format!(" {mark} "), colour),
                    Span::styled(
                        clip(
                            &format!("{lock}{}{skipped}", t.test.label),
                            width.saturating_sub(3 + used),
                        ),
                        label_style,
                    ),
                ];
                spans.extend(badges);
                Line::from(spans)
            }
            E2eRow::Orphans(n) => Line::from(Span::styled(
                clip(&format!("recordings of tests no longer here {n}"), width),
                style::fg(theme.moved),
            )),
            E2eRow::Orphan(i) => {
                let r = &opening.orphans[*i];
                Line::from(Span::styled(clip(&format!(" ? {}", r.test), width), dim))
            }
        },
    );
    frame.render_widget(Paragraph::new(lines), area);
}

/// What the stretch says of a test, after its sentence: `Δ12` for the changed lines it ran,
/// and how it went before against now, with what changed in its run.
fn badges(
    theme: &Theme,
    (touched, outcome): (Option<&Touched>, Option<&Outcome>),
) -> Vec<Span<'static>> {
    let mut out = Vec::new();
    if let Some(t) = touched {
        out.push(Span::styled(format!(" Δ{}", t.lines), style::accent(theme)));
    }
    if let Some(o) = outcome {
        let mark = |passed: Option<bool>| match passed {
            Some(true) => "✓",
            Some(false) => "✗",
            None => "○",
        };
        if o.before != o.after {
            let look = if o.after == Some(false) {
                style::fg(theme.delete_fg).bold()
            } else {
                style::fg(theme.insert_fg)
            };
            out.push(Span::styled(
                format!(" {}→{}", mark(o.before), mark(o.after)),
                look,
            ));
        }
        if o.screens_changed.is_some_and(|n| n > 0) {
            out.push(Span::styled(" screen changed", style::fg(theme.moved)));
        } else if o.steps_changed > 0 {
            out.push(Span::styled(" steps changed", style::fg(theme.moved)));
        }
    }
    out
}

/// What a row of the stretch says, beside the list.
fn draw_stretch_detail(frame: &mut Frame, theme: &Theme, state: &EndToEndState, area: Rect) {
    let (Some(report), Some(row)) = (state.report(), state.rows.current()) else {
        return;
    };
    let width = area.width as usize;
    let dim = style::dim(theme);
    let mut lines: Vec<Line> = Vec::new();
    let para = |lines: &mut Vec<Line>, text: &str, look: Style| {
        lines.extend(
            wrap_text(text, width)
                .into_iter()
                .map(|l| Line::from(Span::styled(l, look))),
        );
    };
    match row {
        E2eRow::Change(i) => {
            let Some(c) = report.changes.get(*i) else {
                return;
            };
            lines.push(Line::from(Span::styled(
                clip(&c.label, width),
                Style::new().bold(),
            )));
            lines.push(Line::from(Span::styled(
                clip(
                    &format!("{}:{} · {}", c.path, c.line, c.level_title()),
                    width,
                ),
                dim,
            )));
            let mut said = format!("In {}, this test was {}", report.span.label, c.what.word());
            if let Some((was, is)) = c.checks {
                said.push_str(&format!(": {was} checks, now {is}"));
            }
            if let Some(was) = &c.was {
                said.push_str(&format!("; it was {was}"));
            }
            said.push('.');
            lines.push(Line::default());
            para(&mut lines, &said, Style::new());
            lines.push(Line::default());
            para(
                &mut lines,
                "A test that asks less than it did is the first thing to read: an agent that cannot make a test pass is tempted to make it stop asking. Enter shows the diff of its file across the stretch.",
                dim,
            );
        }
        E2eRow::Unran(i) => {
            let Some(u) = report.unran.get(*i) else {
                return;
            };
            lines.push(Line::from(Span::styled(
                clip(&u.path, width),
                Style::new().bold(),
            )));
            para(
                &mut lines,
                &format!(
                    "{} of the {} lines of code changed here in {} ran in no end-to-end test.",
                    u.lines, u.changed, report.span.label
                ),
                Style::new(),
            );
            let runs: Vec<String> = u
                .ranges
                .iter()
                .map(|(a, b)| {
                    if a == b {
                        a.to_string()
                    } else {
                        format!("{a}-{b}")
                    }
                })
                .collect();
            para(&mut lines, &format!("lines {}", runs.join(", ")), dim);
            lines.push(Line::default());
            para(
                &mut lines,
                "Nothing the software was seen doing went through them: this is code a person still has to read. Enter opens the file with them marked ▌.",
                dim,
            );
        }
        E2eRow::Base => {
            let Some(b) = &report.base else {
                return;
            };
            lines.push(Line::from(Span::styled(
                format!("Before: {}", b.short),
                Style::new().bold(),
            )));
            let said = if b.recorded > 0 {
                format!(
                    "The tests are recorded as they ran at {}, where {} starts. Each test shows how it went then against now; a test's own screen plays both.",
                    b.short, report.span.label
                )
            } else {
                match &b.command {
                    Some(command) => format!(
                        "To see each test as it ran before {}, the tests are recorded at {}: b makes a worktree of that commit inside the git directory and runs {command} there. It runs the code as it was, and can take minutes.",
                        report.span.label, b.short
                    ),
                    None => "There is no command here that records the end-to-end tests: a Makefile target called e2e, or a configured check named for them, would be.".to_string(),
                }
            };
            para(&mut lines, &said, Style::new());
        }
        _ => {}
    }
    frame.render_widget(Paragraph::new(lines), area);
}

fn draw_detail(frame: &mut Frame, theme: &Theme, state: &EndToEndState, area: Rect) {
    if matches!(
        state.rows.current(),
        Some(E2eRow::Change(_) | E2eRow::Unran(_) | E2eRow::Base)
    ) {
        draw_stretch_detail(frame, theme, state, area);
        return;
    }
    let width = area.width as usize;
    let dim = style::dim(theme);
    let mut lines: Vec<Line> = Vec::new();
    let test = state.test();
    let recording = state.recording();
    match (test, recording) {
        (Some(t), _) => {
            lines.push(Line::from(Span::styled(
                clip(&t.test.label, width),
                Style::new().bold(),
            )));
            let mut place = format!("{}:{} · {}", t.test.path, t.test.line, t.why);
            if t.security {
                place.push_str(" · security");
            }
            lines.push(Line::from(Span::styled(clip(&place, width), dim)));
        }
        (None, Some(r)) => {
            lines.push(Line::from(Span::styled(
                clip(&r.test, width),
                Style::new().bold(),
            )));
            lines.push(Line::from(Span::styled(
                clip(
                    &format!("{}:{} · its test is not there any more", r.file, r.line),
                    width,
                ),
                style::fg(theme.moved),
            )));
        }
        (None, None) => return,
    }
    if state.from_overview {
        lines.push(Line::from(Span::styled(
            "tests as OVERVIEW.md has them, while the symbols are read",
            dim,
        )));
    }
    let Some(r) = recording else {
        lines.push(Line::default());
        lines.push(Line::from(Span::styled(
            "No recording: its last run left none behind, or it has not run here.",
            style::fg(theme.moved),
        )));
        lines.push(Line::from(Span::styled("Enter opens the test.", dim)));
        frame.render_widget(Paragraph::new(lines), area);
        return;
    };
    lines.push(result_line(theme, r, test.and_then(|t| t.stale), width));
    if let Some(commit) = &r.commit {
        let short: String = commit.chars().take(7).collect();
        let dirty = if r.dirty {
            " with uncommitted changes"
        } else {
            ""
        };
        lines.push(Line::from(Span::styled(
            format!("recorded at {short}{dirty}"),
            dim,
        )));
    }
    for step in r.failures().take(3) {
        let detail = step
            .detail
            .as_deref()
            .and_then(|d| d.lines().find(|l| !l.trim().is_empty()))
            .unwrap_or("");
        lines.push(Line::from(vec![
            Span::styled("✗ ", style::fg(theme.delete_fg).bold()),
            Span::raw(clip(
                &format!("{} — {detail}", step.text),
                width.saturating_sub(2),
            )),
        ]));
    }
    let head = lines.len() as u16;
    let [top, rest] =
        Layout::vertical([Constraint::Length(head + 1), Constraint::Fill(1)]).areas(area);
    frame.render_widget(Paragraph::new(lines), top);
    if r.kind == Some(Kind::Terminal) {
        let cast_height = match state.player.cast() {
            Some(Ok(cast)) => cast.height + 2,
            _ => 3,
        };
        // The recording gets what it needs, short of leaving the steps no room at all.
        let room = rest.height.saturating_sub(4).max(3);
        let [player, steps] = Layout::vertical([
            Constraint::Length(cast_height.min(room)),
            Constraint::Fill(1),
        ])
        .areas(rest);
        draw_recording(frame, theme, &state.player, player, false, "");
        draw_steps(frame, theme, &state.player, r, steps);
    } else {
        let mut said = vec![Line::default()];
        if let Some(media) = r.media() {
            said.push(Line::from(vec![
                Span::styled(media, Style::new().bold()),
                Span::styled(" — o opens it in the system's player", dim),
            ]));
        } else if r.recording.is_some() {
            said.push(Line::from(Span::styled(
                "Its recording is not there any more.",
                style::fg(theme.moved),
            )));
        } else {
            said.push(Line::from(Span::styled(
                "Its run left no recording.",
                style::dim(theme),
            )));
        }
        said.push(Line::default());
        let [note, steps] =
            Layout::vertical([Constraint::Length(said.len() as u16), Constraint::Fill(1)])
                .areas(rest);
        frame.render_widget(Paragraph::new(said), note);
        draw_steps(frame, theme, &state.player, r, steps);
    }
}

/// `✓ passed 3 min ago in 4.8 s · matches the tree`.
fn result_line(
    theme: &Theme,
    r: &Recording,
    stale: Option<crate::analysis::Staleness>,
    width: usize,
) -> Line<'static> {
    let (word, colour) = if r.passed {
        ("✓ passed", style::fg(theme.insert_fg))
    } else {
        ("✗ failed", style::fg(theme.delete_fg).bold())
    };
    let mut rest = String::new();
    if let Some(age) = r.started.as_deref().and_then(age) {
        rest.push_str(&format!(" {age}"));
    }
    if r.duration_ms > 0 {
        rest.push_str(&format!(" in {:.1} s", r.duration_ms as f64 / 1000.0));
    }
    let (fresh, fresh_style) = match stale {
        Some(s) if s.commits == 0 && s.files == 0 => {
            ("matches the tree".to_string(), style::dim(theme))
        }
        Some(s) => {
            let mut parts = Vec::new();
            if s.commits > 0 {
                parts.push(plural(s.commits, "commit", "commits"));
            }
            if s.files > 0 {
                parts.push(plural(s.files, "file changed", "files changed"));
            }
            (
                format!("{} since", parts.join(" and ")),
                style::fg(theme.moved),
            )
        }
        None => (String::new(), style::dim(theme)),
    };
    let mut spans = vec![
        Span::styled(word, colour),
        Span::styled(rest, style::dim(theme)),
    ];
    if !fresh.is_empty() {
        spans.push(Span::styled(" · ", style::dim(theme)));
        spans.push(Span::styled(fresh, fresh_style));
    }
    let used: usize = spans.iter().map(|s| s.width()).sum();
    if used > width {
        let text: String = spans.iter().map(|s| s.content.to_string()).collect();
        return Line::from(Span::styled(clip(&text, width), colour));
    }
    Line::from(spans)
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// How long ago an RFC 3339 time was, in the largest unit that fits.
fn age(when: &str) -> Option<String> {
    let then = chrono::DateTime::parse_from_rfc3339(when).ok()?;
    let secs = (chrono::Utc::now() - then.with_timezone(&chrono::Utc)).num_seconds();
    let secs = secs.max(0);
    Some(match secs {
        0..60 => format!("{secs} s ago"),
        60..3600 => format!("{} min ago", secs / 60),
        3600..86400 => format!("{} h ago", secs / 3600),
        _ => plural((secs / 86400) as usize, "day ago", "days ago"),
    })
}

/// Milliseconds as `m:ss.s`.
pub(crate) fn clock(ms: u64) -> String {
    format!("{}:{:04.1}", ms / 60_000, (ms % 60_000) as f64 / 1000.0)
}

/// The selected terminal recording at the playback position, in a box that says where it is.
fn draw_recording(
    frame: &mut Frame,
    theme: &Theme,
    state: &Player,
    area: Rect,
    full: bool,
    label: &str,
) {
    let position = clock(state.position_ms);
    let total = clock(state.duration_ms());
    let playing = if state.playing.is_some() {
        "▶ playing"
    } else {
        "paused"
    };
    let cast = state.cast();
    let mut title = format!(" {label}{position} / {total} {playing} ");
    if let Some(Ok(cast)) = cast {
        let inner_w = area.width.saturating_sub(2);
        let inner_h = area.height.saturating_sub(2);
        if cast.width > inner_w || cast.height > inner_h {
            title.push_str(&format!(
                "· cropped to {}x{} of {}x{} ({}) ",
                inner_w.min(cast.width),
                inner_h.min(cast.height),
                cast.width,
                cast.height,
                if full {
                    "a wider terminal shows it whole"
                } else {
                    "z shows it whole"
                }
            ));
        }
    }
    let block = title_block(
        theme,
        Line::from(Span::styled(title, style::dim(theme))),
        false,
    );
    let inner = block.inner(area);
    frame.render_widget(block, area);
    match cast {
        Some(Ok(cast)) => {
            let screen = cast.screen_at(state.position_ms);
            let lines: Vec<Line> = screen
                .into_iter()
                .take(inner.height as usize)
                .map(|runs| Line::from(runs.into_iter().map(paint).collect::<Vec<_>>()))
                .collect();
            frame.render_widget(Paragraph::new(lines), inner);
        }
        Some(Err(e)) => frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                format!("cannot play the recording: {e}"),
                style::fg(theme.delete_fg),
            ))),
            inner,
        ),
        None => frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "nothing to play here",
                style::dim(theme),
            ))),
            inner,
        ),
    }
}

/// One run of a recorded screen, styled the way the terminal it was recorded in drew it.
fn paint(run: Run) -> Span<'static> {
    let colour = |c: Colour| match c {
        Colour::Index(i) => Color::Indexed(i),
        Colour::Rgb(r, g, b) => Color::Rgb(r, g, b),
    };
    let mut style = Style::new();
    if let Some(fg) = run.fg {
        style = style.fg(colour(fg));
    }
    if let Some(bg) = run.bg {
        style = style.bg(colour(bg));
    }
    let mut modifier = Modifier::empty();
    if run.bold {
        modifier |= Modifier::BOLD;
    }
    if run.italic {
        modifier |= Modifier::ITALIC;
    }
    if run.underline {
        modifier |= Modifier::UNDERLINED;
    }
    if run.inverse {
        modifier |= Modifier::REVERSED;
    }
    Span::styled(run.text, style.add_modifier(modifier))
}

/// The test's steps against the recording's clock, the one playback is at marked.
fn draw_steps(frame: &mut Frame, theme: &Theme, state: &Player, r: &Recording, area: Rect) {
    if area.height == 0 || r.steps.is_empty() {
        return;
    }
    let width = area.width as usize;
    let current = state.current_step();
    let height = area.height as usize;
    let scroll = current.map_or(0, |c| (c + 1).saturating_sub(height.saturating_sub(1)));
    let dim = style::dim(theme);
    let lines: Vec<Line> = r
        .steps
        .iter()
        .enumerate()
        .skip(scroll)
        .take(height)
        .map(|(i, s)| {
            let (mark, mark_style) = match (s.kind.as_str(), s.passed) {
                ("check", Some(false)) => ("✗", style::fg(theme.delete_fg).bold()),
                ("check", _) => ("✓", style::fg(theme.insert_fg)),
                _ => ("›", dim),
            };
            let text_style = if s.kind == "check" { dim } else { Style::new() };
            let line = Line::from(vec![
                Span::styled(format!("{} ", clock(s.at_ms)), dim),
                Span::styled(format!("{mark} "), mark_style),
                Span::styled(clip(&s.text, width.saturating_sub(10)), text_style),
            ]);
            if Some(i) == current {
                line.style(style::cursor(theme))
            } else {
                line
            }
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), area);
}

/// `text` cut to `width` characters, with an ellipsis when it was cut.
fn clip(text: &str, width: usize) -> String {
    if text.chars().count() <= width {
        return text.to_string();
    }
    let mut out: String = text.chars().take(width.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// One end-to-end test on its own: how its last run went, its recording, and the list that
/// leads into the code and the documentation.
pub(super) fn draw_end_to_end_test(frame: &mut Frame, app: &mut App, area: Rect) {
    let theme = app.theme.clone();
    let Screen::EndToEndTest(state) = app.screens.last_mut().expect("screen") else {
        return;
    };
    let title: Line = match &state.detail {
        Some(d) => vec![" test ".into(), format!("{} ", d.test.test.label).bold()].into(),
        None => " test ".into(),
    };
    let block = title_block(&theme, title, true);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let short = state
        .comparison
        .as_ref()
        .and_then(|c| c.base.as_ref())
        .map(|b| b.short.clone())
        .unwrap_or_default();
    let two = state.before.recording.is_some();
    if state.full {
        if two {
            let [left, right] =
                Layout::horizontal([Constraint::Fill(1), Constraint::Fill(1)]).areas(inner);
            draw_recording(
                frame,
                &theme,
                &state.before,
                left,
                true,
                &format!("at {short} · "),
            );
            draw_recording(frame, &theme, &state.player, right, true, "now · ");
        } else {
            draw_recording(frame, &theme, &state.player, inner, true, "");
        }
        return;
    }
    let width = inner.width as usize;
    let dim = style::dim(&theme);
    let mut head: Vec<Line> = Vec::new();
    if let Some(d) = &state.detail {
        let t = &d.test;
        let mut place = format!("{}:{} · {}", t.test.path, t.test.line, t.why);
        if t.security {
            place.push_str(" · security");
        }
        head.push(Line::from(Span::styled(clip(&place, width), dim)));
        if let Some(r) = &t.recording {
            head.push(result_line(&theme, r, t.stale, width));
            for step in r.failures().take(3) {
                let detail = step
                    .detail
                    .as_deref()
                    .and_then(|d| d.lines().find(|l| !l.trim().is_empty()))
                    .unwrap_or("");
                head.push(Line::from(vec![
                    Span::styled("✗ ", style::fg(theme.delete_fg).bold()),
                    Span::raw(clip(
                        &format!("{} — {detail}", step.text),
                        width.saturating_sub(2),
                    )),
                ]));
            }
        }
    }
    let recording = state.player.recording.clone();
    let height_of = |player: &Player| match (&player.recording, player.cast()) {
        (Some(r), Some(Ok(cast))) if r.kind == Some(Kind::Terminal) => {
            (cast.height + 2).min(inner.height / 2).max(3)
        }
        (Some(r), _) if r.kind == Some(Kind::Terminal) => 3,
        (Some(r), _) if r.media().is_some() => 1,
        _ => 0,
    };
    // Before and after side by side when both fit whole, and one above the other when not.
    let cast_width = |player: &Player| match player.cast() {
        Some(Ok(cast)) => cast.width + 2,
        _ => 0,
    };
    let side_by_side = two && cast_width(&state.before) + cast_width(&state.player) <= inner.width;
    let player_height = match (two, side_by_side) {
        (true, true) => height_of(&state.player).max(height_of(&state.before)),
        (true, false) => {
            (height_of(&state.player) + height_of(&state.before)).min(inner.height * 2 / 3)
        }
        _ => height_of(&state.player),
    };
    let [top, player, gap, list] = Layout::vertical([
        Constraint::Length(head.len() as u16),
        Constraint::Length(player_height),
        Constraint::Length(1),
        Constraint::Fill(1),
    ])
    .areas(inner);
    let _ = gap;
    frame.render_widget(Paragraph::new(head), top);
    match &recording {
        Some(r) if r.kind == Some(Kind::Terminal) && two => {
            let [was, is] = if side_by_side {
                Layout::horizontal([Constraint::Fill(1), Constraint::Fill(1)]).areas(player)
            } else {
                Layout::vertical([Constraint::Fill(1), Constraint::Fill(1)]).areas(player)
            };
            draw_recording(
                frame,
                &theme,
                &state.before,
                was,
                false,
                &format!("at {short} · "),
            );
            draw_recording(frame, &theme, &state.player, is, false, "now · ");
        }
        Some(r) if r.kind == Some(Kind::Terminal) => {
            draw_recording(frame, &theme, &state.player, player, false, "");
        }
        Some(r) => {
            if let Some(media) = r.media() {
                frame.render_widget(
                    Paragraph::new(Line::from(vec![
                        Span::styled(media, Style::new().bold()),
                        Span::styled(" — o opens it in the system's player", dim),
                    ])),
                    player,
                );
            }
        }
        None => {}
    }
    app.measured.list_height = list.height as usize;
    draw_test_rows(frame, &theme, state, list);
}

fn draw_test_rows(frame: &mut Frame, theme: &Theme, state: &mut EndToEndTestState, area: Rect) {
    state.rows.ensure_visible(area.height as usize);
    let width = area.width as usize;
    let dim = style::dim(theme);
    let current = state.player.current_step();
    let detail = state.detail.as_ref();
    let comparison = state.comparison.as_ref();
    // The pair playback is at: the one holding the after side's current step.
    let current_pair = comparison
        .and_then(|c| {
            let at = current?;
            let mut seen = 0;
            c.steps.iter().position(|p| {
                if p.after.is_none() {
                    return false;
                }
                seen += 1;
                seen - 1 == at
            })
        })
        .unwrap_or(usize::MAX);
    let lines = list_lines(
        style::cursor(theme),
        state.rows.items.iter(),
        state.rows.cursor,
        state.rows.scroll,
        area.height as usize,
        true,
        |row| {
            let Some(d) = detail else {
                return match row {
                    TestRow::Text(text) => Line::from(Span::styled(format!(" {text}"), dim)),
                    _ => Line::default(),
                };
            };
            let steps = d
                .test
                .recording
                .as_ref()
                .map(|r| r.steps.as_slice())
                .unwrap_or(&[]);
            match row {
                TestRow::Heading(text) => {
                    Line::from(Span::styled(clip(text, width), style::accent(theme).bold()))
                }
                TestRow::Text(text) => {
                    Line::from(Span::styled(clip(&format!(" {text}"), width), dim))
                }
                TestRow::Step(i) => {
                    let s = &steps[*i];
                    let (mark, mark_style) = match (s.kind.as_str(), s.passed) {
                        ("check", Some(false)) => ("✗", style::fg(theme.delete_fg).bold()),
                        ("check", _) => ("✓", style::fg(theme.insert_fg)),
                        _ => ("›", dim),
                    };
                    let now = Some(*i) == current;
                    let text_style = match (now, s.kind.as_str()) {
                        (true, _) => style::accent(theme).bold(),
                        (false, "check") => dim,
                        _ => Style::new(),
                    };
                    Line::from(vec![
                        Span::styled(if now { "▶" } else { " " }, style::accent(theme)),
                        Span::styled(format!("{} ", clock(s.at_ms)), dim),
                        Span::styled(format!("{mark} "), mark_style),
                        Span::styled(clip(&s.text, width.saturating_sub(11)), text_style),
                    ])
                }
                TestRow::Pair(i) => {
                    let Some(pair) = comparison.and_then(|c| c.steps.get(*i)) else {
                        return Line::default();
                    };
                    pair_line(theme, pair, *i == current_pair, width)
                }
                TestRow::Source(i) => {
                    let line = steps.get(*i).map_or(0, |s| s.line);
                    let text = d.sources.get(*i).cloned().flatten().unwrap_or_default();
                    Line::from(Span::styled(
                        clip(&format!("          {line:>4} │ {text}"), width),
                        dim,
                    ))
                }
                TestRow::Ran(i) => {
                    let Some(f) = d.ran.as_ref().and_then(|r| r.get(*i)) else {
                        return Line::default();
                    };
                    let lines = if f.lines == 1 { "line" } else { "lines" };
                    let said = format!("  {} {lines}, {} only this test", f.lines, f.only_here);
                    Line::from(vec![
                        Span::raw(clip(
                            &format!(" {}", f.path),
                            width.saturating_sub(said.len()),
                        )),
                        Span::styled(said, dim),
                    ])
                }
                TestRow::Doc(i) => {
                    let doc = &d.docs[*i];
                    let why = if doc.declared {
                        "  named by the test".to_string()
                    } else {
                        format!("  a guess: shares {}", doc.shared.join(", "))
                    };
                    Line::from(vec![
                        Span::raw(format!(" {}", doc.section.heading)),
                        Span::styled(format!(" — {}:{}", doc.section.path, doc.section.line), dim),
                        Span::styled(
                            why,
                            if doc.declared {
                                dim
                            } else {
                                style::fg(theme.moved)
                            },
                        ),
                    ])
                }
                TestRow::Smaller(i) => {
                    let t = &d.smaller[*i];
                    Line::from(vec![
                        Span::raw(clip(&format!(" {}", t.test.test.label), width / 2)),
                        Span::styled(
                            clip(
                                &format!(
                                    " · {} · {}:{} · shares {}",
                                    t.test.level.title().to_lowercase(),
                                    t.test.test.path,
                                    t.test.test.line,
                                    t.shared.join(", ")
                                ),
                                width / 2,
                            ),
                            dim,
                        ),
                    ])
                }
            }
        },
    );
    frame.render_widget(Paragraph::new(lines), area);
}

/// A step as it ran before and as it runs now: both times, both marks, its text, and what
/// differs — the screen at that step, or that it is there on one side only.
fn pair_line(
    theme: &Theme,
    pair: &crate::stretch::StepPair,
    now: bool,
    width: usize,
) -> Line<'static> {
    let dim = style::dim(theme);
    let mark = |s: Option<&crate::recording::Step>| match s {
        None => ("·", dim),
        Some(s) => match (s.kind.as_str(), s.passed) {
            ("check", Some(false)) => ("✗", style::fg(theme.delete_fg).bold()),
            ("check", _) => ("✓", style::fg(theme.insert_fg)),
            _ => ("›", dim),
        },
    };
    let time =
        |s: Option<&crate::recording::Step>| s.map_or("   —  ".to_string(), |s| clock(s.at_ms));
    let (was, was_look) = mark(pair.before.as_ref());
    let (is, is_look) = mark(pair.after.as_ref());
    let step = pair.after.as_ref().or(pair.before.as_ref());
    let text = step.map_or(String::new(), |s| s.text.clone());
    let (note, note_look) = match (&pair.before, &pair.after, pair.same_screen) {
        (None, Some(_), _) => ("  only now", style::fg(theme.insert_fg)),
        (Some(_), None, _) => ("  only before", style::fg(theme.delete_fg)),
        (_, _, Some(false)) => ("  screen changed", style::fg(theme.moved)),
        _ => ("", dim),
    };
    let lead = format!(
        "{}→{} ",
        time(pair.before.as_ref()),
        time(pair.after.as_ref())
    );
    let room = width.saturating_sub(lead.chars().count() + 6 + note.chars().count());
    let text_look = if now {
        style::accent(theme).bold()
    } else if step.is_some_and(|s| s.kind == "check") {
        dim
    } else {
        Style::new()
    };
    Line::from(vec![
        Span::styled(if now { "▶" } else { " " }, style::accent(theme)),
        Span::styled(lead, dim),
        Span::styled(was.to_string(), was_look),
        Span::styled("→", dim),
        Span::styled(format!("{is} "), is_look),
        Span::styled(clip(&text, room), text_look),
        Span::styled(note, note_look),
    ])
}
