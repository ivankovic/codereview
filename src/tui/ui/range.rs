//! Drawing for the review range: the picker `gp` opens over the screen, and the screen of a
//! pull request or a comparison.

use super::*;
use crate::tui::app::{RangeChoice, RangeRow};

/// A pull request or a comparison: what it is, the files it changes, and its comments.
pub(super) fn draw_range(frame: &mut Frame, app: &mut App, area: Rect) {
    let theme = app.theme.clone();
    let Screen::Range(state) = app.screens.last_mut().expect("screen") else {
        return;
    };
    let title: Line = match &state.pull {
        Some(pr) => vec![
            format!(" pull request #{} ", pr.number).into(),
            format!("{} ", pr.title).bold(),
        ]
        .into(),
        None => vec![" comparison ".into(), format!("{} ", state.label).bold()].into(),
    };
    let block = title_block(&theme, title, true);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let width = inner.width as usize;
    let dim = style::dim(&theme);
    let mut head: Vec<Line> = Vec::new();
    if let Some(pr) = &state.pull {
        let draft = if pr.draft { ", a draft" } else { "" };
        head.push(Line::from(vec![
            Span::styled(format!("{}{draft}", pr.state), style::accent(&theme).bold()),
            Span::styled(format!(" · {} by {}", state.label, pr.author), dim),
        ]));
        head.push(Line::from(Span::styled(
            clip(&format!("{} — o opens it", pr.url), width),
            dim,
        )));
    } else {
        head.push(Line::from(Span::styled(
            clip(
                "Two branches compared as a pull request from one into the other, from where they parted.",
                width,
            ),
            dim,
        )));
    }
    let [top, gap, list] = Layout::vertical([
        Constraint::Length(head.len() as u16),
        Constraint::Length(1),
        Constraint::Fill(1),
    ])
    .areas(inner);
    let _ = gap;
    frame.render_widget(Paragraph::new(head), top);
    app.measured.list_height = list.height as usize;
    state.rows.ensure_visible(list.height as usize);
    let state = &**state;
    let lines = list_lines(
        style::cursor(&theme),
        state.rows.items.iter(),
        state.rows.cursor,
        state.rows.scroll,
        list.height as usize,
        true,
        |row| match row {
            RangeRow::Heading(text) => Line::from(Span::styled(
                clip(text, width),
                style::accent(&theme).bold(),
            )),
            RangeRow::Note(text) => Line::from(Span::styled(clip(&format!(" {text}"), width), dim)),
            RangeRow::File(i) => {
                let f = &state.files[*i];
                vec![
                    Span::styled(format!(" {} ", f.status.letter()), style::accent(&theme)),
                    Span::raw(clip(&f.label()[2..], width.saturating_sub(3))),
                ]
                .into()
            }
            RangeRow::Comment(i) => {
                let c = &state.comments[*i];
                let place = match (&c.path, c.line) {
                    (Some(p), Some(l)) => format!("{p}:{l}"),
                    (Some(p), None) => p.clone(),
                    _ => "the pull request".to_string(),
                };
                let first = c.body.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
                let lead = format!(" {} on {place}: ", c.author);
                vec![
                    Span::styled(lead.clone(), dim),
                    Span::raw(clip(first, width.saturating_sub(lead.chars().count()))),
                ]
                .into()
            }
        },
    );
    frame.render_widget(Paragraph::new(lines), list);
}

/// The range picker over the screen: the stretches, the comparisons and the pull requests.
pub(super) fn draw_range_picker(frame: &mut Frame, app: &mut App, area: Rect) {
    let theme = app.theme.clone();
    let Some(picker) = &mut app.range_picker else {
        return;
    };
    let width = area.width.min(90);
    let height = area.height.min(picker.rows.items.len() as u16 + 2).max(5);
    let popup = Rect {
        x: (area.width - width) / 2,
        y: (area.height - height) / 2,
        width,
        height,
    };
    frame.render_widget(Clear, popup);
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" review range (Enter picks, Esc closes) ")
        .border_style(style::accent(&theme));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);
    let inner_width = inner.width as usize;
    picker.rows.ensure_visible(inner.height as usize);
    let dim = style::dim(&theme);
    let lines = list_lines(
        style::cursor(&theme),
        picker.rows.items.iter(),
        picker.rows.cursor,
        picker.rows.scroll,
        inner.height as usize,
        true,
        |row| match row {
            RangeChoice::Heading(text) => Line::from(Span::styled(
                clip(text, inner_width),
                style::accent(&theme).bold(),
            )),
            RangeChoice::Note(text) => {
                Line::from(Span::styled(clip(&format!("  {text}"), inner_width), dim))
            }
            RangeChoice::Stretch(s) => Line::from(clip(&format!("  {}", s.name()), inner_width)),
            RangeChoice::Compare { base, head } => Line::from(vec![
                Span::raw(clip(&format!("  {head}"), inner_width / 2)),
                Span::styled(format!(" against {base}"), dim),
            ]),
            RangeChoice::Pull(pr) => {
                let lead = format!("  #{} ", pr.number);
                let tail = format!(
                    " — {}{}",
                    pr.author,
                    if pr.draft { ", a draft" } else { "" }
                );
                let room = inner_width.saturating_sub(lead.len() + tail.chars().count());
                Line::from(vec![
                    Span::styled(lead, style::accent(&theme)),
                    Span::raw(clip(&pr.title, room)),
                    Span::styled(tail, dim),
                ])
            }
        },
    );
    frame.render_widget(Paragraph::new(lines), inner);
}

fn clip(text: &str, width: usize) -> String {
    if text.chars().count() <= width {
        return text.to_string();
    }
    let mut out: String = text.chars().take(width.saturating_sub(1)).collect();
    out.push('…');
    out
}
