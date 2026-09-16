//! The explorer: the file tree on the left, the file with its comments on the right.

use super::*;

/// The explorer is two panes: the tree of files, and the file the cursor is on with its
/// comments woven in.
pub(super) fn draw_explorer(frame: &mut Frame, app: &mut App, area: Rect) {
    let tree_width = (area.width / 4).clamp(24, 50);
    let [left, right] =
        Layout::horizontal([Constraint::Length(tree_width), Constraint::Fill(1)]).areas(area);
    draw_tree(frame, app, left);
    draw_file(frame, app, right);
}

/// The file tree, or what a filter leaves of it, with a dot on every path holding a pending
/// comment.
fn draw_tree(frame: &mut Frame, app: &mut App, left: Rect) {
    let title: Line = if app.tree.filter.is_empty() {
        vec![
            " files ".into(),
            format!("{} ", app.session.files.len()).dim(),
        ]
        .into()
    } else {
        vec![
            " filter: ".into(),
            app.tree.filter.clone().yellow(),
            " ".into(),
        ]
        .into()
    };
    let block = title_block(&app.theme, title, app.focus_tree);
    let inner = block.inner(left);
    frame.render_widget(block, left);
    app.measured.tree_height = inner.height as usize;
    app.tree.ensure_visible(inner.height as usize);
    let mut lines = Vec::with_capacity(inner.height as usize);
    let comment_paths: std::collections::HashMap<&str, usize> = {
        let mut m = std::collections::HashMap::new();
        for c in app.session.review.comments() {
            if c.is_pending() {
                *m.entry(c.path.as_str()).or_insert(0) += 1;
            }
        }
        m
    };
    for i in app.tree.scroll..(app.tree.scroll + inner.height as usize) {
        let Some(node) = app.tree.row(i) else {
            break;
        };
        let indent = "  ".repeat(node.depth);
        let mut spans: Vec<Span> = Vec::new();
        let status = if node.is_dir {
            None
        } else {
            app.session.status_letter(&node.path)
        };
        let marker = match status {
            Some('?') => Span::styled("?", style::dim(&app.theme)),
            Some('A') => Span::styled("A", style::fg(app.theme.insert_fg)),
            Some('D') => Span::styled("D", style::fg(app.theme.delete_fg)),
            Some(c) => Span::styled(c.to_string(), style::fg(app.theme.update_fg)),
            None => " ".into(),
        };
        spans.push(marker);
        spans.push(indent.into());
        if node.is_dir {
            let arrow = if app.tree.is_collapsed(&node.path) {
                "▸ "
            } else {
                "▾ "
            };
            spans.push(arrow.dim());
            spans.push(format!("{}/", node.name).bold());
            if let Some(n) = comment_paths.get(node.path.as_str()) {
                spans.push(Span::styled(format!(" ●{n}"), style::fg(app.theme.exact)));
            }
        } else {
            spans.push("  ".into());
            let name = node.name.clone();
            spans.push(match status {
                Some('?') => Span::styled(name, style::dim(&app.theme)),
                Some(_) => Span::styled(name, style::fg(app.theme.update_fg)),
                None => name.into(),
            });
            if let Some(n) = comment_paths.get(node.path.as_str()) {
                spans.push(Span::styled(format!(" ●{n}"), style::fg(app.theme.exact)));
            }
        }
        let mut line = Line::from(spans);
        if i == app.tree.cursor && app.focus_tree {
            line = line.style(style::cursor(&app.theme));
        }
        lines.push(line);
    }
    frame.render_widget(Paragraph::new(lines), inner);
}

/// The file under the cursor: its lines, its comments, and the blame column when it is on.
fn draw_file(frame: &mut Frame, app: &mut App, right: Rect) {
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
                format!("{line}/{} ", v.line_count()).dim(),
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
            let body = v.render(
                &app.theme,
                inner_width,
                inner_height,
                app.highlight,
                !app.focus_tree,
            );
            (title.into(), body)
        }
        None => {
            app.measured.main_height = right.height.saturating_sub(2) as usize;
            let hint = vec![
                "".into(),
                "  Enter opens the selected file. ? lists every key."
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
    let block = title_block(&app.theme, title, !app.focus_tree && app.viewer.is_some());
    let inner = block.inner(right);
    frame.render_widget(block, right);
    frame.render_widget(Paragraph::new(body), inner);
}
