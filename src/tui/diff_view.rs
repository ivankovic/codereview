//! The diff viewer: two versions of one file, side by side or one above the other, with
//! comments woven in as the file viewer does. It shares the painting and the search helpers
//! with its neighbour in `viewer`.

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::anchor::Anchored;
use crate::diff::{AlignedRow, FileDiff, Op};
use crate::highlight::Segment;
use crate::repo::ChangedFile;
use crate::session::DiffTarget;
use crate::theme::Theme;
use crate::tui::style::{self, comment_style, op_marker};
use crate::tui::viewer::{Search, next_hunk, paint};

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

    /// Puts the cursor on the row showing after-side line `line`. False when the diff has
    /// no such row.
    pub fn go_to_after_line(&mut self, line: usize) -> bool {
        match self.rows.iter().position(|r| r.after == Some(line)) {
            Some(i) => {
                self.cursor = i;
                true
            }
            None => false,
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

    /// A diff view, built the way the app builds one.
    fn diff_view(before: &str, after: &str) -> DiffView {
        let diff = crate::diff::FileDiff::compute(std::path::Path::new("a.rs"), before, after);
        DiffView::new(
            crate::session::DiffTarget::Working,
            crate::repo::ChangedFile {
                status: crate::repo::FileStatus::Modified,
                path: "a.rs".into(),
                old_path: None,
            },
            diff,
            Vec::new(),
            "ansi",
        )
    }

    /// Opening a diff puts the cursor on the first change, and the hunk keys walk the rest.
    #[test]
    fn a_diff_opens_on_the_first_change_and_walks_the_hunks() {
        let before = "one\ntwo\nthree\nfour\nfive\nsix\nseven\n";
        let after = "one\nTWO\nthree\nfour\nfive\nSIX\nseven\n";
        let mut view = diff_view(before, after);
        let (hunks, at) = view.hunk_position();
        assert_eq!(hunks, 2, "two lines changed, apart");
        assert_eq!(at, Some(1), "the cursor starts on the first");
        assert_eq!(
            view.after_line(),
            Some(1),
            "the second line, counting from 0"
        );

        view.next_change(true, false);
        assert_eq!(view.hunk_position().1, Some(2));
        assert_eq!(view.after_line(), Some(5));
        // Forward from the last wraps to the first.
        view.next_change(true, false);
        assert_eq!(view.hunk_position().1, Some(1));
        view.next_change(false, false);
        assert_eq!(view.hunk_position().1, Some(2), "backwards wraps too");

        // Jumping to a line by number lands on it.
        assert!(view.go_to_after_line(3));
        assert_eq!(view.after_line(), Some(3));
        assert!(!view.go_to_after_line(999), "no such line");
    }

    /// The unified layout puts a hunk's before lines above its after lines, rather than
    /// interleaving them, and shows every line of both sides exactly once.
    #[test]
    fn the_unified_layout_groups_each_hunk() {
        let before = "keep\nold one\nold two\ntail\n";
        let after = "keep\nnew one\ntail\n";
        let view = diff_view(before, after);
        let theme = crate::theme::Theme::default();
        let rows = view.render_unified(&theme, 80, 100, true);
        let text: Vec<String> = rows
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect();
        let joined = text.join("\n");
        for line in ["keep", "old one", "old two", "new one", "tail"] {
            assert_eq!(
                text.iter().filter(|l| l.contains(line)).count(),
                1,
                "{line} should appear once: {joined}"
            );
        }
        let at = |needle: &str| text.iter().position(|l| l.contains(needle)).unwrap();
        assert!(
            at("old one") < at("new one"),
            "before comes first: {joined}"
        );
        assert!(at("old two") < at("new one"), "{joined}");
        assert!(at("new one") < at("tail"), "{joined}");
    }
}
