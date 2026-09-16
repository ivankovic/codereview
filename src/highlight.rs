//! Syntax highlighting into a representation neither front end owns: per line, a list of
//! contiguous segments with a foreground colour and font style. The TUI turns these into
//! Ratatui spans, the web page into `<span>`s.

use std::sync::OnceLock;

use serde::{Deserialize, Serialize};
use syntect::easy::HighlightLines;
use syntect::highlighting::{FontStyle, Theme, ThemeSet};
use syntect::parsing::{SyntaxReference, SyntaxSet};

/// Files longer than this are not highlighted; syntect is linear but not free.
const MAX_LINES: usize = 20_000;

pub const DEFAULT_THEME: &str = "ansi";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Segment {
    pub text: String,
    pub fg: Option<(u8, u8, u8)>,
    /// A terminal palette index instead of an RGB colour: what the `ansi` theme produces,
    /// encoded by bat's convention as alpha 0 with the index in the red channel.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ansi: Option<u8>,
    pub bold: bool,
    pub italic: bool,
}

impl Segment {
    pub fn plain(text: &str) -> Self {
        Self {
            text: text.to_string(),
            fg: None,
            ansi: None,
            bold: false,
            italic: false,
        }
    }
}

fn syntax_set() -> &'static SyntaxSet {
    static SET: OnceLock<SyntaxSet> = OnceLock::new();
    SET.get_or_init(two_face::syntax::extra_newlines)
}

/// two-face's embedded themes: syntect's own plus Catppuccin, Gruvbox, Nord, Dracula, the
/// terminal-palette `ansi` theme and more.
fn theme_set() -> &'static ThemeSet {
    static SET: OnceLock<ThemeSet> = OnceLock::new();
    SET.get_or_init(|| ThemeSet::from(&two_face::theme::extra()))
}

pub fn theme_names() -> Vec<String> {
    theme_set().themes.keys().cloned().collect()
}

pub fn has_syntax_theme(name: &str) -> bool {
    theme_set().themes.contains_key(name)
}

fn theme(name: &str) -> &'static Theme {
    let set = theme_set();
    set.themes
        .get(name)
        .or_else(|| set.themes.get(DEFAULT_THEME))
        .expect("two-face ships the ansi theme")
}

/// Picks a syntax from the file name (extension, then whole name), then from the first line.
fn syntax_for(path: &str, first_line: &str) -> Option<&'static SyntaxReference> {
    let set = syntax_set();
    let name = path.rsplit('/').next().unwrap_or(path);
    if let Some(ext) = name.rsplit('.').next().filter(|e| *e != name) {
        if let Some(s) = set.find_syntax_by_extension(ext) {
            return Some(s);
        }
    }
    if let Some(s) = set.find_syntax_by_extension(name) {
        return Some(s);
    }
    if name.eq_ignore_ascii_case("makefile") || name.ends_with(".mk") {
        if let Some(s) = set.find_syntax_by_name("Makefile") {
            return Some(s);
        }
    }
    set.find_syntax_by_first_line(first_line)
}

/// The theme's background, for a front end that wants to match it.
pub fn theme_background(theme_name: &str) -> Option<(u8, u8, u8)> {
    theme(theme_name)
        .settings
        .background
        .map(|c| (c.r, c.g, c.b))
}

/// Highlights `text` as the file at `path`. Always returns one entry per line of `text`, each
/// covering the line's bytes in order, so a caller can split segments at diff span boundaries.
pub fn highlight(path: &str, text: &str, theme_name: &str) -> Vec<Vec<Segment>> {
    let lines: Vec<&str> = text.lines().collect();
    let plain = || {
        lines
            .iter()
            .map(|l| vec![Segment::plain(l)])
            .collect::<Vec<_>>()
    };
    if lines.len() > MAX_LINES {
        return plain();
    }
    let Some(syntax) = syntax_for(path, lines.first().copied().unwrap_or("")) else {
        return plain();
    };
    let set = syntax_set();
    let mut hl = HighlightLines::new(syntax, theme(theme_name));
    let mut out = Vec::with_capacity(lines.len());
    for line in &lines {
        // syntect wants the newline for its state machine.
        let with_newline = format!("{line}\n");
        let Ok(ranges) = hl.highlight_line(&with_newline, set) else {
            out.push(vec![Segment::plain(line)]);
            continue;
        };
        let mut segments = Vec::with_capacity(ranges.len());
        for (style, piece) in ranges {
            let piece = piece.strip_suffix('\n').unwrap_or(piece);
            if piece.is_empty() {
                continue;
            }
            let c = style.foreground;
            // bat's encoding for terminal themes: alpha 0 carries a palette index in `r`,
            // alpha 1 means the terminal's default foreground.
            let (fg, ansi) = match c.a {
                0 => (None, Some(c.r)),
                1 => (None, None),
                _ => (Some((c.r, c.g, c.b)), None),
            };
            segments.push(Segment {
                text: piece.to_string(),
                fg,
                ansi,
                bold: style.font_style.contains(FontStyle::BOLD),
                italic: style.font_style.contains(FontStyle::ITALIC),
            });
        }
        out.push(segments);
    }
    out
}

/// Cuts `segments` (one line) at every byte offset in `cuts`, so each resulting segment lies
/// entirely inside one diff span. Offsets outside the line are ignored.
pub fn split_at(segments: &[Segment], cuts: &[usize]) -> Vec<(usize, Segment)> {
    let mut out = Vec::new();
    let mut pos = 0;
    for seg in segments {
        let end = pos + seg.text.len();
        let mut start = pos;
        for &cut in cuts.iter().filter(|&&c| c > pos && c < end) {
            if !seg.text.is_char_boundary(cut - pos) {
                continue;
            }
            push_piece(&mut out, seg, start, cut, pos);
            start = cut;
        }
        push_piece(&mut out, seg, start, end, pos);
        pos = end;
    }
    out
}

fn push_piece(
    out: &mut Vec<(usize, Segment)>,
    seg: &Segment,
    start: usize,
    end: usize,
    base: usize,
) {
    if end <= start {
        return;
    }
    out.push((
        start,
        Segment {
            text: seg.text[start - base..end - base].to_string(),
            fg: seg.fg,
            ansi: seg.ansi,
            bold: seg.bold,
            italic: seg.italic,
        },
    ));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rust_and_makefile_have_syntaxes() {
        assert!(syntax_for("src/main.rs", "").is_some());
        assert!(syntax_for("Makefile", "").is_some());
        assert!(syntax_for("a/b/foo.mk", "").is_some());
        assert!(syntax_for("index.html", "").is_some());
        assert!(syntax_for("x.py", "").is_some());
        assert!(syntax_for("x.ts", "").is_some());
        assert!(syntax_for("script", "#!/bin/bash").is_some());
    }

    #[test]
    fn ansi_theme_yields_palette_indices() {
        let lines = highlight("x.rs", "fn main() {}\n", "ansi");
        assert!(lines[0].iter().any(|s| s.ansi.is_some()));
        assert!(lines[0].iter().all(|s| s.fg.is_none()));
        let lines = highlight("x.rs", "fn main() {}\n", "Catppuccin Latte");
        assert!(lines[0].iter().all(|s| s.fg.is_some()));
    }

    #[test]
    fn highlight_keeps_every_byte() {
        let text = "fn main() {\n    println!(\"hi\");\n}\n";
        let lines = highlight("x.rs", text, "base16-ocean.dark");
        assert_eq!(lines.len(), 3);
        for (line, segs) in text.lines().zip(&lines) {
            let joined: String = segs.iter().map(|s| s.text.as_str()).collect();
            assert_eq!(joined, line);
        }
    }

    #[test]
    fn split_at_cuts() {
        let segs = vec![Segment::plain("hello"), Segment::plain(" world")];
        let pieces = split_at(&segs, &[2, 5, 7, 99]);
        let texts: Vec<(usize, &str)> = pieces.iter().map(|(o, s)| (*o, s.text.as_str())).collect();
        assert_eq!(texts, vec![(0, "he"), (2, "llo"), (5, " w"), (7, "orld")]);
    }
}
