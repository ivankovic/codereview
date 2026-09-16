//! The bridge from the core's theme and highlight segments to Ratatui styles.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Span;

use crate::anchor::AnchorState;
use crate::diff::Op;
use crate::highlight::Segment;
use crate::theme::{Paint, Theme};

pub fn color(paint: Paint) -> Option<Color> {
    match paint {
        Paint::Default => None,
        Paint::Ansi(i) => Some(Color::Indexed(i)),
        Paint::Rgb(r, g, b) => Some(Color::Rgb(r, g, b)),
    }
}

pub fn fg(paint: Paint) -> Style {
    match color(paint) {
        Some(c) => Style::new().fg(c),
        None => Style::new(),
    }
}

pub fn bg(paint: Paint) -> Style {
    match color(paint) {
        Some(c) => Style::new().bg(c),
        None => Style::new(),
    }
}

/// The cursor line: the theme's cursor background, or reverse video when it has none.
pub fn cursor(theme: &Theme) -> Style {
    match color(theme.cursor_bg) {
        Some(c) => Style::new().bg(c),
        None => Style::new().add_modifier(Modifier::REVERSED),
    }
}

pub fn dim(theme: &Theme) -> Style {
    match color(theme.dim) {
        Some(c) => Style::new().fg(c),
        None => Style::new().add_modifier(Modifier::DIM),
    }
}

pub fn accent(theme: &Theme) -> Style {
    fg(theme.accent)
}

pub fn op_marker(op: Op) -> &'static str {
    match op {
        Op::None => " ",
        Op::Insert => "+",
        Op::Delete => "-",
        Op::Update => "~",
        Op::Move => ">",
    }
}

pub fn comment_style(theme: &Theme, state: AnchorState, pending: bool) -> Style {
    let base = fg(theme.comment_fg(state));
    if pending {
        base
    } else {
        base.add_modifier(Modifier::DIM)
    }
}

/// A highlight segment as a span, with an optional background painted over it. Terminal
/// palette indices and RGB colours both come through; a segment with neither keeps the
/// default foreground.
pub fn segment_span(seg: &Segment, highlight: bool, background: Option<Color>) -> Span<'static> {
    let mut style = Style::new();
    if highlight {
        if let Some((r, g, b)) = seg.fg {
            style = style.fg(Color::Rgb(r, g, b));
        } else if let Some(i) = seg.ansi {
            style = style.fg(Color::Indexed(i));
        }
        if seg.bold {
            style = style.add_modifier(Modifier::BOLD);
        }
        if seg.italic {
            style = style.add_modifier(Modifier::ITALIC);
        }
    }
    if let Some(bg) = background {
        style = style.bg(bg);
    }
    Span::styled(seg.text.clone(), style)
}

/// How a changed span within a line is shown: a background in RGB schemes; in the terminal
/// scheme, which has no safe backgrounds, the operation's colour as foreground plus bold.
pub fn change_span(
    seg: &Segment,
    op: Op,
    theme: &Theme,
    highlight: bool,
    line_bg: Option<Color>,
) -> Span<'static> {
    if theme.terminal {
        let mut span = segment_span(seg, false, line_bg);
        span.style = span
            .style
            .patch(fg(theme.op_fg(op)))
            .add_modifier(Modifier::BOLD);
        span
    } else {
        segment_span(seg, highlight, color(theme.span_bg(op)).or(line_bg))
    }
}
