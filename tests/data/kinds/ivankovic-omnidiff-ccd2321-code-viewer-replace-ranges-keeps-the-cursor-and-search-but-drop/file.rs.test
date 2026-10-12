/*  This file is part of the OmniDiff code diffing tool.
 *
 *  Copyright (C) 2026 Marko Ivankovic
 *
 *  This program is free software: you can redistribute it and/or modify
 *  it under the terms of the GNU Affero General Public License as published
 *  by the Free Software Foundation, either version 3 of the License, or
 *  (at your option) any later version.
 *
 *  This program is distributed in the hope that it will be useful,
 *  but WITHOUT ANY WARRANTY; without even the implied warranty of
 *  MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
 *  GNU Affero General Public License for more details.
 *
 *  You should have received a copy of the GNU Affero General Public License
 *  along with this program.  If not, see <https://www.gnu.org/licenses/>.
 */
use std::path::PathBuf;
use std::sync::OnceLock;

use anyhow::{Context, Result};
use ratatui::{buffer::Buffer, prelude::*, text::Line, widgets::StatefulWidget};
use syntect::highlighting::{Theme, ThemeSet};
use syntect::parsing::{SyntaxReference, SyntaxSet};

use crate::code::language::language_for_path_and_content;
use crate::diff::text::{RangeMatch, TextOperation};
use crate::diff::text_range::TextRange;
use crate::tui::display_columns;
use crate::tui::theme::{OverlayPalette, OverlayTheme};

static SYNTAX_SET: OnceLock<SyntaxSet> = OnceLock::new();

static THEME_SET: OnceLock<ThemeSet> = OnceLock::new();

/// `two-face`'s set (the one `bat` ships), not syntect's defaults, which lack Dart, Kotlin, Swift,
/// TypeScript/TSX and Vimscript. Neither set has Bazel/Starlark.
pub(crate) fn syntax_set() -> &'static SyntaxSet {
    SYNTAX_SET.get_or_init(two_face::syntax::extra_newlines)
}

pub(crate) fn theme_set() -> &'static ThemeSet {
    THEME_SET.get_or_init(ThemeSet::load_defaults)
}

/// Every syntect built-in theme name, sorted: exactly the names `get_theme` can resolve.
pub fn syntax_theme_names() -> Vec<String> {
    let mut names: Vec<String> = theme_set().themes.keys().cloned().collect();
    names.sort();
    names
}

/// The syntax name in `syntax_set`, which is not always the obvious one.
pub(crate) fn language_to_syntect(lang: &crate::code::Language) -> Option<&'static str> {
    use crate::code::Language::*;

    match lang {
        Bazel => None,
        C => Some("C"),
        CPP => Some("C++"),
        CSS => Some("CSS"),
        CSharp => Some("C#"),
        Dart => Some("Dart"),
        Go => Some("Go"),
        HTML => Some("HTML"),
        JSON => Some("JSON"),
        Java => Some("Java"),
        JavaScript => Some("JavaScript"),
        Kotlin => Some("Kotlin"),
        LUA => Some("Lua"),
        Lisp => Some("Lisp"),
        MarkDown => Some("Markdown"),
        PHP => Some("PHP"),
        Perl => Some("Perl"),
        ProtoBuf => Some("Protocol Buffer"),
        Python => Some("Python"),
        R => Some("R"),
        Ruby => Some("Ruby"),
        Rust => Some("Rust"),
        SQL => Some("SQL"),
        Scala => Some("Scala"),
        ShellScript => Some("Bourne Again Shell (bash)"),
        Swift => Some("Swift"),
        TSX => Some("TypeScriptReact"),
        TypeScript => Some("TypeScript"),
        Vimscript => Some("VimL"),
        YAML => Some("YAML"),
        XML => Some("XML"),
        Unknown => None,
    }
}

/// The syntax theme until the user picks one: one of syntect's bundled themes, and a dark one,
/// since most terminals are.
pub const DEFAULT_SYNTAX_THEME: &str = "base16-ocean.dark";

fn syntect_color_to_ratatui(color: syntect::highlighting::Color) -> ratatui::style::Color {
    ratatui::style::Color::Rgb(color.r, color.g, color.b)
}

/// `None` for `Identical`/`NotYetSet`, which keep plain syntax highlighting.
fn background_for_operation(operation: &TextOperation, palette: &OverlayPalette) -> Option<Color> {
    palette.background_for(operation)
}

/// Indices into `ranges` sorted by source start, then end, so a zero-width marker sorts before a
/// real range sharing its start and `range_at` finds the real one.
fn build_range_order(ranges: &[RangeMatch]) -> Vec<usize> {
    let mut order: Vec<usize> = (0..ranges.len()).collect();
    order.sort_by_key(|&i| {
        let r = &ranges[i].source;
        (r.start_row, r.start_column, r.end_row, r.end_column)
    });
    order
}

/// The range whose source covers `(row, col)`. Relies on one side's ranges never overlapping;
/// zero-width ranges never match.
fn range_at(ranges: &[RangeMatch], order: &[usize], row: usize, col: usize) -> Option<usize> {
    let split = order.partition_point(|&i| {
        let r = &ranges[i].source;
        (r.start_row, r.start_column) <= (row, col)
    });
    let candidate = *order[..split].last()?;
    let r = &ranges[candidate].source;
    ((row, col) < (r.end_row, r.end_column)).then_some(candidate)
}

/// The nearest of the sorted `positions` strictly after (or before) `cursor`, wrapping.
fn next_position(
    positions: &[(usize, usize)],
    cursor: (usize, usize),
    forward: bool,
) -> Option<(usize, usize)> {
    if forward {
        positions
            .iter()
            .find(|&&pos| pos > cursor)
            .or_else(|| positions.first())
            .copied()
    } else {
        positions
            .iter()
            .rev()
            .find(|&&pos| pos < cursor)
            .or_else(|| positions.last())
            .copied()
    }
}

/// `(index, total)`: how many `positions` sit at or before `cursor` (at least 1), and how many
/// there are. `None` if there are none.
fn count_and_index(positions: &[(usize, usize)], cursor: (usize, usize)) -> Option<(usize, usize)> {
    if positions.is_empty() {
        return None;
    }
    let index = positions
        .iter()
        .filter(|&&pos| pos <= cursor)
        .count()
        .max(1);
    Some((index, positions.len()))
}

/// `line`'s byte length without trailing whitespace: the `row_len` a range that spans the row
/// paints up to. The hand-painted ground truth never marks trailing whitespace (see
/// `human_solver`'s `span_covers`), so the rendering does not either.
fn trailing_whitespace_trimmed_len(line: &Line<'_>) -> crate::diff::text_range::SourceColumn {
    // Bytes, like every `TextRange` column (see `text_range::SourceColumn`).
    let total: usize = line.spans.iter().map(|s| s.content.len()).sum();
    let trailing_whitespace: usize = line
        .spans
        .iter()
        .rev()
        .flat_map(|span| span.content.chars().rev())
        .take_while(|c| c.is_whitespace())
        .map(char::len_utf8)
        .sum();
    crate::diff::text_range::SourceColumn::from_raw(total - trailing_whitespace)
}

/// Patch `style` onto the `[start_col, end_col)` byte range of `line`, keeping the existing
/// styling outside and underneath it.
fn paint_columns(
    line: &Line<'static>,
    start_col: usize,
    end_col: usize,
    style: Style,
) -> Line<'static> {
    let mut spans = Vec::with_capacity(line.spans.len() + 2);
    let mut col = 0usize;

    for span in &line.spans {
        let text: &str = span.content.as_ref();
        let span_len = text.len();
        let span_start = col;
        let span_end = col + span_len;
        col = span_end;

        if span_end <= start_col || span_start >= end_col {
            spans.push(span.clone());
            continue;
        }

        // A column landing inside a multi-byte character splits before it rather than panicking
        // the slice.
        let boundary = |index: usize| crate::diff::text_range::floor_char_boundary(text, index);
        let local_start = boundary(start_col.saturating_sub(span_start));
        let local_end = boundary(end_col.saturating_sub(span_start));

        if local_start > 0 {
            spans.push(Span::styled(text[0..local_start].to_string(), span.style));
        }
        if local_end > local_start {
            spans.push(Span::styled(
                text[local_start..local_end].to_string(),
                span.style.patch(style),
            ));
        }
        if local_end < span_len {
            spans.push(Span::styled(text[local_end..].to_string(), span.style));
        }
    }

    Line::from(spans)
}

/// Scroll, viewport, cursor and the diff overlay's inputs.
#[derive(Default, Clone)]
pub struct CodeViewerState {
    pub scroll: usize,
    /// Horizontal scroll, in display columns (see `tui::display_columns`).
    pub scroll_col: usize,
    pub viewport_height: usize,
    /// Visible content columns, in display columns, excluding the gutter. Set by `render`, the
    /// only place the width is known; 0 until the first frame means "unknown, don't scroll".
    pub viewport_width: usize,
    pub ranges: Vec<RangeMatch>,
    /// See `build_range_order`; must be rebuilt whenever `ranges` changes.
    range_order: Vec<usize>,
    pub cursor_row: usize,
    /// In bytes, the unit of every range and search hit it is compared with. `CodeViewer`
    /// converts it to a display column for the screen.
    pub cursor_col: usize,
    /// The cross-highlight pushed from the other panel's cursor.
    pub highlight_destination: Option<TextRange>,
    /// Whether this side's cursor drives navigation. The focused side highlights the node under
    /// its own cursor, the unfocused side only `highlight_destination`: never both, and never an
    /// unfocused side's stale cursor.
    pub is_focused: bool,
    /// Empty when no search is active.
    pub search_matches: Vec<TextRange>,
    /// The `H` toggle, off by default (as is `theme::load_node_highlight`): it repaints on every
    /// move and covers the diff color, so on by default reads as interference. Gates painting
    /// only, never cursor following.
    pub node_highlight: bool,
}

impl CodeViewerState {
    /// Load a new diff: place the cursor on the first non-zero-width range and clear the
    /// cross-highlight and search.
    pub fn load_ranges(&mut self, ranges: Vec<RangeMatch>) {
        self.ranges = ranges;
        self.range_order = build_range_order(&self.ranges);
        let first_navigable = self
            .range_order
            .iter()
            .map(|&i| &self.ranges[i])
            .find(|range_match| !range_match.source.is_empty());
        let (row, col) = first_navigable
            .map(|range_match| {
                (
                    range_match.source.start_row,
                    range_match.source.start_column,
                )
            })
            .unwrap_or((0, 0));
        self.cursor_row = row;
        self.cursor_col = col;
        self.highlight_destination = None;
        self.search_matches = Vec::new();
    }

    /// Swap in a repainting of the same diff (e.g. `M`) without moving the cursor, which
    /// `load_ranges` would send back to the first change.
    pub fn replace_ranges(&mut self, ranges: Vec<RangeMatch>, line_count: usize) {
        self.ranges = ranges;
        self.range_order = build_range_order(&self.ranges);
        self.cursor_row = self.cursor_row.min(line_count.saturating_sub(1));
        // Search hits index text, not ranges, so they stay valid.
        self.highlight_destination = None;
    }

    fn search_positions(&self) -> Vec<(usize, usize)> {
        self.search_matches
            .iter()
            .map(|range| (range.start_row, range.start_column))
            .collect()
    }

    /// The match Enter jumps to: at or after the cursor, wrapping. Unlike `>`, a match under the
    /// cursor counts, since this is a fresh search's first jump.
    pub fn nearest_search_match_position(&self) -> Option<(usize, usize)> {
        let cursor = (self.cursor_row, self.cursor_col);
        let positions = self.search_positions();
        positions
            .iter()
            .find(|&&pos| pos >= cursor)
            .or_else(|| positions.first())
            .copied()
    }

    /// The match `>`/`<` jump to: strictly after (or before) the cursor, wrapping.
    pub fn next_search_match_position(&self, forward: bool) -> Option<(usize, usize)> {
        next_position(
            &self.search_positions(),
            (self.cursor_row, self.cursor_col),
            forward,
        )
    }

    /// The footer's "match N/M"; see `count_and_index`.
    pub fn search_match_count_and_index(&self) -> Option<(usize, usize)> {
        count_and_index(&self.search_positions(), (self.cursor_row, self.cursor_col))
    }

    fn range_at_cursor(&self) -> Option<usize> {
        range_at(
            &self.ranges,
            &self.range_order,
            self.cursor_row,
            self.cursor_col,
        )
    }

    /// The range the other panel's cursor follows, whatever the operation: following tracks the
    /// cursor over unchanged content too.
    pub fn cursor_destination(&self) -> Option<TextRange> {
        self.range_at_cursor()
            .map(|i| self.ranges[i].destination.clone())
    }

    /// Like `cursor_destination`, but `None` for an `Identical` match, which is never highlighted
    /// on either side.
    pub fn cursor_destination_for_highlight(&self) -> Option<TextRange> {
        let i = self.range_at_cursor()?;
        if self.ranges[i].operation == TextOperation::Identical {
            return None;
        }
        Some(self.ranges[i].destination.clone())
    }
}

/// File contents with syntax highlighting and the diff/cursor overlay. Highlighting is cached per
/// file, since re-highlighting per frame is too slow for scrolling; the overlay is painted fresh.
#[derive(Clone)]
pub struct CodeViewerWidget {
    file_path: Option<PathBuf>,
    contents: String,
    language: Option<crate::code::Language>,
    theme_name: Option<String>,
    syntax_highlighting: bool,
    /// Rebuilt whenever the content, language, theme, or highlighting toggle changes.
    highlighted_lines: Vec<Line<'static>>,
    /// The diff/cursor overlay palette, distinct from the syntax theme.
    overlay_theme: OverlayTheme,
}

impl Default for CodeViewerWidget {
    // Hand-written only so `syntax_highlighting` defaults to `true`.
    fn default() -> Self {
        Self {
            file_path: None,
            contents: String::new(),
            language: None,
            theme_name: None,
            syntax_highlighting: true,
            highlighted_lines: Vec::new(),
            overlay_theme: OverlayTheme::default(),
        }
    }
}

impl CodeViewerWidget {
    pub fn load_file(&mut self, path: PathBuf) -> Result<()> {
        let contents = crate::code::read_text(&path)
            .with_context(|| format!("Failed to read file: {:?}", path))?;
        self.language = language_for_path_and_content(&path, &contents);
        self.file_path = Some(path);
        self.contents = contents;
        self.rebuild_highlight_cache();
        Ok(())
    }

    /// Load contents read elsewhere, so the UI thread does not redo a blocking read.
    pub fn load_contents(&mut self, path: PathBuf, contents: String) {
        self.language = language_for_path_and_content(&path, &contents);
        self.file_path = Some(path);
        self.contents = contents;
        self.rebuild_highlight_cache();
    }

    #[cfg(test)]
    pub fn with_contents(mut self, contents: String) -> Self {
        self.contents = contents;
        self.rebuild_highlight_cache();
        self
    }

    pub fn enable_syntax_highlighting(&mut self) {
        self.syntax_highlighting = true;
        self.rebuild_highlight_cache();
    }

    pub fn disable_syntax_highlighting(&mut self) {
        self.syntax_highlighting = false;
        self.rebuild_highlight_cache();
    }

    pub fn is_syntax_highlighting_enabled(&self) -> bool {
        self.syntax_highlighting
    }

    /// An unknown name falls back to the default theme.
    pub fn set_theme(&mut self, theme_name: String) {
        self.theme_name = Some(theme_name);
        self.rebuild_highlight_cache();
    }

    /// No cache rebuild: the overlay is painted fresh every frame.
    pub fn set_overlay_theme(&mut self, theme: OverlayTheme) {
        self.overlay_theme = theme;
    }

    pub fn line_count(&self) -> usize {
        self.highlighted_lines.len()
    }

    /// Bytes on `row`, or 0 out of bounds.
    pub fn line_len(&self, row: usize) -> usize {
        self.highlighted_lines
            .get(row)
            .map(|line| line.spans.iter().map(|s| s.content.len()).sum())
            .unwrap_or(0)
    }

    /// The plain text of `row`, or empty out of bounds.
    pub fn line_text(&self, row: usize) -> String {
        self.highlighted_lines
            .get(row)
            .map(|line| line.spans.iter().map(|s| s.content.as_ref()).collect())
            .unwrap_or_default()
    }

    /// Every single-line occurrence of `query`, in document order, with byte columns. Empty for
    /// an empty query. Smart-case: a query with any uppercase character matches exactly.
    ///
    /// Folds char by char against the original line rather than lowercasing the line, because
    /// `to_lowercase` is not length-preserving ('İ') and offsets in a lowercased copy drift.
    pub fn find_matches(&self, query: &str) -> Vec<TextRange> {
        if query.is_empty() {
            return Vec::new();
        }
        let case_sensitive = query.chars().any(char::is_uppercase);
        let query_chars: Vec<char> = if case_sensitive {
            query.chars().collect()
        } else {
            query.chars().flat_map(char::to_lowercase).collect()
        };
        let query_len = query_chars.len();
        let mut matches = Vec::new();
        for (row, line) in self.contents.lines().enumerate() {
            let starts: Vec<usize> = line.char_indices().map(|(index, _)| index).collect();
            let chars: Vec<char> = line.chars().collect();
            if chars.len() < query_len {
                continue;
            }
            'starts: for start in 0..=(chars.len() - query_len) {
                for (offset, &query_char) in query_chars.iter().enumerate() {
                    let candidate = chars[start + offset];
                    let matched = if case_sensitive {
                        candidate == query_char
                    } else {
                        let mut lowered = candidate.to_lowercase();
                        lowered.next() == Some(query_char) && lowered.next().is_none()
                    };
                    if !matched {
                        continue 'starts;
                    }
                }
                let start_byte = starts[start];
                let end_byte = starts.get(start + query_len).copied().unwrap_or(line.len());
                matches.push(TextRange::new(row, start_byte, row, end_byte));
            }
        }
        matches
    }

    fn get_syntax(&self) -> Option<&'static SyntaxReference> {
        let lang_name = self.language.as_ref()?;
        let syntect_name = language_to_syntect(lang_name)?;
        syntax_set().find_syntax_by_name(syntect_name)
    }

    /// `theme_name`, or [`DEFAULT_SYNTAX_THEME`] if unset or unknown: `set_theme` does not
    /// validate.
    fn get_theme(&self) -> Theme {
        let theme_set = theme_set();
        let theme_name = self.theme_name.as_deref().unwrap_or(DEFAULT_SYNTAX_THEME);
        theme_set
            .themes
            .get(theme_name)
            .or_else(|| theme_set.themes.get(DEFAULT_SYNTAX_THEME))
            .expect("base16-ocean.dark is one of syntect's own bundled default themes")
            .clone()
    }

    /// The only place highlighting runs; call it on a change of input, never per frame.
    fn rebuild_highlight_cache(&mut self) {
        let lines: Vec<&str> = self.contents.lines().collect();

        self.highlighted_lines = if self.syntax_highlighting {
            self.highlight_lines(&lines).unwrap_or_else(|_| {
                lines
                    .iter()
                    .map(|&line| Line::from(line.to_string()))
                    .collect()
            })
        } else {
            lines
                .iter()
                .map(|&line| Line::from(line.to_string()))
                .collect()
        };
    }

    fn highlight_lines(&self, lines: &[&str]) -> Result<Vec<Line<'static>>> {
        let syntax = match self.get_syntax() {
            Some(s) => s,
            None => {
                return Ok(lines
                    .iter()
                    .map(|&line| Line::from(line.to_string()))
                    .collect());
            }
        };

        let theme = self.get_theme();
        let mut highlighter = syntect::easy::HighlightLines::new(syntax, &theme);
        let mut result = Vec::with_capacity(lines.len());

        for &line in lines {
            let regions: Vec<(syntect::highlighting::Style, &str)> =
                highlighter.highlight_line(line, syntax_set())?;

            let spans: Vec<Span> = regions
                .into_iter()
                .map(|(style, text)| {
                    let color = syntect_color_to_ratatui(style.foreground);
                    Span::styled(text.to_string(), Style::new().fg(color))
                })
                .collect();

            result.push(Line::from(spans));
        }

        Ok(result)
    }

    pub fn visible_lines(&self, state: &CodeViewerState) -> Vec<Line<'static>> {
        let total_lines = self.highlighted_lines.len();

        if total_lines == 0 {
            return vec![Line::from("")];
        }

        let scroll = state.scroll.min(total_lines.saturating_sub(1));
        let start = scroll;
        let end = std::cmp::min(start + state.viewport_height, total_lines);

        (start..end)
            .map(|row| self.overlay_row(row, state))
            .collect()
    }

    fn overlay_row(&self, row: usize, state: &CodeViewerState) -> Line<'static> {
        let palette = self.overlay_theme.palette();
        let mut line = self.highlighted_lines[row].clone();
        let row_len = trailing_whitespace_trimmed_len(&line);
        let cursor_range = range_at(
            &state.ranges,
            &state.range_order,
            state.cursor_row,
            state.cursor_col,
        );

        for (index, range_match) in state.ranges.iter().enumerate() {
            let Some((start_col, end_col)) = range_match.source.columns_on_row(row, row_len) else {
                continue;
            };

            if let Some(bg) = background_for_operation(&range_match.operation, &palette) {
                line = paint_columns(
                    &line,
                    start_col,
                    end_col,
                    Style::new().fg(palette.overlay_fg).bg(bg),
                );
            }

            // Same blue as the counterpart below: one signal on two panels. Focused side only,
            // since an unfocused cursor is stale; never on unchanged content.
            if state.node_highlight
                && state.is_focused
                && cursor_range == Some(index)
                && range_match.operation != TextOperation::Identical
            {
                line = paint_columns(
                    &line,
                    start_col,
                    end_col,
                    Style::new()
                        .fg(palette.overlay_fg)
                        .bg(palette.cross_highlight_bg),
                );
            }
        }

        // Not the cross-highlight blue, or a hit and the cursor's counterpart look alike.
        for search_match in &state.search_matches {
            if let Some((start_col, end_col)) = search_match.columns_on_row(row, row_len) {
                line = paint_columns(
                    &line,
                    start_col,
                    end_col,
                    Style::new().fg(palette.overlay_fg).bg(palette.search_bg),
                );
            }
        }

        // Unfocused side only, so focus never shows both blues. `Identical` is already filtered
        // by `cursor_destination_for_highlight`.
        if state.node_highlight
            && !state.is_focused
            && let Some(destination) = &state.highlight_destination
            && let Some((start_col, end_col)) = destination.columns_on_row(row, row_len)
        {
            line = paint_columns(
                &line,
                start_col,
                end_col,
                Style::new()
                    .fg(palette.overlay_fg)
                    .bg(palette.cross_highlight_bg),
            );
        }

        line
    }

    /// The widest line number plus a separator space; 0 when nothing is loaded.
    pub fn gutter_width(&self) -> usize {
        if self.highlighted_lines.is_empty() {
            return 0;
        }
        self.highlighted_lines.len().to_string().len() + 1
    }

    pub fn has_file(&self) -> bool {
        self.file_path.is_some()
    }

    pub fn filename(&self) -> String {
        self.file_path
            .as_ref()
            .map(|p| {
                p.file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default()
            })
            .unwrap_or_else(|| "Untitled".to_string())
    }

    /// As a person writes it (`C++`, not the variant's `CPP`), for the panel title.
    pub fn language_name(&self) -> String {
        self.language
            .map(crate::code::language::human_name)
            .unwrap_or_else(|| "Plain Text".to_string())
    }
}

/// Gutter and `…` markers. Chrome, not a diff signal, so not in `OverlayPalette`; `DarkGray`
/// reads as dimmed on light and dark terminals.
const GUTTER_STYLE: Style = Style::new().fg(Color::DarkGray);

/// The `width`-column window of `line` from display column `from`, keeping span styling, with
/// every tab expanded to spaces (see `tui::display_columns`). A cut edge shows a dimmed `…` in
/// place of its outermost column. A tab or wide character the window cuts through shows as
/// spaces for the part inside it, so everything after it stays in its display column.
fn slice_columns(line: &Line<'static>, from: usize, width: usize) -> Line<'static> {
    if width == 0 {
        return Line::from("");
    }
    let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
    let total = display_columns::display_width(&text);
    let to = from + width;
    let mut out: Vec<Span<'static>> = Vec::new();

    if from > 0 {
        out.push(Span::styled("…", GUTTER_STYLE));
    }
    let content_from = if from > 0 { from + 1 } else { from };
    let content_to = if total > to { to - 1 } else { to };

    let mut column = 0usize;
    // Whether the last character with a width was drawn as itself, so a zero-width one after it
    // (a combining mark) goes with it.
    let mut previous_shown = false;
    for span in &line.spans {
        if column >= content_to {
            break;
        }
        let mut content = String::new();
        for ch in span.content.chars() {
            let start = column;
            let char_width = display_columns::char_display_width(ch, start);
            column += char_width;
            if char_width == 0 {
                if previous_shown {
                    content.push(ch);
                }
                continue;
            }
            previous_shown = ch != '\t' && start >= content_from && column <= content_to;
            let visible = column
                .min(content_to)
                .saturating_sub(start.max(content_from));
            if previous_shown {
                content.push(ch);
            } else {
                content.extend(std::iter::repeat_n(' ', visible));
            }
        }
        if !content.is_empty() {
            out.push(Span::styled(content, span.style));
        }
    }
    if total > to {
        out.push(Span::styled("…", GUTTER_STYLE));
    }
    Line::from(out)
}

impl StatefulWidget for &CodeViewerWidget {
    type State = CodeViewerState;

    fn render(self, area: Rect, buf: &mut Buffer, state: &mut Self::State) {
        // No border or title: `DiffViewer` draws the title line above.
        let inner = area;

        let gutter_width = self.gutter_width();
        let content_width = (inner.width as usize).saturating_sub(gutter_width);
        state.viewport_width = content_width;

        let lines = self.visible_lines(state);

        if lines.is_empty() {
            buf.set_line(inner.x, inner.y, &Line::from(""), inner.width);
        } else {
            for (i, line) in lines.iter().enumerate() {
                let y = inner.y + i as u16;
                if y >= inner.y + inner.height {
                    continue;
                }
                let mut composed: Vec<Span<'static>> = Vec::new();
                if gutter_width > 0 {
                    composed.push(Span::styled(
                        format!("{:>w$} ", state.scroll + i + 1, w = gutter_width - 1),
                        GUTTER_STYLE,
                    ));
                }
                composed.extend(slice_columns(line, state.scroll_col, content_width).spans);
                buf.set_line(inner.x, y, &Line::from(composed), inner.width);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff::text::RangeMatch;

    fn widget_with_line(text: &str) -> CodeViewerWidget {
        CodeViewerWidget::default().with_contents(text.to_string())
    }

    #[test]
    fn syntax_highlighting_is_enabled_by_default_and_actually_highlights() {
        let mut widget = CodeViewerWidget::default();
        assert!(widget.is_syntax_highlighting_enabled());

        widget.load_contents(
            PathBuf::from("test.rs"),
            "fn main() { let x = 1; }".to_string(),
        );
        let line = &widget.highlighted_lines[0];
        assert!(
            line.spans.len() > 1,
            "a real Rust line should be split into multiple differently-styled spans by syntax \
             highlighting, got {} span(s): {:?}",
            line.spans.len(),
            line.spans
        );
    }

    /// A wrong syntax name fails silently as unstyled text, so every name is checked.
    #[test]
    fn every_language_except_the_documented_bazel_gap_resolves_to_a_real_syntax() {
        use crate::code::Language;

        // Listed by hand: `Language` has no iterator.
        let languages = [
            Language::Bazel,
            Language::C,
            Language::CPP,
            Language::CSS,
            Language::CSharp,
            Language::Dart,
            Language::Go,
            Language::HTML,
            Language::JSON,
            Language::Java,
            Language::JavaScript,
            Language::Kotlin,
            Language::LUA,
            Language::Lisp,
            Language::MarkDown,
            Language::PHP,
            Language::Perl,
            Language::ProtoBuf,
            Language::Python,
            Language::R,
            Language::Ruby,
            Language::Rust,
            Language::SQL,
            Language::Scala,
            Language::ShellScript,
            Language::Swift,
            Language::TSX,
            Language::TypeScript,
            Language::Vimscript,
            Language::YAML,
            Language::XML,
            Language::Unknown,
        ];

        for language in languages {
            let widget = CodeViewerWidget {
                language: Some(language),
                ..CodeViewerWidget::default()
            };
            let resolved = widget.get_syntax().is_some();
            if matches!(language, Language::Bazel | Language::Unknown) {
                assert!(
                    !resolved,
                    "{language:?} was expected to still have no syntax definition"
                );
            } else {
                assert!(resolved, "{language:?} should resolve to a real syntax");
            }
        }
    }

    #[test]
    fn set_theme_with_an_unknown_name_falls_back_instead_of_panicking() {
        let mut widget = CodeViewerWidget::default();
        widget.set_theme("this-theme-does-not-exist".to_string());
        widget.load_contents(PathBuf::from("test.rs"), "fn main() {}".to_string());
        assert!(!widget.highlighted_lines.is_empty());
    }

    #[test]
    fn find_matches_finds_case_insensitive_occurrences_in_document_order() {
        let widget = widget_with_line("Hello world\nworld hello WORLD\n");
        let matches = widget.find_matches("world");
        assert_eq!(
            matches,
            vec![
                TextRange::new(0, 6, 0, 11),
                TextRange::new(1, 0, 1, 5),
                TextRange::new(1, 12, 1, 17),
            ]
        );
    }

    #[test]
    fn find_matches_is_empty_for_an_empty_query_or_no_occurrences() {
        let widget = widget_with_line("hello world\n");
        assert_eq!(widget.find_matches(""), Vec::new());
        assert_eq!(widget.find_matches("xyz"), Vec::new());
    }

    /// 'İ' lowercases to two characters, so offsets found in a lowercased copy would drift.
    #[test]
    fn find_matches_columns_are_correct_when_lowercasing_changes_character_count() {
        let widget = widget_with_line("İ world\n");
        // Byte columns: 'İ' is two bytes.
        assert_eq!(
            widget.find_matches("world"),
            vec![TextRange::new(0, 3, 0, 8)]
        );
    }

    /// Asserted on the text the columns select, since a range is only right relative to the unit
    /// its reader assumes.
    #[test]
    fn a_search_highlight_covers_the_query_on_a_non_ascii_row() {
        for (label, line, query) in [
            ("ascii", "let a = world;", "world"),
            ("two-byte", "let é = world;", "world"),
            ("three-byte", "let 漢 = world;", "world"),
            ("wide-run", "漢漢漢 world", "world"),
        ] {
            let widget = widget_with_line(&format!("{line}\n"));
            let matches = widget.find_matches(query);
            assert_eq!(matches.len(), 1, "{label}: expected one match");

            let row_len = crate::diff::text_range::row_len_of(line);
            let (start, end) = matches[0]
                .columns_on_row(0, row_len)
                .expect("the match is on row 0");
            assert_eq!(
                &line[start..end],
                query,
                "{label}: the highlight covers {:?} instead of the query",
                &line[start..end]
            );
        }
    }

    fn default_palette() -> OverlayPalette {
        OverlayTheme::default().palette()
    }

    fn range_match(operation: TextOperation, start_col: usize, end_col: usize) -> RangeMatch {
        RangeMatch {
            source: TextRange::new(0, start_col, 0, end_col),
            destination: TextRange::zero(),
            operation,
        }
    }

    /// Without an explicit foreground, the terminal default can be unreadable on the diff
    /// background (dark background, light-themed terminal).
    #[test]
    fn diff_overlay_pairs_explicit_foreground_with_background() {
        let widget = widget_with_line("hello world");
        let ranges = vec![range_match(TextOperation::Insert, 0, 5)];
        let range_order = build_range_order(&ranges);
        let state = CodeViewerState {
            ranges,
            range_order,
            cursor_row: 0,
            cursor_col: 99,
            viewport_height: 1,
            ..Default::default()
        };

        let line = widget.overlay_row(0, &state);
        let span = &line.spans[0];
        assert_eq!(span.content, "hello");
        let palette = default_palette();
        assert_eq!(span.style.fg, Some(palette.overlay_fg));
        assert_eq!(
            span.style.bg,
            background_for_operation(&TextOperation::Insert, &palette)
        );
    }

    /// Matches the hand-painted ground truth, which never marks trailing whitespace.
    #[test]
    fn multi_row_range_does_not_paint_a_middle_rows_trailing_whitespace() {
        let widget = widget_with_line("foo   \nbar");
        let ranges = vec![RangeMatch {
            source: TextRange::new(0, 0, 1, 3),
            destination: TextRange::zero(),
            operation: TextOperation::Move,
        }];
        let range_order = build_range_order(&ranges);
        let state = CodeViewerState {
            ranges,
            range_order,
            cursor_row: 0,
            cursor_col: 99,
            viewport_height: 2,
            ..Default::default()
        };

        let first_row = widget.overlay_row(0, &state);
        let palette = default_palette();
        let painted_bg = background_for_operation(&TextOperation::Move, &palette);

        assert_eq!(first_row.spans[0].content, "foo");
        assert_eq!(first_row.spans[0].style.bg, painted_bg);
        let trailing: String = first_row.spans[1..]
            .iter()
            .map(|s| s.content.as_ref())
            .collect();
        assert_eq!(trailing, "   ");
        for span in &first_row.spans[1..] {
            assert_ne!(
                span.style.bg, painted_bg,
                "trailing whitespace must not carry the range's background: {first_row:?}"
            );
        }
    }

    /// Off by default is deliberate; see `CodeViewerState::node_highlight`.
    #[test]
    fn node_highlight_is_off_until_enabled() {
        let widget = widget_with_line("hello world");
        let ranges = vec![range_match(TextOperation::Insert, 0, 5)];
        let range_order = build_range_order(&ranges);
        let mut focused_state = CodeViewerState {
            ranges,
            range_order,
            cursor_row: 0,
            cursor_col: 0,
            viewport_height: 1,
            is_focused: true,
            ..Default::default()
        };
        let palette = default_palette();

        let focused_span = &widget.overlay_row(0, &focused_state).spans[0];
        assert_eq!(
            focused_span.style.bg,
            background_for_operation(&TextOperation::Insert, &palette),
            "with the highlight off, the range keeps its own diff color"
        );

        focused_state.node_highlight = true;
        let focused_span = &widget.overlay_row(0, &focused_state).spans[0];
        assert_eq!(
            focused_span.style.bg,
            Some(palette.cross_highlight_bg),
            "once enabled, a real change under the cursor is painted blue as before"
        );
    }

    #[test]
    fn cross_highlight_is_suppressed_for_an_identical_match() {
        let widget = widget_with_line("hello world");
        let ranges = vec![range_match(TextOperation::Identical, 0, 5)];
        let range_order = build_range_order(&ranges);
        let state = CodeViewerState {
            ranges,
            range_order,
            cursor_row: 0,
            cursor_col: 0,
            viewport_height: 1,
            is_focused: true,
            ..Default::default()
        };
        let palette = default_palette();
        assert_ne!(
            widget.overlay_row(0, &state).spans[0].style.bg,
            Some(palette.cross_highlight_bg),
            "the cursor sitting on unchanged content should not be painted blue"
        );
    }

    #[test]
    fn cursor_overlay_uses_bright_blue_with_explicit_foreground() {
        let widget = widget_with_line("hello world");
        let ranges = vec![range_match(TextOperation::Insert, 0, 5)];
        let range_order = build_range_order(&ranges);
        let state = CodeViewerState {
            ranges,
            range_order,
            cursor_row: 0,
            cursor_col: 0,
            viewport_height: 1,
            is_focused: true,
            node_highlight: true,
            ..Default::default()
        };

        let line = widget.overlay_row(0, &state);
        let span = &line.spans[0];
        assert_eq!(span.content, "hello");
        let palette = default_palette();
        assert_eq!(span.style.fg, Some(palette.overlay_fg));
        assert_eq!(span.style.bg, Some(palette.cross_highlight_bg));
    }

    /// An unfocused panel's own cursor is stale, so its node must not stay highlighted.
    #[test]
    fn unfocused_panel_does_not_highlight_its_own_cursor_range() {
        let widget = widget_with_line("hello world");
        let ranges = vec![range_match(TextOperation::Insert, 0, 5)];
        let range_order = build_range_order(&ranges);
        let state = CodeViewerState {
            ranges,
            range_order,
            cursor_row: 0,
            cursor_col: 0,
            viewport_height: 1,
            is_focused: false,
            ..Default::default()
        };

        let line = widget.overlay_row(0, &state);
        let span = &line.spans[0];
        assert_eq!(span.content, "hello");
        assert_eq!(
            span.style.bg,
            background_for_operation(&TextOperation::Insert, &default_palette())
        );
    }

    #[test]
    fn focused_panel_ignores_stale_highlight_destination() {
        let widget = widget_with_line("hello world");
        let state = CodeViewerState {
            is_focused: true,
            highlight_destination: Some(TextRange::new(0, 6, 0, 11)),
            viewport_height: 1,
            ..Default::default()
        };

        let line = widget.overlay_row(0, &state);
        assert!(
            line.spans.iter().all(|span| span.style.bg.is_none()),
            "no span should be painted: highlight_destination is stale once focused"
        );
    }

    #[test]
    fn node_highlight_off_leaves_the_cursor_range_showing_its_diff_color() {
        let widget = widget_with_line("hello world");
        let ranges = vec![range_match(TextOperation::Insert, 0, 5)];
        let range_order = build_range_order(&ranges);
        let state = CodeViewerState {
            ranges,
            range_order,
            cursor_row: 0,
            cursor_col: 0,
            viewport_height: 1,
            is_focused: true,
            ..Default::default()
        };

        let line = widget.overlay_row(0, &state);
        let palette = default_palette();
        let span = &line.spans[0];
        assert_eq!(span.content, "hello");
        assert_ne!(
            span.style.bg,
            Some(palette.cross_highlight_bg),
            "the node highlight must not paint while it is toggled off"
        );
        assert_eq!(
            span.style.bg,
            background_for_operation(&TextOperation::Insert, &palette),
            "the underlying diff color must survive - that is what the highlight was covering"
        );
    }

    #[test]
    fn node_highlight_off_leaves_the_counterpart_unpainted() {
        let widget = widget_with_line("hello world");
        let state = CodeViewerState {
            is_focused: false,
            highlight_destination: Some(TextRange::new(0, 6, 0, 11)),
            viewport_height: 1,
            ..Default::default()
        };

        let line = widget.overlay_row(0, &state);
        assert!(
            line.spans.iter().all(|span| span.style.bg.is_none()),
            "the counterpart highlight must not paint while the toggle is off"
        );
    }

    #[test]
    fn range_at_finds_covering_range_and_resolves_ties_and_gaps() {
        let ranges = vec![
            range_match(TextOperation::Delete, 0, 3),
            RangeMatch {
                source: TextRange::new(0, 3, 0, 3),
                destination: TextRange::zero(),
                operation: TextOperation::Insert,
            },
            range_match(TextOperation::Identical, 3, 7),
        ];
        let order = build_range_order(&ranges);

        assert_eq!(range_at(&ranges, &order, 0, 1), Some(0));
        assert_eq!(range_at(&ranges, &order, 0, 3), Some(2));
        assert_eq!(range_at(&ranges, &order, 0, 6), Some(2));
        assert_eq!(range_at(&ranges, &order, 0, 7), None);
        assert_eq!(range_at(&ranges, &order, 1, 0), None);
    }

    #[test]
    fn load_ranges_places_cursor_on_first_navigable_position() {
        let mut state = CodeViewerState::default();
        state.load_ranges(vec![
            RangeMatch {
                source: TextRange::new(0, 0, 0, 0),
                destination: TextRange::zero(),
                operation: TextOperation::Insert,
            },
            range_match(TextOperation::Delete, 2, 4),
        ]);
        assert_eq!((state.cursor_row, state.cursor_col), (0, 2));
    }

    fn state_with_three_search_matches_on_rows_1_4_and_8() -> CodeViewerState {
        CodeViewerState {
            search_matches: vec![
                TextRange::new(1, 0, 1, 4),
                TextRange::new(4, 2, 4, 6),
                TextRange::new(8, 1, 8, 5),
            ],
            ..Default::default()
        }
    }

    #[test]
    fn nearest_search_match_position_finds_the_match_at_or_after_the_cursor() {
        let mut state = state_with_three_search_matches_on_rows_1_4_and_8();
        state.cursor_row = 0;
        state.cursor_col = 0;
        assert_eq!(state.nearest_search_match_position(), Some((1, 0)));

        state.cursor_row = 4;
        state.cursor_col = 2;
        assert_eq!(
            state.nearest_search_match_position(),
            Some((4, 2)),
            "sitting exactly on a match should find that match, unlike \
             next_search_match_position's strictly-after semantics - this is the first jump for \
             a fresh search"
        );

        state.cursor_row = 9;
        state.cursor_col = 0;
        assert_eq!(
            state.nearest_search_match_position(),
            Some((1, 0)),
            "past the last match should wrap to the first"
        );
    }

    #[test]
    fn nearest_search_match_position_is_none_with_no_matches() {
        let state = CodeViewerState::default();
        assert_eq!(state.nearest_search_match_position(), None);
    }

    #[test]
    fn next_search_match_position_wraps_around_at_the_ends() {
        let mut state = state_with_three_search_matches_on_rows_1_4_and_8();

        state.cursor_row = 4;
        state.cursor_col = 2;
        assert_eq!(
            state.next_search_match_position(true),
            Some((8, 1)),
            "sitting exactly on a match should jump to the *next* one, not stay put"
        );

        state.cursor_row = 8;
        state.cursor_col = 5; // past the last match
        assert_eq!(
            state.next_search_match_position(true),
            Some((1, 0)),
            "forward past the last match should wrap to the first"
        );

        state.cursor_row = 0;
        state.cursor_col = 0; // before the first match
        assert_eq!(
            state.next_search_match_position(false),
            Some((8, 1)),
            "backward before the first match should wrap to the last"
        );
    }

    #[test]
    fn search_match_count_and_index_counts_matches_at_or_before_the_cursor() {
        let mut state = state_with_three_search_matches_on_rows_1_4_and_8();

        state.cursor_row = 0;
        state.cursor_col = 0;
        assert_eq!(state.search_match_count_and_index(), Some((1, 3)));

        state.cursor_row = 4;
        state.cursor_col = 2;
        assert_eq!(state.search_match_count_and_index(), Some((2, 3)));

        state.cursor_row = 8;
        state.cursor_col = 5;
        assert_eq!(state.search_match_count_and_index(), Some((3, 3)));
    }

    #[test]
    fn search_match_count_and_index_is_none_with_no_matches() {
        let state = CodeViewerState::default();
        assert_eq!(state.search_match_count_and_index(), None);
    }

    #[test]
    fn load_ranges_clears_any_previous_search_matches() {
        let mut state = state_with_three_search_matches_on_rows_1_4_and_8();
        state.load_ranges(Vec::new());
        assert_eq!(state.search_matches, Vec::new());
    }

    #[test]
    fn overlay_row_paints_search_matches_in_the_dedicated_search_color() {
        let widget = widget_with_line("hello world");
        let state = CodeViewerState {
            search_matches: vec![TextRange::new(0, 6, 0, 11)],
            viewport_height: 1,
            ..Default::default()
        };

        let line = widget.overlay_row(0, &state);
        let span = line
            .spans
            .iter()
            .find(|span| span.content == "world")
            .expect("highlighted span");
        let palette = default_palette();
        assert_eq!(
            span.style.bg,
            Some(palette.search_bg),
            "search matches must use the dedicated search color, not the cursor blue"
        );
        assert_eq!(span.style.fg, Some(palette.overlay_fg));
    }

    #[test]
    fn cursor_destination_returns_matched_range_for_current_position() {
        let mut state = CodeViewerState::default();
        let dest = TextRange::new(5, 1, 5, 9);
        state.load_ranges(vec![RangeMatch {
            source: TextRange::new(0, 2, 0, 4),
            destination: dest.clone(),
            operation: TextOperation::Update,
        }]);
        assert_eq!(state.cursor_destination(), Some(dest));
    }

    #[test]
    fn cross_highlight_destination_uses_bright_blue_with_explicit_foreground() {
        let widget = widget_with_line("hello world");
        let state = CodeViewerState {
            is_focused: false,
            highlight_destination: Some(TextRange::new(0, 6, 0, 11)),
            viewport_height: 1,
            node_highlight: true,
            ..Default::default()
        };

        let line = widget.overlay_row(0, &state);
        let span = line
            .spans
            .iter()
            .find(|span| span.content == "world")
            .expect("highlighted span");
        let palette = default_palette();
        assert_eq!(span.style.fg, Some(palette.overlay_fg));
        assert_eq!(span.style.bg, Some(palette.cross_highlight_bg));
    }

    #[test]
    fn set_overlay_theme_changes_painted_colors() {
        let mut widget = widget_with_line("hello world");
        let ranges = vec![range_match(TextOperation::Insert, 0, 5)];
        let range_order = build_range_order(&ranges);
        let state = CodeViewerState {
            ranges,
            range_order,
            cursor_row: 0,
            cursor_col: 99,
            viewport_height: 1,
            ..Default::default()
        };

        let before = widget.overlay_row(0, &state).spans[0].style;
        widget.set_overlay_theme(OverlayTheme::SolarizedLight);
        let after = widget.overlay_row(0, &state).spans[0].style;

        assert_ne!(before.bg, after.bg);
        assert_eq!(
            after.fg,
            Some(OverlayTheme::SolarizedLight.palette().overlay_fg)
        );
    }

    #[test]
    fn render_never_draws_its_own_border_or_title() {
        let area = Rect::new(0, 0, 20, 5);
        let mut state = CodeViewerState {
            viewport_height: 1,
            ..Default::default()
        };

        let mut buf = Buffer::empty(area);
        let widget = widget_with_line("hello");
        (&widget).render(area, &mut buf, &mut state);

        let text: String = buf.content().iter().map(|cell| cell.symbol()).collect();
        assert!(
            !text.contains(&widget.filename()),
            "no title should ever be drawn: {text}"
        );
        assert_eq!(buf[(0, 0)].symbol(), "1", "gutter line number first");
        assert_eq!(
            buf[(widget.gutter_width() as u16, 0)].symbol(),
            "h",
            "content immediately after the gutter, no border row/column to skip"
        );
    }

    #[test]
    fn gutter_width_tracks_line_count_and_disappears_when_empty() {
        assert_eq!(CodeViewerWidget::default().gutter_width(), 0);
        assert_eq!(widget_with_line("one line").gutter_width(), 2);
        let ninety_nine_lines = widget_with_line(&vec!["x"; 99].join("\n"));
        assert_eq!(ninety_nine_lines.gutter_width(), 3);
    }

    #[test]
    fn slice_columns_windows_a_line_and_marks_cut_edges() {
        let line = Line::from(vec![
            Span::styled("abcde".to_string(), Style::new().fg(Color::Red)),
            Span::styled("fghij".to_string(), Style::new().fg(Color::Green)),
        ]);

        let full = slice_columns(&line, 0, 20);
        let text: String = full.spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(text, "abcdefghij");

        let window = slice_columns(&line, 2, 5);
        let text: String = window.spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(text, "…def…");
        assert_eq!(window.spans.first().unwrap().style, GUTTER_STYLE);
        assert_eq!(window.spans.last().unwrap().style, GUTTER_STYLE);
        assert!(
            window
                .spans
                .iter()
                .any(|s| s.content.contains('d') && s.style.fg == Some(Color::Red)),
            "red span styling should survive slicing: {window:?}"
        );
        assert!(
            window
                .spans
                .iter()
                .any(|s| s.content.contains('f') && s.style.fg == Some(Color::Green)),
            "green span styling should survive slicing: {window:?}"
        );

        let prefix = slice_columns(&line, 0, 4);
        let text: String = prefix.spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(text, "abc…");

        let empty = slice_columns(&line, 3, 0);
        let text: String = empty.spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(text, "");
    }

    fn sliced_text(line: &Line<'static>, from: usize, width: usize) -> String {
        slice_columns(line, from, width)
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect()
    }

    #[test]
    fn slice_columns_expands_tabs_to_the_next_tab_stop_keeping_their_style() {
        let painted = Style::new().bg(Color::Red);
        let line = Line::from(vec![
            Span::styled("\t".to_string(), painted),
            Span::from("a\tb".to_string()),
        ]);

        assert_eq!(sliced_text(&line, 0, 20), "    a   b");
        let window = slice_columns(&line, 0, 20);
        assert_eq!(window.spans[0].content, "    ");
        assert_eq!(
            window.spans[0].style, painted,
            "a painted tab paints all of its columns"
        );
    }

    #[test]
    fn slice_columns_counts_display_columns_when_scrolled_past_a_tab() {
        let line = Line::from("\tabcdef".to_string());
        // Columns: the tab 0-3, then `a` at 4. From column 2, `…` takes column 2, the rest of the
        // tab is column 3.
        assert_eq!(sliced_text(&line, 2, 20), "… abcdef");
        assert_eq!(sliced_text(&line, 5, 20), "…cdef");
        assert_eq!(sliced_text(&line, 0, 6), "    a…");
    }

    #[test]
    fn slice_columns_keeps_columns_aligned_across_a_cut_wide_character() {
        let line = Line::from("漢字ab".to_string());
        // `漢` is columns 0-1, `字` 2-3. From column 1, `…` takes column 1; `字` is whole.
        assert_eq!(sliced_text(&line, 1, 20), "…字ab");
        // From column 2, `…` takes column 2, and `字`'s second column is left as a space.
        assert_eq!(sliced_text(&line, 2, 20), "… ab");
        // A window ending inside `字`: column 2 is shown as a space, column 3 is the `…`.
        assert_eq!(sliced_text(&line, 0, 4), "漢 …");
    }

    #[test]
    fn find_matches_is_case_sensitive_when_the_query_has_an_uppercase_letter() {
        let widget = widget_with_line("World world WORLD\n");
        assert_eq!(
            widget.find_matches("World"),
            vec![TextRange::new(0, 0, 0, 5)]
        );
        assert_eq!(widget.find_matches("world").len(), 3);
    }

    #[test]
    fn paint_columns_reads_columns_as_bytes_on_a_multi_byte_row() {
        let line = Line::from("é = world");
        let style = Style::new().bg(Color::Red);
        let painted = paint_columns(&line, 5, 10, style);
        let span = painted
            .spans
            .iter()
            .find(|span| span.style.bg == Some(Color::Red))
            .expect("painted span");
        assert_eq!(span.content, "world");
    }

    #[test]
    fn trailing_whitespace_trimmed_len_counts_bytes_and_drops_trailing_whitespace() {
        let line = Line::from(vec![Span::from("é x"), Span::from("  ")]);
        assert_eq!(trailing_whitespace_trimmed_len(&line).get(), 4);
    }

    #[test]
    fn replace_ranges_keeps_the_cursor_and_search_but_drops_the_cross_highlight() {
        let mut state = state_with_three_search_matches_on_rows_1_4_and_8();
        state.cursor_row = 4;
        state.cursor_col = 2;
        state.highlight_destination = Some(TextRange::new(0, 0, 0, 1));

        state.replace_ranges(vec![range_match(TextOperation::Insert, 0, 5)], 10);

        assert_eq!((state.cursor_row, state.cursor_col), (4, 2));
        assert_eq!(state.search_matches.len(), 3);
        assert_eq!(state.highlight_destination, None);
    }
}
