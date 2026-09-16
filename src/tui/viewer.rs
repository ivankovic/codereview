//! The two code viewers: one file with its comments woven in as rows below the lines they
//! refer to, and a diff of two versions of a file, side by side or unified.

use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthChar;

use crate::anchor::{AnchorState, Anchored};
use crate::diff::{AlignedRow, FileDiff, Op, Span as DiffSpan};
use crate::highlight::{Segment, split_at};
use crate::repo::{BlameLine, ChangedFile};
use crate::session::{DiffTarget, FileView};
use crate::theme::Theme;
use crate::tui::style::{self, change_span, comment_style, op_marker, segment_span};

const TAB_WIDTH: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Row {
    /// 0-based line of the file.
    Code(usize),
    /// Index into the viewer's comments, shown under the comment's last line.
    Comment(usize),
}

#[derive(Debug, Clone, Default)]
pub struct Search {
    pub query: String,
    /// (line, start byte, end byte), in file order.
    pub matches: Vec<(usize, usize, usize)>,
}

impl Search {
    pub fn new(query: &str, lines: &[String]) -> Self {
        let smart_case = query.chars().any(char::is_uppercase);
        let needle = if smart_case {
            query.to_string()
        } else {
            query.to_lowercase()
        };
        let mut matches = Vec::new();
        if !needle.is_empty() {
            for (i, line) in lines.iter().enumerate() {
                let hay = if smart_case {
                    line.clone()
                } else {
                    line.to_lowercase()
                };
                let mut from = 0;
                while let Some(pos) = hay[from..].find(&needle) {
                    let start = from + pos;
                    // Lowercasing can change byte lengths for some scripts; clip defensively.
                    let end = (start + needle.len()).min(line.len());
                    if start < line.len()
                        && line.is_char_boundary(start)
                        && line.is_char_boundary(end)
                    {
                        matches.push((i, start, end));
                    }
                    from = start + needle.len().max(1);
                    if from >= hay.len() {
                        break;
                    }
                }
            }
        }
        Self {
            query: query.to_string(),
            matches,
        }
    }

    fn ranges_on(&self, line: usize) -> Vec<(usize, usize)> {
        self.matches
            .iter()
            .filter(|(l, _, _)| *l == line)
            .map(|(_, s, e)| (*s, *e))
            .collect()
    }

    /// The next match line after `line` (or before, backwards), wrapping.
    pub fn next_line(&self, line: usize, forward: bool) -> Option<usize> {
        if self.matches.is_empty() {
            return None;
        }
        let lines: Vec<usize> = self.matches.iter().map(|m| m.0).collect();
        if forward {
            lines.iter().find(|&&l| l > line).or(lines.first()).copied()
        } else {
            lines
                .iter()
                .rev()
                .find(|&&l| l < line)
                .or(lines.last())
                .copied()
        }
    }
}

pub struct Viewer {
    pub path: String,
    pub lines: Vec<String>,
    pub highlighted: Vec<Vec<Segment>>,
    pub comments: Vec<Anchored>,
    pub blame: Option<Vec<BlameLine>>,
    /// Per-line change against the last committed or staged version, when the file differs.
    pub line_ops: Option<Vec<Op>>,
    pub binary: bool,
    pub rows: Vec<Row>,
    pub cursor: usize,
    pub scroll: usize,
    pub hscroll: usize,
    /// Byte column of the cursor within the current line, clamped at render time.
    pub col: usize,
    /// The code area width the last render had, for keeping the column in view.
    pub code_width: usize,
    /// Start line (0-based) of a visual selection.
    pub visual: Option<usize>,
    pub search: Option<Search>,
}

impl Viewer {
    pub fn new(view: FileView, syntax_theme: &str) -> Self {
        let lines: Vec<String> = view.text.lines().map(str::to_string).collect();
        let highlighted = crate::highlight::highlight(&view.path, &view.text, syntax_theme);
        let mut viewer = Self {
            path: view.path,
            lines,
            highlighted,
            comments: view.comments,
            blame: None,
            line_ops: None,
            binary: view.binary,
            rows: Vec::new(),
            cursor: 0,
            scroll: 0,
            hscroll: 0,
            col: 0,
            code_width: 80,
            visual: None,
            search: None,
        };
        viewer.rebuild_rows();
        viewer
    }

    /// The current line's text, for column motions.
    fn line(&self) -> &str {
        self.current_line()
            .and_then(|i| self.lines.get(i))
            .map(String::as_str)
            .unwrap_or("")
    }

    /// The cursor column clamped to the line, on a character boundary.
    pub fn column(&self) -> usize {
        let line = self.line();
        let mut col = self.col.min(line.len());
        while col > 0 && !line.is_char_boundary(col) {
            col -= 1;
        }
        col
    }

    pub fn move_col(&mut self, delta: isize) {
        let line = self.line().to_string();
        let mut col = self.column();
        if delta > 0 {
            for _ in 0..delta {
                match line[col..].chars().next() {
                    Some(c) if col + c.len_utf8() < line.len() => col += c.len_utf8(),
                    Some(c) if col + c.len_utf8() == line.len() => col += c.len_utf8(),
                    _ => break,
                }
            }
            // Stay on the last character, not past it, on a non-empty line.
            if col >= line.len() && !line.is_empty() {
                col = line.len() - line.chars().next_back().map_or(0, char::len_utf8);
            }
        } else {
            for _ in 0..(-delta) {
                match line[..col].chars().next_back() {
                    Some(c) => col -= c.len_utf8(),
                    None => break,
                }
            }
        }
        self.col = col;
        self.keep_col_visible();
    }

    pub fn col_home(&mut self) {
        self.col = 0;
        self.keep_col_visible();
    }

    pub fn col_first_nonblank(&mut self) {
        let line = self.line();
        self.col = line.len() - line.trim_start().len();
        self.keep_col_visible();
    }

    pub fn col_end(&mut self) {
        let line = self.line();
        self.col = line.len() - line.chars().next_back().map_or(0, char::len_utf8);
        self.keep_col_visible();
    }

    /// `w`: the start of the next word; `b`: the start of the previous one.
    pub fn word(&mut self, forward: bool) {
        let line = self.line().to_string();
        let is_word = |c: char| c.is_alphanumeric() || c == '_';
        let col = self.column();
        let next = if forward {
            let mut i = col;
            let mut in_word = line[i..].chars().next().is_some_and(is_word);
            for (off, c) in line[col..].char_indices() {
                i = col + off;
                if in_word && !is_word(c) {
                    in_word = false;
                } else if !in_word && is_word(c) {
                    break;
                }
            }
            if line[i..].chars().next().is_some_and(is_word) && i != col {
                i
            } else if in_word {
                col
            } else {
                i
            }
        } else {
            let mut i = col;
            // Skip back over non-word characters, then to the start of the word.
            while i > 0 {
                let c = line[..i].chars().next_back().expect("i > 0");
                if is_word(c) {
                    break;
                }
                i -= c.len_utf8();
            }
            while i > 0 {
                let c = line[..i].chars().next_back().expect("i > 0");
                if !is_word(c) {
                    break;
                }
                i -= c.len_utf8();
            }
            i
        };
        self.col = next;
        self.keep_col_visible();
    }

    /// Scrolls horizontally so the cursor column is on screen.
    pub fn keep_col_visible(&mut self) {
        let line = self.line();
        let col = self.column();
        let display = display_width(&line[..col]);
        let width = self.code_width.max(4);
        if display < self.hscroll {
            self.hscroll = display;
        } else if display >= self.hscroll + width - 1 {
            self.hscroll = display + 2 - width;
        }
    }

    pub fn rehighlight(&mut self, syntax_theme: &str) {
        let text = self.lines.join("\n");
        self.highlighted = crate::highlight::highlight(&self.path, &text, syntax_theme);
    }

    pub fn set_comments(&mut self, comments: Vec<Anchored>) {
        let line = self.current_line();
        self.comments = comments;
        self.rebuild_rows();
        if let Some(line) = line {
            self.go_to_line(line);
        }
    }

    fn rebuild_rows(&mut self) {
        let mut rows = Vec::with_capacity(self.lines.len() + self.comments.len());
        // Comments on the file as a whole have no line; they head the file.
        for (ci, c) in self.comments.iter().enumerate() {
            if c.line.is_none() {
                rows.push(Row::Comment(ci));
            }
        }
        for i in 0..self.lines.len() {
            rows.push(Row::Code(i));
            for (ci, c) in self.comments.iter().enumerate() {
                if c.last_row() == Some(i) {
                    rows.push(Row::Comment(ci));
                }
            }
        }
        // Comments past the end of the file (stale, file shrank) go last.
        for (ci, c) in self.comments.iter().enumerate() {
            if c.last_row().is_some_and(|r| r >= self.lines.len()) {
                rows.push(Row::Comment(ci));
            }
        }
        if rows.is_empty() {
            rows.push(Row::Code(0));
        }
        self.rows = rows;
        self.cursor = self.cursor.min(self.rows.len() - 1);
    }

    pub fn line_count(&self) -> usize {
        self.lines.len()
    }

    /// The 0-based file line the cursor refers to; `None` on a comment on the whole file.
    pub fn current_line(&self) -> Option<usize> {
        match self.rows.get(self.cursor)? {
            Row::Code(i) => Some(*i),
            Row::Comment(ci) => Some(self.comments[*ci].rows()?.start),
        }
    }

    pub fn current_comment(&self) -> Option<&Anchored> {
        match self.rows.get(self.cursor)? {
            Row::Comment(ci) => self.comments.get(*ci),
            Row::Code(i) => self.comments.iter().find(|c| c.covers_row(*i)),
        }
    }

    pub fn on_comment_row(&self) -> bool {
        matches!(self.rows.get(self.cursor), Some(Row::Comment(_)))
    }

    /// The selected line range (0-based, inclusive), from the visual anchor to the cursor.
    pub fn selection(&self) -> Option<(usize, usize)> {
        let cur = self.current_line()?;
        let start = self.visual.unwrap_or(cur);
        Some((start.min(cur), start.max(cur)))
    }

    pub fn move_by(&mut self, delta: isize) {
        if self.rows.is_empty() {
            return;
        }
        let max = self.rows.len() as isize - 1;
        self.cursor = (self.cursor as isize + delta).clamp(0, max) as usize;
    }

    /// Moves over code rows only, skipping comment rows.
    pub fn move_lines(&mut self, delta: isize) {
        let Some(line) = self.current_line() else {
            return;
        };
        let target = (line as isize + delta).clamp(0, self.lines.len().max(1) as isize - 1);
        self.go_to_line(target as usize);
    }

    pub fn go_to_line(&mut self, line: usize) {
        let line = line.min(self.lines.len().saturating_sub(1));
        if let Some(i) = self.rows.iter().position(|r| *r == Row::Code(line)) {
            self.cursor = i;
        }
    }

    pub fn go_top(&mut self) {
        self.cursor = 0;
    }

    pub fn go_bottom(&mut self) {
        self.cursor = self.rows.len().saturating_sub(1);
    }

    pub fn next_comment(&mut self, forward: bool) {
        let idx: Vec<usize> = self
            .rows
            .iter()
            .enumerate()
            .filter(|(_, r)| matches!(r, Row::Comment(_)))
            .map(|(i, _)| i)
            .collect();
        let next = if forward {
            idx.iter().find(|&&i| i > self.cursor).or(idx.first())
        } else {
            idx.iter().rev().find(|&&i| i < self.cursor).or(idx.last())
        };
        if let Some(&i) = next {
            self.cursor = i;
        }
    }

    pub fn next_change(&mut self, forward: bool) {
        let Some(ops) = &self.line_ops else {
            return;
        };
        let Some(cur) = self.current_line() else {
            return;
        };
        if let Some(line) = next_hunk(ops, cur, forward) {
            self.go_to_line(line);
        }
    }

    pub fn set_search(&mut self, query: &str) -> usize {
        if query.is_empty() {
            self.search = None;
            return 0;
        }
        let search = Search::new(query, &self.lines);
        let n = search.matches.len();
        self.search = Some(search);
        n
    }

    pub fn next_match(&mut self, forward: bool) {
        let Some(search) = &self.search else {
            return;
        };
        let cur = self.current_line().unwrap_or(0);
        if let Some(line) = search.next_line(cur, forward) {
            self.go_to_line(line);
        }
    }

    /// Jumps to the first match at or after the cursor.
    pub fn nearest_match(&mut self) {
        let Some(search) = &self.search else {
            return;
        };
        let cur = self.current_line().unwrap_or(0);
        let line = search
            .matches
            .iter()
            .map(|m| m.0)
            .find(|&l| l >= cur)
            .or_else(|| search.matches.first().map(|m| m.0));
        if let Some(line) = line {
            self.go_to_line(line);
        }
    }

    pub fn ensure_visible(&mut self, height: usize) {
        if height == 0 {
            return;
        }
        if self.cursor < self.scroll {
            self.scroll = self.cursor;
        } else if self.cursor >= self.scroll + height {
            self.scroll = self.cursor + 1 - height;
        }
        self.scroll = self.scroll.min(self.rows.len().saturating_sub(1));
    }

    pub fn scroll_by(&mut self, delta: isize, height: usize) {
        let max = self.rows.len().saturating_sub(1) as isize;
        self.scroll = (self.scroll as isize + delta).clamp(0, max) as usize;
        if self.cursor < self.scroll {
            self.cursor = self.scroll;
        } else if self.cursor >= self.scroll + height {
            self.cursor = (self.scroll + height).saturating_sub(1).min(max as usize);
        }
    }

    pub fn gutter_width(&self) -> usize {
        let digits = self.lines.len().max(1).to_string().len();
        let blame = self.blame.as_ref().map_or(0, |_| 26);
        // marker + change marker + number + separator
        2 + blame + digits + 2
    }

    /// Renders `height` rows starting at `scroll`, `width` cells wide.
    pub fn render(
        &self,
        theme: &Theme,
        width: usize,
        height: usize,
        highlight: bool,
        focused: bool,
    ) -> Vec<Line<'static>> {
        let mut out = Vec::with_capacity(height);
        if self.binary {
            out.push("(binary file)".dim().into());
            return out;
        }
        if self.lines.is_empty() {
            out.push("(empty file)".dim().into());
        }
        let dim = style::dim(theme);
        let digits = self.lines.len().max(1).to_string().len();
        let gutter = self.gutter_width();
        let code_width = width.saturating_sub(gutter);
        // `render` takes `&self`; the width is recorded through interior mutability's poor
        // cousin: the caller sets `code_width` from `gutter_width()` before moving columns.
        let selection = self.visual.and_then(|_| self.selection());
        for (ri, row) in self.rows.iter().enumerate().skip(self.scroll).take(height) {
            let is_cursor = ri == self.cursor;
            match *row {
                Row::Code(i) => {
                    let Some(text) = self.lines.get(i) else {
                        continue;
                    };
                    let op = self
                        .line_ops
                        .as_ref()
                        .and_then(|o| o.get(i).copied())
                        .unwrap_or(Op::None);
                    let in_selection = selection.is_some_and(|(a, b)| (a..=b).contains(&i));
                    let cursor_here = is_cursor && focused;
                    let base_bg = if cursor_here {
                        style::color(theme.cursor_bg)
                    } else if in_selection {
                        style::color(theme.visual_bg)
                    } else {
                        style::color(theme.line_bg(op))
                    };
                    let mut spans: Vec<Span<'static>> = Vec::new();
                    let marker = self
                        .comments
                        .iter()
                        .find(|c| c.covers_row(i))
                        .map(|c| {
                            Span::styled("●", comment_style(theme, c.state, c.comment.is_pending()))
                        })
                        .unwrap_or_else(|| " ".into());
                    spans.push(marker);
                    spans.push(Span::styled(op_marker(op), style::fg(theme.op_fg(op))));
                    if let Some(blame) = &self.blame {
                        let text = blame
                            .get(i)
                            .map(|b| {
                                let author: String = b.author.chars().take(10).collect();
                                format!(
                                    "{:>7} {:<10} {} ",
                                    &b.hash[..7.min(b.hash.len())],
                                    author,
                                    b.date
                                )
                            })
                            .unwrap_or_else(|| " ".repeat(26));
                        spans.push(Span::styled(text, dim));
                    }
                    spans.push(Span::styled(format!("{:>digits$} ", i + 1), dim));
                    spans.push(Span::styled("│", dim));
                    let search = self
                        .search
                        .as_ref()
                        .map(|s| s.ranges_on(i))
                        .unwrap_or_default();
                    let segs = self
                        .highlighted
                        .get(i)
                        .cloned()
                        .unwrap_or_else(|| vec![Segment::plain(text)]);
                    let cursor_col = cursor_here.then(|| self.column());
                    let code = paint(
                        theme,
                        &segs,
                        &[],
                        &search,
                        cursor_col,
                        base_bg,
                        highlight,
                        self.hscroll,
                        code_width,
                    );
                    spans.extend(code);
                    let mut line = Line::from(spans);
                    if cursor_here && base_bg.is_none() {
                        line = line.style(Style::new().add_modifier(Modifier::REVERSED));
                    }
                    out.push(line);
                }
                Row::Comment(ci) => {
                    let c = &self.comments[ci];
                    out.push(comment_row(theme, c, gutter, width, is_cursor && focused));
                }
            }
        }
        out
    }
}

/// One comment as a row: author, state, and text, indented into the code column.
fn comment_row(
    theme: &Theme,
    c: &Anchored,
    gutter: usize,
    width: usize,
    is_cursor: bool,
) -> Line<'static> {
    let style = comment_style(theme, c.state, c.comment.is_pending());
    let mut head = String::new();
    head.push_str(if c.comment.is_pending() {
        "◆ "
    } else {
        "✓ "
    });
    if let Some(a) = &c.comment.author {
        head.push_str(a);
        head.push(' ');
    }
    match c.state {
        AnchorState::Exact => {}
        AnchorState::Moved => {
            if let Some(label) = c.comment.line_label() {
                head.push_str(&format!("(moved from {label}) "));
            }
        }
        AnchorState::Stale => {
            if let Some(label) = c.comment.line_label() {
                head.push_str(&format!("(stale, was line {label}) "));
            }
        }
    }
    let avail = width.saturating_sub(gutter + head.len() + 1);
    let text = truncate(&c.comment.text, avail);
    let mut spans: Vec<Span<'static>> = vec![
        " ".repeat(gutter.saturating_sub(1)).into(),
        Span::styled("│", style::dim(theme)),
    ];
    spans.push(Span::styled(head, style.bold()));
    spans.push(Span::styled(text, style));
    let mut line = Line::from(spans);
    if is_cursor {
        line = line.style(style::cursor(theme));
    }
    line
}

fn truncate(text: &str, width: usize) -> String {
    let mut out = String::new();
    let mut w = 0;
    for ch in text.chars() {
        let cw = ch.width().unwrap_or(0);
        if w + cw > width {
            if width > 0 {
                out.pop();
                out.push('…');
            }
            return out;
        }
        out.push(ch);
        w += cw;
    }
    out
}

/// Terminal cells `text` occupies, tabs expanded from column 0.
pub fn display_width(text: &str) -> usize {
    let mut col = 0;
    for ch in text.chars() {
        col += if ch == '\t' {
            TAB_WIDTH - (col % TAB_WIDTH)
        } else {
            ch.width().unwrap_or(0)
        };
    }
    col
}

/// The line index of the next (or previous) run of changed lines after `from`, wrapping.
pub fn next_hunk(ops: &[Op], from: usize, forward: bool) -> Option<usize> {
    let starts: Vec<usize> = ops
        .iter()
        .enumerate()
        .filter(|(i, op)| op.is_change() && (*i == 0 || !ops[i - 1].is_change()))
        .map(|(i, _)| i)
        .collect();
    if starts.is_empty() {
        return None;
    }
    if forward {
        starts
            .iter()
            .find(|&&s| s > from)
            .or(starts.first())
            .copied()
    } else {
        // The start of the hunk before the one containing `from`.
        let current_start = if ops.get(from).is_some_and(|o| o.is_change()) {
            starts.iter().rev().find(|&&s| s <= from).copied()
        } else {
            None
        };
        let limit = current_start.unwrap_or(from);
        starts
            .iter()
            .rev()
            .find(|&&s| s < limit)
            .or(starts.last())
            .copied()
    }
}

/// Paints one line: highlight segments cut at diff span and search boundaries, each piece
/// given the background of what covers it, then scrolled and clipped to `width` cells.
#[allow(clippy::too_many_arguments)]
pub fn paint(
    theme: &Theme,
    segments: &[Segment],
    diff_spans: &[DiffSpan],
    search: &[(usize, usize)],
    cursor_col: Option<usize>,
    base_bg: Option<Color>,
    highlight: bool,
    hscroll: usize,
    width: usize,
) -> Vec<Span<'static>> {
    let mut cuts: Vec<usize> = Vec::new();
    let line_len: usize = segments.iter().map(|s| s.text.len()).sum();
    let cursor_range = cursor_col.filter(|c| *c < line_len).map(|c| {
        let text: String = segments.iter().map(|s| s.text.as_str()).collect();
        let len = text[c..].chars().next().map_or(1, char::len_utf8);
        (c, c + len)
    });
    if let Some((s, e)) = cursor_range {
        cuts.push(s);
        cuts.push(e);
    }
    for s in diff_spans {
        cuts.push(s.start);
        cuts.push(s.end);
    }
    for (s, e) in search {
        cuts.push(*s);
        cuts.push(*e);
    }
    cuts.sort_unstable();
    cuts.dedup();
    let pieces = split_at(segments, &cuts);
    let mut spans: Vec<Span<'static>> = Vec::with_capacity(pieces.len());
    for (offset, seg) in pieces {
        if cursor_range.is_some_and(|(s, e)| s <= offset && offset < e) {
            let mut span = segment_span(&seg, highlight, base_bg);
            span.style = span.style.add_modifier(Modifier::REVERSED);
            spans.push(span);
        } else if search.iter().any(|(s, e)| *s <= offset && offset < *e) {
            let mut span = segment_span(&seg, highlight, style::color(theme.search_bg).or(base_bg));
            if theme.terminal {
                span.style = span.style.add_modifier(Modifier::REVERSED);
            }
            spans.push(span);
        } else if let Some(op) = diff_spans
            .iter()
            .find(|s| s.start <= offset && offset < s.end)
            .map(|s| s.op)
        {
            spans.push(change_span(&seg, op, theme, highlight, base_bg));
        } else {
            spans.push(segment_span(&seg, highlight, base_bg));
        }
    }
    let mut clipped = clip(spans, hscroll, width);
    if cursor_col.is_some() && line_len == 0 {
        clipped.push(Span::styled(
            " ",
            Style::new().add_modifier(Modifier::REVERSED),
        ));
    }
    if let Some(bg) = base_bg {
        // Fill the rest of the row so the line background reaches the edge.
        let used: usize = clipped.iter().map(|s| s.width()).sum();
        if used < width {
            clipped.push(Span::styled(" ".repeat(width - used), Style::new().bg(bg)));
        }
    }
    clipped
}

/// Expands tabs, drops the first `hscroll` cells, keeps at most `width` cells. A `…` marks a
/// cut edge on either side.
fn clip(spans: Vec<Span<'static>>, hscroll: usize, width: usize) -> Vec<Span<'static>> {
    let mut out: Vec<Span<'static>> = Vec::new();
    let mut col = 0usize;
    let mut shown = 0usize;
    let mut truncated_right = false;
    'outer: for span in spans {
        let mut buf = String::new();
        for ch in span.content.chars() {
            let cw = if ch == '\t' {
                TAB_WIDTH - (col % TAB_WIDTH)
            } else {
                ch.width().unwrap_or(0)
            };
            let visible = col + cw > hscroll;
            if visible {
                if shown + cw > width {
                    truncated_right = true;
                    if !buf.is_empty() {
                        out.push(Span::styled(std::mem::take(&mut buf), span.style));
                    }
                    break 'outer;
                }
                let partial = col < hscroll;
                if ch == '\t' || partial {
                    // A tab, or a wide character cut by the scroll edge: only its remaining cells.
                    let cells = if partial { col + cw - hscroll } else { cw };
                    buf.push_str(&" ".repeat(cells));
                    shown += cells;
                } else {
                    buf.push(ch);
                    shown += cw;
                }
            }
            col += cw;
        }
        if !buf.is_empty() {
            out.push(Span::styled(buf, span.style));
        }
    }
    if hscroll > 0 && !out.is_empty() {
        out.insert(0, "…".dim());
        // Drop one cell to make room for the marker.
        if let Some(first) = out.get_mut(1) {
            let mut chars = first.content.chars();
            chars.next();
            let rest: String = chars.collect();
            *first = Span::styled(rest, first.style);
        }
    }
    if truncated_right {
        if let Some(last) = out.last_mut() {
            let mut s: String = last.content.chars().collect();
            s.pop();
            s.push('…');
            *last = Span::styled(s, last.style);
        }
    }
    out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffLayout {
    Auto,
    SideBySide,
    Unified,
}

impl DiffLayout {
    pub fn next(self) -> Self {
        match self {
            Self::Auto => Self::SideBySide,
            Self::SideBySide => Self::Unified,
            Self::Unified => Self::Auto,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::SideBySide => "side by side",
            Self::Unified => "unified",
        }
    }

    /// The config file spelling.
    pub fn name(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::SideBySide => "side-by-side",
            Self::Unified => "unified",
        }
    }

    pub fn from_name(name: &str) -> Self {
        match name {
            "side-by-side" => Self::SideBySide,
            "unified" => Self::Unified,
            _ => Self::Auto,
        }
    }

    pub fn side_by_side(self, width: usize) -> bool {
        match self {
            Self::Auto => width >= 160,
            Self::SideBySide => true,
            Self::Unified => false,
        }
    }
}

pub struct DiffView {
    pub target: DiffTarget,
    pub file: ChangedFile,
    pub diff: FileDiff,
    pub before_lines: Vec<String>,
    pub after_lines: Vec<String>,
    pub before_hl: Vec<Vec<Segment>>,
    pub after_hl: Vec<Vec<Segment>>,
    pub rows: Vec<AlignedRow>,
    /// Comments anchored against the after side.
    pub comments: Vec<Anchored>,
    pub cursor: usize,
    pub scroll: usize,
    pub hscroll: usize,
    pub search: Option<Search>,
    /// Which side the cursor line refers to, for commenting and searching.
    pub focus_after: bool,
}

impl DiffView {
    pub fn new(
        target: DiffTarget,
        file: ChangedFile,
        diff: FileDiff,
        comments: Vec<Anchored>,
        theme: &str,
    ) -> Self {
        let before_lines: Vec<String> = diff.before.text.lines().map(str::to_string).collect();
        let after_lines: Vec<String> = diff.after.text.lines().map(str::to_string).collect();
        let old_path = file.old_path.clone().unwrap_or_else(|| file.path.clone());
        let before_hl = crate::highlight::highlight(&old_path, &diff.before.text, theme);
        let after_hl = crate::highlight::highlight(&file.path, &diff.after.text, theme);
        let rows = diff.aligned_rows();
        let mut view = Self {
            target,
            file,
            diff,
            before_lines,
            after_lines,
            before_hl,
            after_hl,
            rows,
            comments,
            cursor: 0,
            scroll: 0,
            hscroll: 0,
            search: None,
            focus_after: true,
        };
        // Start on the first change.
        view.next_change(true, true);
        view
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    pub fn rehighlight(&mut self, syntax_theme: &str) {
        let old_path = self
            .file
            .old_path
            .clone()
            .unwrap_or_else(|| self.file.path.clone());
        self.before_hl =
            crate::highlight::highlight(&old_path, &self.diff.before.text, syntax_theme);
        self.after_hl =
            crate::highlight::highlight(&self.file.path, &self.diff.after.text, syntax_theme);
    }

    pub fn move_by(&mut self, delta: isize) {
        if self.rows.is_empty() {
            return;
        }
        let max = self.rows.len() as isize - 1;
        self.cursor = (self.cursor as isize + delta).clamp(0, max) as usize;
    }

    pub fn go_top(&mut self) {
        self.cursor = 0;
    }

    pub fn go_bottom(&mut self) {
        self.cursor = self.rows.len().saturating_sub(1);
    }

    fn row_is_change(&self, r: &AlignedRow) -> bool {
        r.before.is_some_and(|i| {
            self.diff
                .before
                .line_ops
                .get(i)
                .is_some_and(|o| o.is_change())
        }) || r.after.is_some_and(|i| {
            self.diff
                .after
                .line_ops
                .get(i)
                .is_some_and(|o| o.is_change())
        })
    }

    /// Jumps to the start of the next hunk of changed rows; `from_start` looks from row 0
    /// inclusive rather than after the cursor.
    pub fn next_change(&mut self, forward: bool, from_start: bool) {
        let ops: Vec<Op> = self
            .rows
            .iter()
            .map(|r| {
                if self.row_is_change(r) {
                    Op::Update
                } else {
                    Op::None
                }
            })
            .collect();
        if from_start {
            if let Some(i) = ops.iter().position(|o| o.is_change()) {
                self.cursor = i;
            }
            return;
        }
        if let Some(i) = next_hunk(&ops, self.cursor, forward) {
            self.cursor = i;
        }
    }

    /// Count of hunks and which one the cursor is in, for the status bar.
    pub fn hunk_position(&self) -> (usize, Option<usize>) {
        let mut count = 0;
        let mut current = None;
        let mut in_hunk = false;
        for (i, r) in self.rows.iter().enumerate() {
            let change = self.row_is_change(r);
            if change && !in_hunk {
                count += 1;
            }
            if change && i == self.cursor {
                current = Some(count);
            }
            in_hunk = change;
        }
        (count, current)
    }

    /// The 0-based after-side line under the cursor, if the row has one.
    pub fn after_line(&self) -> Option<usize> {
        self.rows.get(self.cursor)?.after
    }

    pub fn before_line(&self) -> Option<usize> {
        self.rows.get(self.cursor)?.before
    }

    pub fn go_to_after_line(&mut self, line: usize) {
        if let Some(i) = self.rows.iter().position(|r| r.after == Some(line)) {
            self.cursor = i;
        }
    }

    pub fn set_search(&mut self, query: &str) -> usize {
        if query.is_empty() {
            self.search = None;
            return 0;
        }
        let search = Search::new(query, &self.after_lines);
        let n = search.matches.len();
        self.search = Some(search);
        n
    }

    pub fn next_match(&mut self, forward: bool) {
        let Some(search) = &self.search else {
            return;
        };
        let cur = self.after_line().unwrap_or(0);
        if let Some(line) = search.next_line(cur, forward) {
            self.go_to_after_line(line);
        }
    }

    /// Display height of row `i` in unified layout: two when both sides show a changed line.
    fn unified_height(&self, r: &AlignedRow) -> usize {
        match (r.before, r.after) {
            (Some(_), Some(_)) if self.row_is_change(r) => 2,
            _ => 1,
        }
    }

    pub fn ensure_visible(&mut self, height: usize, side_by_side: bool) {
        if height == 0 || self.rows.is_empty() {
            return;
        }
        if !side_by_side {
            // In unified layout a hunk is drawn as one block, so scrolling must start at a
            // hunk boundary or at an unchanged row: move the scroll back to the hunk's start.
            while self.scroll > 0
                && self.row_is_change(&self.rows[self.scroll])
                && self.row_is_change(&self.rows[self.scroll - 1])
            {
                self.scroll -= 1;
            }
        }
        if self.cursor < self.scroll {
            self.scroll = self.cursor;
            if !side_by_side {
                while self.scroll > 0
                    && self.row_is_change(&self.rows[self.scroll])
                    && self.row_is_change(&self.rows[self.scroll - 1])
                {
                    self.scroll -= 1;
                }
            }
            return;
        }
        // Walk forward from scroll summing display heights until the cursor fits.
        loop {
            let used: usize = self.rows[self.scroll..=self.cursor]
                .iter()
                .map(|r| {
                    if side_by_side {
                        1
                    } else {
                        self.unified_height(r)
                    }
                })
                .sum();
            if used <= height || self.scroll >= self.cursor {
                break;
            }
            self.scroll += 1;
        }
    }

    pub fn scroll_by(&mut self, delta: isize, height: usize) {
        let max = self.rows.len().saturating_sub(1) as isize;
        self.scroll = (self.scroll as isize + delta).clamp(0, max) as usize;
        if self.cursor < self.scroll {
            self.cursor = self.scroll;
        } else if self.cursor >= self.scroll + height {
            self.cursor = (self.scroll + height).saturating_sub(1).min(max as usize);
        }
    }

    fn gutter_digits(&self) -> usize {
        self.before_lines
            .len()
            .max(self.after_lines.len())
            .max(1)
            .to_string()
            .len()
    }

    fn side_line(
        &self,
        theme: &Theme,
        after: bool,
        i: Option<usize>,
        width: usize,
        highlight: bool,
        is_cursor: bool,
    ) -> Line<'static> {
        let digits = self.gutter_digits();
        let gutter = 2 + digits + 2;
        let code_width = width.saturating_sub(gutter);
        let dim = style::dim(theme);
        let Some(i) = i else {
            let mut spans: Vec<Span<'static>> =
                vec![" ".repeat(gutter - 1).into(), Span::styled("│", dim)];
            spans.push(Span::styled(" ".repeat(code_width), style::bg(theme.panel)));
            return Line::from(spans);
        };
        let (side, lines, hl) = if after {
            (&self.diff.after, &self.after_lines, &self.after_hl)
        } else {
            (&self.diff.before, &self.before_lines, &self.before_hl)
        };
        let op = side.line_ops.get(i).copied().unwrap_or(Op::None);
        let base_bg = if is_cursor {
            style::color(theme.cursor_bg)
        } else {
            style::color(theme.line_bg(op))
        };
        let mut spans: Vec<Span<'static>> = Vec::new();
        let marker = if after {
            self.comments
                .iter()
                .find(|c| c.covers_row(i))
                .map(|c| Span::styled("●", comment_style(theme, c.state, c.comment.is_pending())))
                .unwrap_or_else(|| " ".into())
        } else {
            " ".into()
        };
        spans.push(marker);
        spans.push(Span::styled(op_marker(op), style::fg(theme.op_fg(op))));
        spans.push(Span::styled(format!("{:>digits$} ", i + 1), dim));
        spans.push(Span::styled("│", dim));
        let search = if after {
            self.search
                .as_ref()
                .map(|s| s.ranges_on(i))
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        let text = lines.get(i).cloned().unwrap_or_default();
        let segs = hl
            .get(i)
            .cloned()
            .unwrap_or_else(|| vec![Segment::plain(&text)]);
        let diff_spans = side.spans.get(i).cloned().unwrap_or_default();
        spans.extend(paint(
            theme,
            &segs,
            &diff_spans,
            &search,
            None,
            base_bg,
            highlight,
            self.hscroll,
            code_width,
        ));
        let mut line = Line::from(spans);
        if is_cursor && base_bg.is_none() {
            line = line.style(Style::new().add_modifier(Modifier::REVERSED));
        }
        line
    }

    /// Side-by-side: two columns of `height` lines each.
    pub fn render_columns(
        &self,
        theme: &Theme,
        col_width: usize,
        height: usize,
        highlight: bool,
    ) -> (Vec<Line<'static>>, Vec<Line<'static>>) {
        let mut left = Vec::with_capacity(height);
        let mut right = Vec::with_capacity(height);
        for (ri, r) in self.rows.iter().enumerate().skip(self.scroll).take(height) {
            let is_cursor = ri == self.cursor;
            left.push(self.side_line(theme, false, r.before, col_width, highlight, is_cursor));
            right.push(self.side_line(theme, true, r.after, col_width, highlight, is_cursor));
        }
        (left, right)
    }

    /// Unified: one column. Within a hunk, every before line comes first, then every after
    /// line, the way `diff -u` reads.
    pub fn render_unified(
        &self,
        theme: &Theme,
        width: usize,
        height: usize,
        highlight: bool,
    ) -> Vec<Line<'static>> {
        let mut out = Vec::with_capacity(height);
        let mut ri = self.scroll;
        while ri < self.rows.len() && out.len() < height {
            let r = self.rows[ri];
            if !self.row_is_change(&r) {
                if let Some(a) = r.after {
                    out.push(self.side_line(
                        theme,
                        true,
                        Some(a),
                        width,
                        highlight,
                        ri == self.cursor,
                    ));
                } else if let Some(b) = r.before {
                    out.push(self.side_line(
                        theme,
                        false,
                        Some(b),
                        width,
                        highlight,
                        ri == self.cursor,
                    ));
                }
                ri += 1;
                continue;
            }
            // A hunk: the run of changed rows starting here.
            let start = ri;
            let mut end = ri;
            while end < self.rows.len() && self.row_is_change(&self.rows[end]) {
                end += 1;
            }
            for (k, r) in self.rows[start..end].iter().enumerate() {
                if let Some(b) = r.before {
                    if out.len() < height {
                        out.push(self.side_line(
                            theme,
                            false,
                            Some(b),
                            width,
                            highlight,
                            start + k == self.cursor,
                        ));
                    }
                }
            }
            for (k, r) in self.rows[start..end].iter().enumerate() {
                if let Some(a) = r.after {
                    if out.len() < height {
                        out.push(self.side_line(
                            theme,
                            true,
                            Some(a),
                            width,
                            highlight,
                            start + k == self.cursor,
                        ));
                    }
                }
            }
            ri = end;
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hunks() {
        let ops = [
            Op::None,
            Op::Insert,
            Op::Insert,
            Op::None,
            Op::Update,
            Op::None,
        ];
        assert_eq!(next_hunk(&ops, 0, true), Some(1));
        assert_eq!(next_hunk(&ops, 1, true), Some(4));
        assert_eq!(next_hunk(&ops, 4, true), Some(1));
        assert_eq!(next_hunk(&ops, 4, false), Some(1));
        assert_eq!(next_hunk(&ops, 3, false), Some(1));
        assert_eq!(next_hunk(&ops, 1, false), Some(4));
        assert_eq!(next_hunk(&[Op::None], 0, true), None);
    }

    #[test]
    fn clip_scrolls_and_expands_tabs() {
        let spans = vec![Span::from("\tab"), Span::from("cdef")];
        let out = clip(spans.clone(), 0, 6);
        let text: String = out.iter().map(|s| s.content.to_string()).collect();
        assert_eq!(text, "    a…");
        let out = clip(spans, 3, 20);
        let text: String = out.iter().map(|s| s.content.to_string()).collect();
        assert_eq!(text, "…abcdef");
    }

    #[test]
    fn search_smart_case() {
        let lines = vec!["Foo foo".to_string(), "bar".to_string()];
        let s = Search::new("foo", &lines);
        assert_eq!(s.matches.len(), 2);
        let s = Search::new("Foo", &lines);
        assert_eq!(s.matches, vec![(0, 0, 3)]);
        assert_eq!(s.next_line(0, true), Some(0));
    }

    #[test]
    fn column_motions() {
        let view = FileView {
            path: "f.rs".into(),
            text: "let total_x = compute(items);\n\tshort\n".into(),
            binary: false,
            comments: vec![],
            notes: vec![],
        };
        let mut v = Viewer::new(view, "ansi");
        v.code_width = 12;
        v.word(true);
        assert_eq!(v.column(), 4);
        v.word(true);
        assert_eq!(v.column(), 14);
        v.word(true);
        assert_eq!(v.column(), 22);
        assert!(
            v.hscroll > 0,
            "the view scrolled to keep the cursor visible"
        );
        v.word(false);
        assert_eq!(v.column(), 14);
        v.col_end();
        assert_eq!(v.column(), 28);
        v.move_col(5);
        assert_eq!(v.column(), 28);
        v.col_home();
        assert_eq!(v.hscroll, 0);
        v.move_col(-1);
        assert_eq!(v.column(), 0);
        v.move_lines(1);
        v.col_first_nonblank();
        assert_eq!(v.column(), 1);
        assert_eq!(display_width("\tab"), 6);
    }

    #[test]
    fn viewer_rows_include_comments() {
        let mut c = crate::review::Comment::new("f", 2, 2, "hm");
        c.anchor = Some("b".into());
        let view = FileView {
            path: "f".into(),
            text: "a\nb\nc\n".into(),
            binary: false,
            comments: crate::anchor::anchor_all(&[&c], "a\nb\nc\n"),
            notes: vec![],
        };
        let mut v = Viewer::new(view, "ansi");
        assert_eq!(
            v.rows,
            vec![Row::Code(0), Row::Code(1), Row::Comment(0), Row::Code(2)]
        );
        v.go_to_line(2);
        assert_eq!(v.cursor, 3);
        v.next_comment(false);
        assert!(v.on_comment_row());
        assert_eq!(v.current_line(), Some(1));
        let lines = v.render(&Theme::default(), 40, 10, true, true);
        assert_eq!(lines.len(), 4);
    }

    #[test]
    fn whole_file_comment_heads_the_file() {
        let c = crate::review::Comment::on_path("f", "needs tests");
        let view = FileView {
            path: "f".into(),
            text: "a\nb\n".into(),
            binary: false,
            comments: crate::anchor::anchor_all(&[&c], "a\nb\n"),
            notes: vec![],
        };
        let v = Viewer::new(view, "ansi");
        assert_eq!(v.rows, vec![Row::Comment(0), Row::Code(0), Row::Code(1)]);
        assert!(v.on_comment_row());
        assert_eq!(v.current_line(), None);
        assert_eq!(v.selection(), None);
    }
}
