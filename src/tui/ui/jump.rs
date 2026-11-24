//! Drawing for the jump menu, `J`: where the cursor is, and the places to go from there, a
//! group a heading with its letter.

use super::*;
use crate::tui::app::{FindRow, JumpRow};

pub(super) fn draw_jump_menu(frame: &mut Frame, app: &mut App, area: Rect) {
    let theme = app.theme.clone();
    let Some(menu) = &mut app.jump_menu else {
        return;
    };
    let width = area.width.min(110);
    let height = area.height.min(menu.rows.items.len() as u16 + 2).max(5);
    let popup = Rect {
        x: area.x + (area.width - width) / 2,
        y: area.y + (area.height - height) / 2,
        width,
        height,
    };
    frame.render_widget(Clear, popup);
    let what = match (&menu.here.name, menu.here.defines) {
        (Some(name), true) => format!("{name}, defined here"),
        (Some(name), false) => format!("{name}, used here"),
        (None, _) => "this line".to_string(),
    };
    // A definition is not said to be in itself.
    let within = menu
        .here
        .within
        .as_ref()
        .filter(|w| {
            !(menu.here.defines
                && menu
                    .here
                    .name
                    .as_ref()
                    .is_some_and(|n| w.ends_with(&format!(" {n}"))))
        })
        .map(|w| format!(" in {w}"))
        .unwrap_or_default();
    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!(
            " from {what}{within} · a letter to its group, Enter goes, Esc closes "
        ))
        .border_style(style::accent(&theme));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);
    let width = inner.width as usize;
    menu.rows.ensure_visible(inner.height as usize);
    let dim = style::dim(&theme);
    let menu = &*menu;
    let lines = list_lines(
        style::cursor(&theme),
        menu.rows.items.iter(),
        menu.rows.cursor,
        menu.rows.scroll,
        inner.height as usize,
        true,
        |row| match *row {
            JumpRow::Heading(kind) => {
                let n = menu.here.group(kind).map_or(0, |g| g.places.len());
                Line::from(vec![
                    Span::styled(
                        format!("{} {n}", kind.title()),
                        style::accent(&theme).bold(),
                    ),
                    Span::styled(format!("  ({})", kind.key()), dim),
                ])
            }
            JumpRow::Place(..) => {
                let Some(p) = menu.place(*row) else {
                    return Line::default();
                };
                let lead = format!("  {}:{}  ", p.path, p.line);
                let note = p
                    .note
                    .as_ref()
                    .map(|n| format!("  — {n}"))
                    .unwrap_or_default();
                let room = width.saturating_sub(lead.chars().count() + note.chars().count());
                let text: String = p.text.chars().take(room).collect();
                Line::from(vec![
                    Span::styled(lead, style::accent(&theme)),
                    Span::raw(text),
                    Span::styled(note, dim),
                ])
            }
        },
    );
    frame.render_widget(Paragraph::new(lines), inner);
}

/// The search, `/`: what is typed at the top, the hits under it in groups, the best first.
pub(super) fn draw_finder(frame: &mut Frame, app: &mut App, area: Rect) {
    let theme = app.theme.clone();
    let Some(f) = &mut app.finder else {
        return;
    };
    let width = area.width.min(120);
    let height = (area.height * 4 / 5).max(area.height.min(8));
    let popup = Rect {
        x: area.x + (area.width - width) / 2,
        y: area.y + (area.height - height) / 2,
        width,
        height,
    };
    frame.render_widget(Clear, popup);
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" search · Tab next group, Enter goes, Esc closes ")
        .title_bottom(
            " f: files  s: symbols  t: tests  d: docs  c: comments  \"…\" exact  /…/ regex ",
        )
        .border_style(style::accent(&theme));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);
    if inner.height < 2 {
        return;
    }
    let dim = style::dim(&theme);
    let status = if f.input.text.trim().is_empty() {
        "a name, a path, a test, a heading, or any text".to_string()
    } else if f.searching_text() {
        "searching the text…".to_string()
    } else {
        let n: usize = f.groups.iter().map(|(_, h)| h.len()).sum();
        format!("{n} found")
    };
    let input = Line::from(vec![
        Span::styled("/ ", style::accent(&theme).bold()),
        Span::raw(f.input.text.clone()),
        Span::styled("▏", style::accent(&theme)),
        Span::styled(format!("   {status}"), dim),
    ]);
    frame.render_widget(Paragraph::new(input), Rect { height: 1, ..inner });
    let list = Rect {
        y: inner.y + 1,
        height: inner.height - 1,
        ..inner
    };
    let width = list.width as usize;
    f.rows.ensure_visible(list.height as usize);
    let f = &*f;
    let lines = list_lines(
        style::cursor(&theme),
        f.rows.items.iter(),
        f.rows.cursor,
        f.rows.scroll,
        list.height as usize,
        true,
        |row| match *row {
            FindRow::Heading(group) => {
                let n = f
                    .groups
                    .iter()
                    .find(|(g, _)| *g == group)
                    .map_or(0, |(_, h)| h.len());
                Line::from(Span::styled(
                    format!("{} {n}", group.title()),
                    style::accent(&theme).bold(),
                ))
            }
            FindRow::Hit(..) => {
                let Some(hit) = f.hit(*row) else {
                    return Line::default();
                };
                let lead = match hit.group {
                    crate::search::Group::InFile => format!("  {:>5}  ", hit.line),
                    _ => "  ".to_string(),
                };
                let note = hit
                    .note
                    .as_ref()
                    .map(|n| format!("  {n}"))
                    .unwrap_or_default();
                let room = width.saturating_sub(lead.chars().count() + note.chars().count());
                let text: String = hit.text.chars().take(room).collect();
                Line::from(vec![
                    Span::styled(lead, dim),
                    Span::raw(text),
                    Span::styled(note, dim),
                ])
            }
        },
    );
    frame.render_widget(Paragraph::new(lines), list);
}
