//! Drawing for the screens that stack on top of the explorer: the log and a commit's files,
//! the changes list, a diff, the review list, the notes list, and a list of places to jump
//! to.

use super::*;

/// The log is two panes: the commits, and what the one under the cursor changed.
pub(super) fn draw_log(frame: &mut Frame, app: &mut App, area: Rect) {
    let [left, right] =
        Layout::horizontal([Constraint::Percentage(50), Constraint::Fill(1)]).areas(area);
    draw_commits(frame, app, left);
    draw_commit_files(frame, app, right);
}

/// The commits, newest first, with the file's name in the title when this is one file's
/// history rather than the whole repository's.
fn draw_commits(frame: &mut Frame, app: &mut App, left: Rect) {
    let Screen::Log(state) = app.screens.last_mut().expect("screen") else {
        return;
    };
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
}

/// What the commit under the cursor changed: its message, and the files it touched.
fn draw_commit_files(frame: &mut Frame, app: &mut App, right: Rect) {
    let Screen::Log(state) = app.screens.last_mut().expect("screen") else {
        return;
    };
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
}

pub(super) fn draw_changes(frame: &mut Frame, app: &mut App, area: Rect) {
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
}

pub(super) fn draw_diff(frame: &mut Frame, app: &mut App, area: Rect) {
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

pub(super) fn draw_review(frame: &mut Frame, app: &mut App, area: Rect) {
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
}

pub(super) fn draw_notes(frame: &mut Frame, app: &mut App, area: Rect) {
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
}

pub(super) fn draw_locations(frame: &mut Frame, app: &mut App, area: Rect) {
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
