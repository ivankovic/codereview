//! The explorer: the open file, with its comments, across the whole width. Where no file is
//! open, the documents or the datasets are listed to open one; the code lists nothing, since
//! there are too many files to list, and says how to get to one.

use super::*;

pub(super) fn draw_explorer(frame: &mut Frame, app: &mut App, area: Rect) {
    if app.viewer.is_none() && matches!(app.focus, Focus::Docs | Focus::Data) {
        draw_files(frame, app, area);
        return;
    }
    draw_file(frame, app, area);
}

/// The focus's documents or datasets, a dot on every one with a pending comment and its
/// letter if it changed.
fn draw_files(frame: &mut Frame, app: &mut App, area: Rect) {
    let what = match app.focus {
        Focus::Data => "datasets",
        _ => "documents",
    };
    let title: Line = vec![
        format!(" {what} ").into(),
        format!("{} ", app.files.items.len()).dim(),
        "· Enter opens, / searches ".dim(),
    ]
    .into();
    let block = title_block(&app.theme, title, true);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    app.measured.main_height = inner.height as usize;
    if app.files.items.is_empty() {
        let line = Line::from(format!("  No {what} in this repository.").dim());
        frame.render_widget(Paragraph::new(line), inner);
        return;
    }
    let mut pending: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    for c in app.session.review.comments() {
        if c.is_pending() {
            *pending.entry(c.path.as_str()).or_insert(0) += 1;
        }
    }
    app.files.ensure_visible(inner.height as usize);
    let theme = &app.theme;
    let lines: Vec<Line> = app
        .files
        .items
        .iter()
        .enumerate()
        .skip(app.files.scroll)
        .take(inner.height as usize)
        .map(|(i, path)| {
            let status = app.session.status_letter(path);
            let mut spans: Vec<Span> = vec![
                match status {
                    Some(c) => Span::styled(format!(" {c} "), style::fg(theme.update_fg)),
                    None => "   ".into(),
                },
                path.clone().into(),
            ];
            if let Some(n) = pending.get(path.as_str()) {
                spans.push(Span::styled(format!(" ●{n}"), style::fg(theme.exact)));
            }
            let line = Line::from(spans);
            if i == app.files.cursor {
                line.style(style::cursor(theme))
            } else {
                line
            }
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), inner);
}

/// The file under the cursor: its lines, its comments, and the blame column when it is on; or,
/// for a picture, the picture, where the terminal can draw one.
fn draw_file(frame: &mut Frame, app: &mut App, right: Rect) {
    app.ensure_picture();
    let (title, body): (Line, Vec<Line>) = match &mut app.viewer {
        Some(v) => {
            let inner_height = right.height.saturating_sub(2) as usize;
            let inner_width = right.width.saturating_sub(2) as usize;
            app.measured.main_height = inner_height;
            app.measured.main_width = inner_width;
            v.ensure_visible(inner_height);
            let pending = v.comments.iter().filter(|c| c.comment.is_pending()).count();
            let line = v.current_line().map_or(0, |l| l + 1);
            let mut title: Vec<Span> = vec![
                " ".into(),
                v.path.clone().bold(),
                " ".into(),
                if v.total_lines > v.line_count() {
                    format!("{line}/{} of {} ", v.line_count(), v.total_lines).dim()
                } else {
                    format!("{line}/{} ", v.line_count()).dim()
                },
            ];
            if pending > 0 {
                title.push(Span::styled(
                    format!("●{pending} "),
                    style::fg(app.theme.exact),
                ));
            }
            if let Some(status) = app.session.status_letter(&v.path) {
                title.push(format!("[{status}] ").yellow());
            }
            if v.visual.is_some() {
                title.push("VISUAL ".reversed());
            }
            if let Some(s) = &v.search {
                title.push(format!("/{} ({}) ", s.query, s.matches.len()).yellow());
            }
            let body = v.render(&app.theme, inner_width, inner_height, app.highlight, true);
            (title.into(), body)
        }
        None => {
            app.measured.main_height = right.height.saturating_sub(2) as usize;
            let hint = vec![
                "".into(),
                "  / searches for a file, a definition, a test, a heading or any text;"
                    .dim()
                    .into(),
                "  in a file, Enter goes to what the name under the cursor is, J to the rest."
                    .dim()
                    .into(),
                "  o opens a pull request, branch or worktree to review. ? lists every key."
                    .dim()
                    .into(),
                "".into(),
                format!(
                    "  {} files, {} pending comments, {} notes.",
                    app.session.files.len(),
                    app.session.review.pending_count(),
                    app.session.notes.notes().len()
                )
                .dim()
                .into(),
                "".into(),
                "  L  repository log        S  working tree changes"
                    .dim()
                    .into(),
                "  R  review comments       T  notes".dim().into(),
                "  H  history of a file     D  diff a changed file"
                    .dim()
                    .into(),
            ];
            (" codereview ".into(), hint)
        }
    };
    let block = title_block(&app.theme, title, app.viewer.is_some());
    let inner = block.inner(right);
    frame.render_widget(block, right);
    match app.picture.as_mut().map(|p| &mut p.shown) {
        Some(Ok(protocol)) => {
            frame.render_stateful_widget(ratatui_image::StatefulImage::default(), inner, protocol)
        }
        Some(Err(why)) => {
            let line = Line::from(format!(" {why}").dim());
            frame.render_widget(Paragraph::new(line), inner);
        }
        None => frame.render_widget(Paragraph::new(body), inner),
    }
}
