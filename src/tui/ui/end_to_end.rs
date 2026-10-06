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
use crate::theme::Theme;
use crate::tui::app::{App, E2eRow, EndToEndState, Screen};
use crate::tui::style;

use super::{list_lines, title_block, wrap_text};

pub(super) fn draw_end_to_end(frame: &mut Frame, app: &mut App, area: Rect) {
    let theme = app.theme.clone();
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
            vec![
                " end to end ".into(),
                format!("{} ", parts.join(" · ")).dim(),
            ]
            .into()
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
        draw_recording(frame, &theme, state, inner, true);
        return;
    }
    let list_width = (inner.width / 3).clamp(24.min(inner.width), 56);
    let [list, gap, detail] = Layout::horizontal([
        Constraint::Length(list_width),
        Constraint::Length(1),
        Constraint::Fill(1),
    ])
    .areas(inner);
    let _ = gap;
    app.measured.list_height = list.height as usize;
    draw_list(frame, &theme, state, list);
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

fn draw_list(frame: &mut Frame, theme: &Theme, state: &mut EndToEndState, area: Rect) {
    let Some(opening) = &state.opening else {
        return;
    };
    state.rows.ensure_visible(area.height as usize);
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
                Line::from(vec![
                    Span::styled(format!(" {mark} "), colour),
                    Span::styled(
                        clip(
                            &format!("{lock}{}{skipped}", t.test.label),
                            width.saturating_sub(3),
                        ),
                        label_style,
                    ),
                ])
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

fn draw_detail(frame: &mut Frame, theme: &Theme, state: &EndToEndState, area: Rect) {
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
        let cast_height = match state.cast() {
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
        draw_recording(frame, theme, state, player, false);
        draw_steps(frame, theme, state, r, steps);
    } else {
        let mut said = vec![Line::default()];
        if let Some(media) = r.media() {
            said.push(Line::from(vec![
                Span::styled(media, Style::new().bold()),
                Span::styled(" — o opens it in the system's player", dim),
            ]));
        } else {
            said.push(Line::from(Span::styled(
                "Its recording is not there any more.",
                style::fg(theme.moved),
            )));
        }
        said.push(Line::default());
        let [note, steps] =
            Layout::vertical([Constraint::Length(said.len() as u16), Constraint::Fill(1)])
                .areas(rest);
        frame.render_widget(Paragraph::new(said), note);
        draw_steps(frame, theme, state, r, steps);
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
fn draw_recording(frame: &mut Frame, theme: &Theme, state: &EndToEndState, area: Rect, full: bool) {
    let position = clock(state.position_ms);
    let total = clock(state.duration_ms());
    let playing = if state.playing.is_some() {
        "▶ playing"
    } else {
        "paused"
    };
    let cast = state.cast();
    let mut title = format!(" {position} / {total} {playing} ");
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
fn draw_steps(frame: &mut Frame, theme: &Theme, state: &EndToEndState, r: &Recording, area: Rect) {
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
