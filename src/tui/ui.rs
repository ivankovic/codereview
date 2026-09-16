//! Drawing. Reads the app, writes to the frame, and records the sizes key handling needs.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};
use unicode_width::UnicodeWidthStr;

use crate::anchor::AnchorState;
use crate::session::DiffTarget;
use crate::theme::Theme;
use crate::tui::agent_panel;
use crate::tui::app::{App, Screen};
use crate::tui::style::{self, comment_style};
use crate::tui::workspace::Workspace;

mod explorer;
mod screens;

use explorer::draw_explorer;
use screens::{draw_changes, draw_diff, draw_locations, draw_log, draw_notes, draw_review};

/// Every open repository: the strip naming them when there is more than one, the active
/// app below it, and the picker on top.
pub fn draw_workspace(frame: &mut Frame, ws: &mut Workspace) {
    let area = frame.area();
    let body = if ws.apps.len() > 1 {
        let [strip, body] =
            Layout::vertical([Constraint::Length(1), Constraint::Fill(1)]).areas(area);
        draw_repo_strip(frame, ws, strip);
        body
    } else {
        area
    };
    let active = ws.active;
    draw_app(frame, &mut ws.apps[active], body);
    if ws.picker.is_some() {
        draw_repo_picker(frame, ws, area);
    }
}

/// ` 1 codereview │ 2 codediff ⠋ │ 3 nvim-review ?3 `: number, name, and what is going on
/// there: `?` while its agent waits for permission, a spinner while it works, the pending
/// comment count.
fn draw_repo_strip(frame: &mut Frame, ws: &Workspace, area: Rect) {
    let theme = ws.app().theme.clone();
    let mut labels: Vec<(String, Style)> = Vec::new();
    for (i, app) in ws.apps.iter().enumerate() {
        let mut label = format!(" {} {}", i + 1, app.session.name());
        if app.agent.permission().is_some() {
            label.push_str(" ?");
        } else if app.agent_active() {
            label.push(' ');
            label.push_str(app.agent.spinner());
        }
        let pending = app.session.review.pending_count();
        if pending > 0 {
            label.push_str(&format!(" ·{pending}"));
        }
        label.push(' ');
        let style_ = if i == ws.active {
            style::accent(&theme).reversed()
        } else if app.agent.permission().is_some() {
            style::fg(theme.moved).bold()
        } else {
            style::dim(&theme)
        };
        labels.push((label, style_));
    }
    // More repositories than fit: start from the first that still leaves the active one
    // visible, and mark what is cut off on the left.
    let hint = " gt/gT next/previous  W pick";
    let room = (area.width as usize).saturating_sub(hint.len() + 2);
    let mut start = ws.active;
    let mut used = labels[ws.active].0.width() + 1;
    while start > 0 && used + labels[start - 1].0.width() < room {
        start -= 1;
        used += labels[start].0.width() + 1;
    }
    let mut spans: Vec<Span> = Vec::new();
    if start > 0 {
        spans.push(Span::styled("‹", style::dim(&theme)));
    }
    for (label, style_) in labels.into_iter().skip(start) {
        spans.push(Span::styled(label, style_));
        spans.push(Span::styled("│", style::dim(&theme)));
    }
    spans.push(Span::styled(hint, style::dim(&theme)));
    frame.render_widget(
        Paragraph::new(Line::from(spans)).style(style::fg(theme.fg).patch(style::bg(theme.panel))),
        area,
    );
}

fn draw_repo_picker(frame: &mut Frame, ws: &Workspace, area: Rect) {
    let Some(index) = ws.picker else {
        return;
    };
    let theme = ws.app().theme.clone();
    let height = (ws.apps.len() as u16 + 3).min(area.height);
    let width = (area.width * 2 / 3).clamp(30, 90).min(area.width);
    let rect = Rect {
        x: (area.width - width) / 2,
        y: (area.height.saturating_sub(height)) / 2,
        width,
        height,
    };
    frame.render_widget(Clear, rect);
    let block = title_block(
        &theme,
        " repositories  Enter switch  1-9 direct  Esc close ".into(),
        true,
    )
    .style(style::fg(theme.fg).patch(style::bg(theme.panel)));
    let inner = block.inner(rect);
    frame.render_widget(block, rect);
    // Keep the highlighted row inside the box when there are more rows than lines.
    let rows = inner.height as usize;
    let offset = index.saturating_sub(rows.saturating_sub(1));
    let lines: Vec<Line> = ws
        .apps
        .iter()
        .enumerate()
        .skip(offset)
        .take(rows)
        .map(|(i, app)| {
            let pending = app.session.review.pending_count();
            let agent = if app.agent.permission().is_some() {
                "agent waiting for permission"
            } else if app.agent_active() {
                "agent working"
            } else {
                ""
            };
            let text = format!(
                "{} {:<24} {:<40} {} pending  {}",
                i + 1,
                app.session.name(),
                app.session.root().display(),
                pending,
                agent
            );
            let mut style_ = Style::new();
            if i == index {
                style_ = style::accent(&theme).reversed();
            } else if i == ws.active {
                style_ = style_.bold();
            }
            Line::from(Span::styled(text, style_))
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), inner);
}

pub fn draw(frame: &mut Frame, app: &mut App) {
    let area = frame.area();
    draw_app(frame, app, area);
}

/// One repository's app in `area`.
pub fn draw_app(frame: &mut Frame, app: &mut App, area: Rect) {
    let [main, status] = Layout::vertical([Constraint::Fill(1), Constraint::Length(1)]).areas(area);
    match app.screens.last_mut().expect("screen") {
        Screen::Explorer => draw_explorer(frame, app, main),
        Screen::Log(_) => draw_log(frame, app, main),
        Screen::Changes(_) => draw_changes(frame, app, main),
        Screen::Diff(_) => draw_diff(frame, app, main),
        Screen::Review(_) => draw_review(frame, app, main),
        Screen::Notes(_) => draw_notes(frame, app, main),
        Screen::Locations(_) => draw_locations(frame, app, main),
        Screen::Agent => {
            let theme = app.theme.clone();
            let label = app.agent_label();
            let name = app.agent.name().unwrap_or_else(|| label.clone());
            let (h, w) = agent_panel::draw(frame, &mut app.agent, &theme, &label, &name, main);
            app.measured.main_height = h;
            app.measured.main_width = w;
        }
    }
    draw_status(frame, app, status);
    if app.show_help {
        draw_help(frame, app, area);
    }
    if app.theme_picker.is_some() {
        draw_theme_picker(frame, app, area);
    }
}

pub(crate) fn title_block(theme: &Theme, title: Line<'static>, focused: bool) -> Block<'static> {
    let block = Block::default().borders(Borders::ALL).title(title);
    if focused {
        block.border_style(style::accent(theme))
    } else {
        block.border_style(style::fg(theme.border).patch(style::dim(theme)))
    }
}
pub(crate) fn list_lines<'a, T>(
    cursor_style: Style,
    items: impl Iterator<Item = &'a T>,
    cursor: usize,
    scroll: usize,
    height: usize,
    focused: bool,
    render: impl Fn(&T) -> Line<'static>,
) -> Vec<Line<'static>>
where
    T: 'a,
{
    items
        .enumerate()
        .skip(scroll)
        .take(height)
        .map(|(i, item)| {
            let line = render(item);
            if i == cursor && focused {
                line.style(cursor_style)
            } else {
                line
            }
        })
        .collect()
}

/// Wraps `text` to `width` cells per line, breaking at spaces where possible.
pub(crate) fn wrap_text(text: &str, width: usize) -> Vec<String> {
    let width = width.max(8);
    let mut out = Vec::new();
    for raw in text.split('\n') {
        let mut line = String::new();
        let mut line_width = 0;
        for word in raw.split(' ') {
            let w = word.width();
            if line_width > 0 && line_width + 1 + w > width {
                out.push(std::mem::take(&mut line));
                line_width = 0;
            }
            if w > width {
                // A single long token: hard-break it.
                for ch in word.chars() {
                    let cw = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
                    if line_width + cw > width {
                        out.push(std::mem::take(&mut line));
                        line_width = 0;
                    }
                    line.push(ch);
                    line_width += cw;
                }
                continue;
            }
            if line_width > 0 {
                line.push(' ');
                line_width += 1;
            }
            line.push_str(word);
            line_width += w;
        }
        out.push(line);
    }
    out
}

fn draw_status(frame: &mut Frame, app: &mut App, area: Rect) {
    if let Some(prompt) = &app.prompt {
        prompt.draw(frame, area);
        return;
    }
    let mut spans: Vec<Span> = vec![format!(" {} ", app.screen().name()).reversed()];
    let elsewhere = !matches!(app.screen(), Screen::Agent);
    // What just happened comes first: a message is cleared by the next key, and burying an
    // error under the agent's progress for the length of a turn loses it entirely.
    if let Some((text, is_error)) = &app.message {
        spans.push(" ".into());
        spans.push(if *is_error {
            text.clone().red()
        } else {
            text.clone().into()
        });
        if app.agent.permission().is_some() && elsewhere {
            spans.push("  agent is waiting for permission (A)".into());
        }
    } else if app.agent.permission().is_some() && elsewhere {
        spans.push(" ".into());
        spans.push(Span::styled(
            "agent is waiting for permission (A)",
            style::fg(app.theme.moved).bold(),
        ));
    } else if app.agent_active() && elsewhere {
        spans.push(" ".into());
        spans.push(Span::styled(
            format!(
                "agent: {} (A)",
                app.agent.progress().unwrap_or_else(|| "working".into())
            ),
            style::dim(&app.theme),
        ));
    } else {
        let context: Option<String> = match app.screen() {
            Screen::Diff(d) => {
                // The comment under the cursor, if any.
                d.view
                    .after_line()
                    .and_then(|line| d.view.comments.iter().find(|c| c.covers_row(line)))
                    .map(|c| format!("● {}", c.comment.text))
            }
            Screen::Explorer => app.viewer.as_ref().and_then(|v| {
                let line = v.current_line()?;
                let op = v.line_ops.as_ref()?.get(line).copied()?;
                op.is_change()
                    .then(|| format!("{} vs HEAD", crate::tui::style::op_marker(op)))
            }),
            _ => None,
        };
        spans.push(" ".into());
        match context {
            Some(c) => spans.push(c.cyan()),
            None => spans.push(hint_for(app.screen()).dim()),
        }
    }
    let right = format!(
        " {} · {} pending · ? help ",
        app.session.repo.head_label(),
        app.session.review.pending_count()
    );
    let left_width: usize = spans.iter().map(|s| s.width()).sum();
    let pad = (area.width as usize).saturating_sub(left_width + right.width());
    spans.push(" ".repeat(pad).into());
    spans.push(right.dim());
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn hint_for(screen: &Screen) -> &'static str {
    match screen {
        Screen::Explorer => {
            "c comment  gd def  gr uses  @ symbols  a/i ask agent  A agent panel  H history  D diff  / search"
        }
        Screen::Log(_) => "Enter files/diff  Tab switch  w vs working tree  H file history  q back",
        Screen::Changes(_) => "Enter diff  s staged/working  H history  o open  q back",
        Screen::Diff(_) => "n/p hunks  ]/[ files  c comment  v layout  o open in explorer  q back",
        Screen::Review(_) => {
            "Enter go to  x done  e edit  d delete  a/A ask agent  s completed  Z re-anchor  q back"
        }
        Screen::Notes(_) => "Enter go to  n add  e edit  d delete  q back",
        Screen::Locations(_) => "Enter go there  Ctrl-o back to where you were  q close",
        Screen::Agent => {
            "i ask  A address all pending  Esc cancel  1-9/y/n answer  t thoughts  l log  C clear  R restart  q back"
        }
    }
}

const HELP: &[(&str, &str)] = &[
    ("Everywhere", ""),
    ("?", "this help"),
    ("q / Esc", "back, or quit from the explorer"),
    ("L", "repository log"),
    ("S", "working tree changes (s toggles staged)"),
    ("R", "review: every comment in REVIEW.md"),
    ("T", "notes: every note in NOTES.md"),
    ("N", "add a general note"),
    ("r", "re-read the repository and both Markdown files"),
    ("t", "colour scheme picker; the choice is saved"),
    ("Ctrl-h", "toggle syntax highlighting"),
    ("A  Ctrl-a", "the agent panel"),
    ("i", "ask the agent something, from anywhere"),
    (
        "gt / gT",
        "next / previous repository, when several are open",
    ),
    ("W", "repository picker"),
    ("", ""),
    ("Explorer", ""),
    ("Tab", "switch between the tree and the file"),
    ("j/k h/l", "move; in the tree h/l fold and unfold"),
    ("Enter", "open the file; on a comment row, edit it"),
    ("/", "filter the tree; search the file"),
    ("> <", "next / previous search match"),
    (":", "go to line"),
    ("z / Z", "fold / unfold every directory"),
    (
        "c",
        "comment on the line (or the V selection); in the tree, on the whole file or directory",
    ),
    ("V", "start / end a line selection"),
    ("x", "mark the comment done / pending"),
    ("e", "edit the comment"),
    ("d / u", "delete the comment / undo the delete"),
    ("] [", "next / previous comment"),
    ("} {", "next / previous change against HEAD"),
    ("n", "note on this file"),
    ("H", "history of this file"),
    ("D", "diff this file against HEAD"),
    ("B", "blame column"),
    ("o", "open in $EDITOR at the line"),
    (
        "h l w b 0 ^ $",
        "move the column cursor: character, word, line start, end",
    ),
    (
        "gd  Ctrl-]",
        "go to the definition of the identifier under the cursor",
    ),
    (
        "gr  *",
        "every occurrence of the identifier under the cursor",
    ),
    ("gs  @", "symbols defined in this file"),
    ("gS  #", "search symbols across the repository"),
    ("Ctrl-o", "back to where you were before a jump"),
    (
        "a",
        "ask the agent about the line, the selection, or the comment",
    ),
    ("", ""),
    ("Agent panel", ""),
    ("i", "ask something"),
    ("A", "address every pending comment in REVIEW.md"),
    ("1-9 y n", "answer a permission request"),
    ("Esc", "cancel the running turn"),
    ("t l", "show thoughts / the backend's log lines"),
    ("C R", "clear the transcript, restart the agent"),
    ("", ""),
    ("Log and history", ""),
    ("Enter", "files of the commit, then the diff of a file"),
    ("w", "the file at this commit against the working tree"),
    ("H", "history of the selected file"),
    ("", ""),
    ("Diff", ""),
    ("n / p", "next / previous hunk"),
    ("] [", "next / previous file of the same commit"),
    ("v", "layout: auto, side by side, unified"),
    (
        "c x e D",
        "comment on the after line; done, edit, delete it",
    ),
    ("o", "open the after line in the explorer"),
    ("", ""),
    ("Colours", ""),
    ("green", "inserted"),
    ("red", "deleted"),
    ("yellow", "updated"),
    ("grey", "moved"),
    (
        "cyan ●",
        "comment; yellow when it moved, red when its line is gone",
    ),
];

fn draw_theme_picker(frame: &mut Frame, app: &App, area: Rect) {
    let Some((index, _)) = &app.theme_picker else {
        return;
    };
    let themes = crate::theme::all();
    let width = area.width.min(64);
    let height = area.height.min(themes.len() as u16 + 6);
    let popup = Rect {
        x: (area.width - width) / 2,
        y: (area.height - height) / 2,
        width,
        height,
    };
    frame.render_widget(Clear, popup);
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" colour scheme (j/k preview, Enter keep, Esc cancel) ")
        .border_style(style::accent(&app.theme));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);
    let [list, legend_area] =
        Layout::vertical([Constraint::Fill(1), Constraint::Length(3)]).areas(inner);
    let theme = &app.theme;
    let mut lines: Vec<Line> = themes
        .iter()
        .enumerate()
        .map(|(i, t)| {
            let kind = if t.terminal {
                "your terminal's palette"
            } else if t.dark {
                "dark"
            } else {
                "light"
            };
            let line: Line = vec![
                format!(" {:<22}", t.name).into(),
                Span::styled(kind.to_string(), style::dim(theme)),
            ]
            .into();
            if i == *index {
                line.style(style::cursor(theme))
            } else {
                line
            }
        })
        .collect();
    let skip = index.saturating_sub(list.height.saturating_sub(1) as usize);
    lines.drain(..skip.min(lines.len()));
    frame.render_widget(Paragraph::new(lines), list);
    // A legend in the previewed colours, so the choice can be judged without leaving.
    let sample = |op: crate::diff::Op, text: &str| -> Span<'static> {
        let seg = crate::highlight::Segment::plain(text);
        style::change_span(&seg, op, theme, false, style::color(theme.line_bg(op)))
    };
    let legend_lines: Vec<Line> = vec![
        "".into(),
        vec![
            " ".into(),
            sample(crate::diff::Op::Insert, " inserted "),
            " ".into(),
            sample(crate::diff::Op::Delete, " deleted "),
            " ".into(),
            sample(crate::diff::Op::Update, " updated "),
            " ".into(),
            sample(crate::diff::Op::Move, " moved "),
            " ".into(),
            Span::styled("● exact", style::fg(theme.exact)),
            " ".into(),
            Span::styled("● moved", style::fg(theme.moved)),
            " ".into(),
            Span::styled("● stale", style::fg(theme.stale)),
        ]
        .into(),
    ];
    frame.render_widget(Paragraph::new(legend_lines), legend_area);
}

fn draw_help(frame: &mut Frame, app: &App, area: Rect) {
    let width = area.width.min(70);
    let height = area.height.min(HELP.len() as u16 + 2);
    let popup = Rect {
        x: (area.width - width) / 2,
        y: (area.height - height) / 2,
        width,
        height,
    };
    frame.render_widget(Clear, popup);
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" keys (j/k scroll, any other key closes) ")
        .border_style(style::accent(&app.theme));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);
    let lines: Vec<Line> = HELP
        .iter()
        .skip(app.help_scroll)
        .take(inner.height as usize)
        .map(|(k, v)| {
            if v.is_empty() {
                Line::from(k.to_string().bold().cyan())
            } else {
                vec![format!("  {k:<10}").yellow(), v.to_string().into()].into()
            }
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), inner);
}
