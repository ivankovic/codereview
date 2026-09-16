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
use crate::tui::app::{App, ChangesState, EntryKind, LogState, NotesState, ReviewState, Screen};
use crate::tui::style::{self, comment_style};
use crate::tui::workspace::Workspace;

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
        if app.agent_state.permission.is_some() {
            label.push_str(" ?");
        } else if app.agent_active() {
            let frame_ = app
                .agent_state
                .turn_started
                .or(app.agent_state.start_began)
                .map(|t| t.elapsed().as_millis() / 80)
                .unwrap_or(0) as usize
                % SPINNER.len();
            label.push(' ');
            label.push_str(SPINNER[frame_]);
        }
        let pending = app.session.review.pending_count();
        if pending > 0 {
            label.push_str(&format!(" ·{pending}"));
        }
        label.push(' ');
        let style_ = if i == ws.active {
            style::accent(&theme).reversed()
        } else if app.agent_state.permission.is_some() {
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
            let agent = if app.agent_state.permission.is_some() {
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
        Screen::Agent => draw_agent(frame, app, main),
    }
    draw_status(frame, app, status);
    if app.show_help {
        draw_help(frame, app, area);
    }
    if app.theme_picker.is_some() {
        draw_theme_picker(frame, app, area);
    }
}

fn title_block(theme: &Theme, title: Line<'static>, focused: bool) -> Block<'static> {
    let block = Block::default().borders(Borders::ALL).title(title);
    if focused {
        block.border_style(style::accent(theme))
    } else {
        block.border_style(style::fg(theme.border).patch(style::dim(theme)))
    }
}

fn draw_explorer(frame: &mut Frame, app: &mut App, area: Rect) {
    let tree_width = (area.width / 4).clamp(24, 50);
    let [left, right] =
        Layout::horizontal([Constraint::Length(tree_width), Constraint::Fill(1)]).areas(area);

    // Tree.
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

    // Viewer.
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

fn list_lines<'a, T>(
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

fn draw_log(frame: &mut Frame, app: &mut App, area: Rect) {
    let Screen::Log(state) = app.screens.last_mut().expect("screen") else {
        return;
    };
    let [left, right] =
        Layout::horizontal([Constraint::Percentage(50), Constraint::Fill(1)]).areas(area);
    // Commits.
    let title: Line = match &state.path {
        Some(p) => vec![
            " history of ".into(),
            p.clone().bold(),
            format!(
                " {} commits{} ",
                state.commits.items.len(),
                if state.exhausted { "" } else { "+" }
            )
            .dim(),
        ]
        .into(),
        None => vec![
            " log ".into(),
            app.session.repo.head_label().bold(),
            format!(
                " {} commits{} ",
                state.commits.items.len(),
                if state.exhausted { "" } else { "+" }
            )
            .dim(),
        ]
        .into(),
    };
    let block = title_block(&app.theme, title, !state.focus_files);
    let inner = block.inner(left);
    frame.render_widget(block, left);
    app.measured.list_height = inner.height as usize;
    state.commits.ensure_visible(inner.height as usize);
    let width = inner.width as usize;
    let lines = list_lines(
        style::cursor(&app.theme),
        state.commits.items.iter(),
        state.commits.cursor,
        state.commits.scroll,
        inner.height as usize,
        !state.focus_files,
        |c| {
            let author: String = c.author.chars().take(12).collect();
            let head = format!("{} {} {:<12} ", c.short, c.day(), author);
            let avail = width.saturating_sub(head.width());
            let subject: String = c.subject.chars().take(avail).collect();
            vec![head.dim(), subject.into()].into()
        },
    );
    frame.render_widget(Paragraph::new(lines), inner);

    // Details and files.
    let [details, files] =
        Layout::vertical([Constraint::Length(8), Constraint::Fill(1)]).areas(right);
    let mut detail_lines: Vec<Line> = Vec::new();
    if let Some(c) = state.commits.current() {
        detail_lines.push(vec!["commit  ".dim(), c.hash.clone().yellow()].into());
        detail_lines.push(
            vec![
                "author  ".dim(),
                format!("{} <{}>", c.author, c.email).into(),
            ]
            .into(),
        );
        detail_lines.push(vec!["date    ".dim(), c.date.clone().into()].into());
        detail_lines.push("".into());
        detail_lines.push(c.subject.clone().bold().into());
        for l in c.body.lines().take(2) {
            detail_lines.push(l.to_string().into());
        }
    }
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::new().dim());
    let inner = block.inner(details);
    frame.render_widget(block, details);
    frame.render_widget(
        Paragraph::new(detail_lines).wrap(Wrap { trim: false }),
        inner,
    );

    let block = title_block(
        &app.theme,
        vec![
            " files ".into(),
            format!("{} ", state.files.items.len()).dim(),
        ]
        .into(),
        state.focus_files,
    );
    let inner = block.inner(files);
    frame.render_widget(block, files);
    state.files.ensure_visible(inner.height as usize);
    let lines = list_lines(
        style::cursor(&app.theme),
        state.files.items.iter(),
        state.files.cursor,
        state.files.scroll,
        inner.height as usize,
        state.focus_files,
        |f| {
            let color = match f.status.letter() {
                'A' => Color::Green,
                'D' => Color::Red,
                'R' | 'C' => Color::Magenta,
                _ => Color::Yellow,
            };
            vec![
                Span::styled(format!("{} ", f.status.letter()), Style::new().fg(color)),
                f.label()[2..].to_string().into(),
            ]
            .into()
        },
    );
    frame.render_widget(Paragraph::new(lines), inner);
    let _ = LogState::name_hint;
}

impl LogState {
    #[allow(dead_code)]
    fn name_hint() {}
}

fn draw_changes(frame: &mut Frame, app: &mut App, area: Rect) {
    let Screen::Changes(state) = app.screens.last_mut().expect("screen") else {
        return;
    };
    let title: Line = vec![
        " ".into(),
        (if state.target == DiffTarget::Staged {
            "staged"
        } else {
            "working tree"
        })
        .bold(),
        format!(" {} files ", state.files.items.len()).dim(),
        "(s switches) ".dim(),
    ]
    .into();
    let block = title_block(&app.theme, title, true);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    app.measured.list_height = inner.height as usize;
    state.files.ensure_visible(inner.height as usize);
    let lines = list_lines(
        style::cursor(&app.theme),
        state.files.items.iter(),
        state.files.cursor,
        state.files.scroll,
        inner.height as usize,
        true,
        |f| {
            let color = match f.status.letter() {
                'A' | '?' => Color::Green,
                'D' => Color::Red,
                'R' | 'C' => Color::Magenta,
                _ => Color::Yellow,
            };
            vec![
                Span::styled(format!(" {} ", f.status.letter()), Style::new().fg(color)),
                f.label()[2..].to_string().into(),
            ]
            .into()
        },
    );
    frame.render_widget(Paragraph::new(lines), inner);
    let _ = ChangesState::hint;
}

impl ChangesState {
    #[allow(dead_code)]
    fn hint() {}
}

fn draw_diff(frame: &mut Frame, app: &mut App, area: Rect) {
    let highlight = app.highlight;
    let layout = app.layout;
    let theme = app.theme.clone();
    let Screen::Diff(d) = app.screens.last_mut().expect("screen") else {
        return;
    };
    let v = &mut d.view;
    let (hunks, current) = v.hunk_position();
    let (removed, added) = v.diff.counts();
    let mut title: Vec<Span> = vec![
        " ".into(),
        v.file.label().bold(),
        " ".into(),
        v.target.label().yellow(),
        " ".into(),
    ];
    if d.siblings.len() > 1 {
        title.push(format!("{}/{} ", d.index + 1, d.siblings.len()).dim());
    }
    title.push(format!("-{removed} +{added} ").dim());
    match current {
        Some(c) => title.push(format!("hunk {c}/{hunks} ").dim()),
        None => title.push(format!("{hunks} hunks ").dim()),
    }
    title.push(if v.diff.structural {
        "structural ".green().dim()
    } else {
        "line diff ".dim()
    });
    if v.diff.large_residual {
        title.push("rewritten? ".red());
    }
    if let Some(s) = &v.search {
        title.push(format!("/{} ({}) ", s.query, s.matches.len()).yellow());
    }
    title.push(format!("[{}] ", layout.label()).dim());
    let block = title_block(&app.theme, title.into(), true);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let width = inner.width as usize;
    let height = inner.height as usize;
    app.measured.main_height = height;
    app.measured.main_width = width;
    if v.is_empty() {
        frame.render_widget(Paragraph::new("(no content)".dim()), inner);
        return;
    }
    if layout.side_by_side(width) {
        v.ensure_visible(height, true);
        let [l, sep, r] = Layout::horizontal([
            Constraint::Fill(1),
            Constraint::Length(1),
            Constraint::Fill(1),
        ])
        .areas(inner);
        let (left, right) = v.render_columns(&theme, l.width as usize, height, highlight);
        frame.render_widget(Paragraph::new(left), l);
        frame.render_widget(Paragraph::new(vec![Line::from("│".dim()); height]), sep);
        frame.render_widget(Paragraph::new(right), r);
    } else {
        v.ensure_visible(height, false);
        let lines = v.render_unified(&theme, width, height, highlight);
        frame.render_widget(Paragraph::new(lines), inner);
    }
}

fn draw_review(frame: &mut Frame, app: &mut App, area: Rect) {
    let theme = app.theme.clone();
    let Screen::Review(state) = app.screens.last_mut().expect("screen") else {
        return;
    };
    let pending = state
        .items
        .items
        .iter()
        .filter(|a| a.comment.is_pending())
        .count();
    let title: Line = vec![
        " review ".into(),
        format!(
            "{pending} pending, {} completed ",
            state.items.items.len() - pending
        )
        .dim(),
        (if state.show_completed {
            "(a hides completed) "
        } else {
            "(a shows completed) "
        })
        .dim(),
    ]
    .into();
    let block = title_block(&app.theme, title, true);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    app.measured.list_height = inner.height as usize;
    state.items.ensure_visible(inner.height as usize);
    let width = inner.width as usize;
    let visible = App::review_visible(state);
    let lines = list_lines(
        style::cursor(&app.theme),
        visible.into_iter(),
        state.items.cursor,
        state.items.scroll,
        inner.height as usize,
        true,
        |a| {
            let style = comment_style(&theme, a.state, a.comment.is_pending());
            let mark = if a.comment.is_pending() { "◆" } else { "✓" };
            let loc = a.location();
            let state_txt = match a.state {
                AnchorState::Exact => String::new(),
                AnchorState::Moved => " moved".to_string(),
                AnchorState::Stale => " stale".to_string(),
            };
            let author = a
                .comment
                .author
                .clone()
                .map(|x| format!(" {x}"))
                .unwrap_or_default();
            let head = format!("{mark} {loc}{state_txt}{author}: ");
            let avail = width.saturating_sub(head.width() + 1);
            let text: String = a.comment.text.chars().take(avail).collect();
            vec![Span::styled(head, style.bold()), Span::styled(text, style)].into()
        },
    );
    frame.render_widget(Paragraph::new(lines), inner);
    let _ = ReviewState::hint;
}

impl ReviewState {
    #[allow(dead_code)]
    fn hint() {}
}

fn draw_notes(frame: &mut Frame, app: &mut App, area: Rect) {
    let Screen::Notes(state) = app.screens.last_mut().expect("screen") else {
        return;
    };
    let title: Line = vec![
        " notes ".into(),
        format!("{} ", state.items.items.len()).dim(),
    ]
    .into();
    let block = title_block(&app.theme, title, true);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    app.measured.list_height = inner.height as usize;
    state.items.ensure_visible(inner.height as usize);
    let width = inner.width as usize;
    let lines = list_lines(
        style::cursor(&app.theme),
        state.items.items.iter(),
        state.items.cursor,
        state.items.scroll,
        inner.height as usize,
        true,
        |n| {
            let head = format!("{}: ", n.target);
            let avail = width.saturating_sub(head.width() + 1);
            let text: String = n.text.replace('\n', " ⏎ ").chars().take(avail).collect();
            vec![head.cyan().bold(), text.into()].into()
        },
    );
    frame.render_widget(Paragraph::new(lines), inner);
    let _ = NotesState::hint;
}

impl NotesState {
    #[allow(dead_code)]
    fn hint() {}
}

fn draw_locations(frame: &mut Frame, app: &mut App, area: Rect) {
    let theme = app.theme.clone();
    let Screen::Locations(state) = app.screens.last_mut().expect("screen") else {
        return;
    };
    let title: Line = vec![
        " ".into(),
        state.title.clone().bold(),
        format!(" {}/{} ", state.items.cursor + 1, state.items.items.len()).into(),
    ]
    .into();
    let block = title_block(&theme, title, true);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    app.measured.list_height = inner.height as usize;
    state.items.ensure_visible(inner.height as usize);
    let width = inner.width as usize;
    let loc_width = state
        .items
        .items
        .iter()
        .map(|h| h.path.len() + 1 + h.line.to_string().len())
        .max()
        .unwrap_or(10)
        .min(width / 2);
    let lines = list_lines(
        style::cursor(&theme),
        state.items.items.iter(),
        state.items.cursor,
        state.items.scroll,
        inner.height as usize,
        true,
        |h| {
            let loc = format!("{}:{}", h.path, h.line);
            let mut spans: Vec<Span> = vec![Span::styled(
                format!("{loc:<loc_width$} "),
                style::accent(&theme),
            )];
            if !h.label.is_empty() {
                spans.push(format!("{} ", h.label).bold());
            }
            let used: usize = spans.iter().map(|s| s.width()).sum();
            let avail = width.saturating_sub(used + 1);
            let text: String = h.text.chars().take(avail).collect();
            spans.push(Span::styled(text, style::dim(&theme)));
            spans.into()
        },
    );
    frame.render_widget(Paragraph::new(lines), inner);
}

/// Wraps `text` to `width` cells per line, breaking at spaces where possible.
fn wrap_text(text: &str, width: usize) -> Vec<String> {
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

const SPINNER: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

fn draw_agent(frame: &mut Frame, app: &mut App, area: Rect) {
    let theme = app.theme.clone();
    let name = app
        .agent
        .as_ref()
        .map(|a| a.name().to_string())
        .unwrap_or_else(|| app.agent_label());
    let mut title: Vec<Span> = vec![
        " agent ".into(),
        name.bold(),
        format!(" {} ", app.agent_state.status).into(),
    ];
    if let Some(progress) = app.agent_progress() {
        let frame_ = (app
            .agent_state
            .turn_started
            .or(app.agent_state.start_began)
            .map(|t| t.elapsed().as_millis() / 80)
            .unwrap_or(0)
            % SPINNER.len() as u128) as usize;
        title.push(Span::styled(
            format!("{} {progress} ", SPINNER[frame_]),
            style::accent(&theme),
        ));
    }
    if let Some(usage) = app.agent_usage() {
        title.push(Span::styled(format!("{usage} "), style::dim(&theme)));
    }
    let flags = format!(
        "{}{}",
        if app.agent_state.show_thoughts {
            "thoughts shown "
        } else {
            ""
        },
        if app.agent_state.show_log {
            ""
        } else {
            "log hidden "
        }
    );
    if !flags.is_empty() {
        title.push(Span::styled(flags, style::dim(&theme)));
    }
    let title: Line = title.into();
    let block = title_block(&theme, title, true);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let has_permission = app.agent_state.permission.is_some();
    let [body, ask] = Layout::vertical([
        Constraint::Fill(1),
        Constraint::Length(if has_permission { 2 } else { 0 }),
    ])
    .areas(inner);
    app.measured.main_height = body.height as usize;
    app.measured.main_width = body.width as usize;
    let width = body.width.saturating_sub(1) as usize;
    let mut lines: Vec<Line> = Vec::new();
    for entry in &app.agent_state.entries {
        if entry.kind == EntryKind::Thought && !app.agent_state.show_thoughts {
            continue;
        }
        if entry.kind == EntryKind::Log && !app.agent_state.show_log {
            continue;
        }
        let (prefix, style_) = match entry.kind {
            EntryKind::User => ("you  ", style::accent(&theme).bold()),
            EntryKind::Agent => ("agent", Style::new()),
            EntryKind::Thought => ("think", style::dim(&theme)),
            EntryKind::Tool => ("tool ", style::fg(theme.update_fg)),
            EntryKind::System => ("     ", style::dim(&theme)),
            EntryKind::Log => ("log  ", style::dim(&theme)),
        };
        let text_style = match entry.kind {
            EntryKind::Thought | EntryKind::System | EntryKind::Log => style::dim(&theme),
            EntryKind::Tool => style::fg(theme.update_fg),
            _ => Style::new(),
        };
        for (i, wrapped) in wrap_text(&entry.text, width.saturating_sub(7))
            .into_iter()
            .enumerate()
        {
            let head = if i == 0 {
                format!("{prefix} │ ")
            } else {
                "      │ ".to_string()
            };
            lines.push(
                vec![
                    Span::styled(head, style_),
                    Span::styled(wrapped, text_style),
                ]
                .into(),
            );
        }
        if entry.kind != EntryKind::Log {
            lines.push("".into());
        }
    }
    if app.agent_state.entries.is_empty() {
        lines.push(
            "  Nothing yet. Press i to ask something, A to have every pending comment addressed."
                .dim()
                .into(),
        );
        lines.push("".into());
        lines.push(
            format!(
                "  The agent is `{}`; change [agent] in the config file to use another.",
                app.agent_label()
            )
            .dim()
            .into(),
        );
    }
    let total = lines.len();
    let height = body.height as usize;
    let bottom = total.saturating_sub(height);
    if app.agent_state.follow {
        app.agent_state.scroll = bottom;
    }
    app.agent_state.scroll = app.agent_state.scroll.min(bottom);
    // Scrolling back down to the end re-engages following, as `G` does.
    if app.agent_state.scroll == bottom {
        app.agent_state.follow = true;
    }
    let shown: Vec<Line> = lines
        .into_iter()
        .skip(app.agent_state.scroll)
        .take(height)
        .collect();
    frame.render_widget(Paragraph::new(shown), body);
    if let Some((_, title, options)) = &app.agent_state.permission {
        let mut spans: Vec<Span> = vec![
            Span::styled(" agent asks: ", style::fg(theme.moved).bold()),
            title.clone().into(),
        ];
        let mut choices: Vec<Span> = vec![" ".into()];
        for (i, o) in options.iter().enumerate() {
            choices.push(Span::styled(
                format!("[{}] {}  ", i + 1, o.name),
                style::accent(&theme),
            ));
        }
        choices.push("y first allow, n first reject".dim());
        spans.truncate(2);
        frame.render_widget(
            Paragraph::new(vec![Line::from(spans), Line::from(choices)]),
            ask,
        );
    }
}

fn draw_status(frame: &mut Frame, app: &mut App, area: Rect) {
    if let Some(prompt) = &app.prompt {
        prompt.draw(frame, area);
        return;
    }
    let mut spans: Vec<Span> = vec![format!(" {} ", app.screen().name()).reversed()];
    if app.agent_state.permission.is_some() && !matches!(app.screen(), Screen::Agent) {
        spans.push(" ".into());
        spans.push(Span::styled(
            "agent is waiting for permission (Ctrl-a)",
            style::fg(app.theme.moved).bold(),
        ));
    } else if app.agent_active() && !matches!(app.screen(), Screen::Agent) {
        spans.push(" ".into());
        spans.push(Span::styled(
            format!(
                "agent: {} (A)",
                app.agent_progress().unwrap_or_else(|| "working".into())
            ),
            style::dim(&app.theme),
        ));
    } else if let Some((text, is_error)) = &app.message {
        spans.push(" ".into());
        spans.push(if *is_error {
            text.clone().red()
        } else {
            text.clone().into()
        });
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
    ("magenta", "moved"),
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
