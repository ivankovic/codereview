//! Colour schemes. One [`Theme`] names the syntect theme for syntax highlighting and every
//! colour the front ends paint themselves: cursor, selection, search, the four diff
//! operations, the three comment states. Both front ends read the same theme, so a scheme
//! chosen in one is what the other shows.

use serde::{Deserialize, Serialize};

use crate::anchor::AnchorState;
use crate::diff::Op;

/// A colour as a front end can use it: the terminal's default, one of its 16 palette
/// entries, or an RGB value. Serialised as a CSS-ish string (`default`, `ansi:2`, `#rrggbb`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Paint {
    Default,
    Ansi(u8),
    Rgb(u8, u8, u8),
}

impl Paint {
    pub fn css(self) -> String {
        match self {
            Paint::Default => "default".into(),
            Paint::Ansi(i) => format!("ansi:{i}"),
            Paint::Rgb(r, g, b) => format!("#{r:02x}{g:02x}{b:02x}"),
        }
    }
}

impl Serialize for Paint {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.css())
    }
}

impl<'de> Deserialize<'de> for Paint {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let text = String::deserialize(d)?;
        if text == "default" {
            return Ok(Paint::Default);
        }
        if let Some(i) = text.strip_prefix("ansi:") {
            return i.parse().map(Paint::Ansi).map_err(serde::de::Error::custom);
        }
        let hex = text.strip_prefix('#').unwrap_or(&text);
        if hex.len() == 6 {
            if let Ok(v) = u32::from_str_radix(hex, 16) {
                return Ok(Paint::Rgb((v >> 16) as u8, (v >> 8) as u8, v as u8));
            }
        }
        Err(serde::de::Error::custom(format!("not a colour: {text}")))
    }
}

const fn rgb(v: u32) -> Paint {
    Paint::Rgb((v >> 16) as u8, (v >> 8) as u8, v as u8)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Theme {
    pub name: String,
    pub dark: bool,
    /// Uses only the terminal's own palette; not offered by the web UI.
    pub terminal: bool,
    /// syntect theme name, from two-face's embedded set.
    pub syntax: String,
    pub bg: Paint,
    pub fg: Paint,
    pub panel: Paint,
    pub border: Paint,
    pub dim: Paint,
    pub accent: Paint,
    pub cursor_bg: Paint,
    pub visual_bg: Paint,
    pub search_bg: Paint,
    pub insert_line: Paint,
    pub insert_span: Paint,
    pub insert_fg: Paint,
    pub delete_line: Paint,
    pub delete_span: Paint,
    pub delete_fg: Paint,
    pub update_line: Paint,
    pub update_span: Paint,
    pub update_fg: Paint,
    pub move_line: Paint,
    pub move_span: Paint,
    pub move_fg: Paint,
    pub exact: Paint,
    pub moved: Paint,
    pub stale: Paint,
}

impl Theme {
    /// Background of a whole line holding a change.
    pub fn line_bg(&self, op: Op) -> Paint {
        match op {
            Op::None => Paint::Default,
            Op::Insert => self.insert_line,
            Op::Delete => self.delete_line,
            Op::Update => self.update_line,
            Op::Move => self.move_line,
        }
    }

    /// Background of the changed characters within a line.
    pub fn span_bg(&self, op: Op) -> Paint {
        match op {
            Op::None => Paint::Default,
            Op::Insert => self.insert_span,
            Op::Delete => self.delete_span,
            Op::Update => self.update_span,
            Op::Move => self.move_span,
        }
    }

    /// Foreground for markers and, in the terminal scheme, for changed characters.
    pub fn op_fg(&self, op: Op) -> Paint {
        match op {
            Op::None => Paint::Default,
            Op::Insert => self.insert_fg,
            Op::Delete => self.delete_fg,
            Op::Update => self.update_fg,
            Op::Move => self.move_fg,
        }
    }

    pub fn comment_fg(&self, state: AnchorState) -> Paint {
        match state {
            AnchorState::Exact => self.exact,
            AnchorState::Moved => self.moved,
            AnchorState::Stale => self.stale,
        }
    }

    pub fn named(name: &str) -> Option<Theme> {
        all()
            .into_iter()
            .find(|t| t.name.eq_ignore_ascii_case(name))
    }
}

impl Default for Theme {
    /// The terminal's own palette: readable on any background, which is what a first run
    /// needs. `t` in the TUI picks something prettier.
    fn default() -> Self {
        terminal()
    }
}

/// Every built-in scheme, in the order the picker lists them.
pub fn all() -> Vec<Theme> {
    vec![
        terminal(),
        dark(),
        light(),
        catppuccin_latte(),
        catppuccin_mocha(),
        solarized_light(),
        solarized_dark(),
        gruvbox_light(),
        gruvbox_dark(),
        nord(),
        dracula(),
    ]
}

fn terminal() -> Theme {
    Theme {
        name: "Terminal".into(),
        dark: false,
        terminal: true,
        syntax: "ansi".into(),
        bg: Paint::Default,
        fg: Paint::Default,
        panel: Paint::Default,
        border: Paint::Ansi(8),
        dim: Paint::Ansi(8),
        accent: Paint::Ansi(6),
        cursor_bg: Paint::Default,
        visual_bg: Paint::Ansi(4),
        search_bg: Paint::Ansi(3),
        insert_line: Paint::Default,
        insert_span: Paint::Default,
        insert_fg: Paint::Ansi(2),
        delete_line: Paint::Default,
        delete_span: Paint::Default,
        delete_fg: Paint::Ansi(1),
        update_line: Paint::Default,
        update_span: Paint::Default,
        update_fg: Paint::Ansi(3),
        move_line: Paint::Default,
        move_span: Paint::Default,
        move_fg: Paint::Ansi(5),
        exact: Paint::Ansi(6),
        moved: Paint::Ansi(3),
        stale: Paint::Ansi(1),
    }
}

#[allow(clippy::too_many_arguments)]
fn scheme(
    name: &str,
    dark: bool,
    syntax: &str,
    [bg, fg, panel, border, dim, accent]: [u32; 6],
    [cursor, visual, search]: [u32; 3],
    [insert_line, insert_span, insert_fg]: [u32; 3],
    [delete_line, delete_span, delete_fg]: [u32; 3],
    [update_line, update_span, update_fg]: [u32; 3],
    [move_line, move_span, move_fg]: [u32; 3],
    [exact, moved, stale]: [u32; 3],
) -> Theme {
    Theme {
        name: name.into(),
        dark,
        terminal: false,
        syntax: syntax.into(),
        bg: rgb(bg),
        fg: rgb(fg),
        panel: rgb(panel),
        border: rgb(border),
        dim: rgb(dim),
        accent: rgb(accent),
        cursor_bg: rgb(cursor),
        visual_bg: rgb(visual),
        search_bg: rgb(search),
        insert_line: rgb(insert_line),
        insert_span: rgb(insert_span),
        insert_fg: rgb(insert_fg),
        delete_line: rgb(delete_line),
        delete_span: rgb(delete_span),
        delete_fg: rgb(delete_fg),
        update_line: rgb(update_line),
        update_span: rgb(update_span),
        update_fg: rgb(update_fg),
        move_line: rgb(move_line),
        move_span: rgb(move_span),
        move_fg: rgb(move_fg),
        exact: rgb(exact),
        moved: rgb(moved),
        stale: rgb(stale),
    }
}

fn dark() -> Theme {
    scheme(
        "Dark",
        true,
        "base16-ocean.dark",
        [0x15171c, 0xd6d8de, 0x1b1e24, 0x2b2f38, 0x8a8f9a, 0x5ec8e5],
        [0x323746, 0x283c5a, 0x785a00],
        [0x102a16, 0x165424, 0x3fb950],
        [0x341414, 0x6e1e1e, 0xf85149],
        [0x30280a, 0x604e0a, 0xe3b341],
        [0x261836, 0x46246e, 0xa371f7],
        [0x5ec8e5, 0xe3b341, 0xf85149],
    )
}

fn light() -> Theme {
    scheme(
        "Light",
        false,
        "GitHub",
        [0xffffff, 0x24292e, 0xf6f8fa, 0xd0d7de, 0x6e7781, 0x0969da],
        [0xe8ecf2, 0xcfe0ff, 0xffe066],
        [0xe6ffec, 0xabf2bc, 0x1a7f37],
        [0xffebe9, 0xffb7b3, 0xcf222e],
        [0xfff8c5, 0xffe08a, 0x9a6700],
        [0xefe6ff, 0xd2bcff, 0x8250df],
        [0x0969da, 0x9a6700, 0xcf222e],
    )
}

fn catppuccin_latte() -> Theme {
    scheme(
        "Catppuccin Latte",
        false,
        "Catppuccin Latte",
        [0xeff1f5, 0x4c4f69, 0xe6e9ef, 0xccd0da, 0x6c6f85, 0x179299],
        [0xccd0da, 0xc6d6f8, 0xf5e0b4],
        [0xdcefd6, 0xb5e2a8, 0x40a02b],
        [0xf7d6dc, 0xf2afbc, 0xd20f39],
        [0xf6e7c9, 0xf0d08f, 0xdf8e1d],
        [0xe6daf8, 0xd2bcf3, 0x8839ef],
        [0x1e66f5, 0xdf8e1d, 0xd20f39],
    )
}

fn catppuccin_mocha() -> Theme {
    scheme(
        "Catppuccin Mocha",
        true,
        "Catppuccin Mocha",
        [0x1e1e2e, 0xcdd6f4, 0x181825, 0x313244, 0x7f849c, 0x94e2d5],
        [0x313244, 0x45475a, 0x6f5f2a],
        [0x2a3b33, 0x3c5f45, 0xa6e3a1],
        [0x452b3a, 0x6b3a4c, 0xf38ba8],
        [0x453f2e, 0x6b5e3a, 0xf9e2af],
        [0x3b3050, 0x574478, 0xcba6f7],
        [0x89b4fa, 0xf9e2af, 0xf38ba8],
    )
}

fn solarized_light() -> Theme {
    scheme(
        "Solarized Light",
        false,
        "Solarized (light)",
        [0xfdf6e3, 0x657b83, 0xeee8d5, 0xd9d2c2, 0x93a1a1, 0x2aa198],
        [0xeee8d5, 0xd6e4e8, 0xe8dfa0],
        [0xe3edd3, 0xc8dfa8, 0x859900],
        [0xf5dad5, 0xedb8ae, 0xdc322f],
        [0xf3e8c5, 0xe9d48e, 0xb58900],
        [0xe9ddef, 0xd7c0e3, 0xd33682],
        [0x268bd2, 0xb58900, 0xdc322f],
    )
}

fn solarized_dark() -> Theme {
    scheme(
        "Solarized Dark",
        true,
        "Solarized (dark)",
        [0x002b36, 0x839496, 0x073642, 0x0e4553, 0x586e75, 0x2aa198],
        [0x073642, 0x0b4a5a, 0x5a4a10],
        [0x0e3a2e, 0x17553a, 0x859900],
        [0x3a1f26, 0x5a2a30, 0xdc322f],
        [0x3a3520, 0x5a4f20, 0xb58900],
        [0x2a2a50, 0x3e3a70, 0x6c71c4],
        [0x268bd2, 0xb58900, 0xdc322f],
    )
}

fn gruvbox_light() -> Theme {
    scheme(
        "Gruvbox Light",
        false,
        "gruvbox-light",
        [0xfbf1c7, 0x3c3836, 0xf2e5bc, 0xd5c4a1, 0x7c6f64, 0x427b58],
        [0xebdbb2, 0xd5c4a1, 0xf2d66e],
        [0xe4e8b6, 0xcbd48a, 0x79740e],
        [0xf4d3c6, 0xedae9a, 0x9d0006],
        [0xf5e4b0, 0xedd07a, 0xb57614],
        [0xebd7df, 0xd9b6c6, 0x8f3f71],
        [0x076678, 0xb57614, 0x9d0006],
    )
}

fn gruvbox_dark() -> Theme {
    scheme(
        "Gruvbox Dark",
        true,
        "gruvbox-dark",
        [0x282828, 0xebdbb2, 0x1d2021, 0x3c3836, 0x928374, 0x8ec07c],
        [0x3c3836, 0x504945, 0x6a5a10],
        [0x32361e, 0x4a5a24, 0xb8bb26],
        [0x3e2624, 0x5e2e2a, 0xfb4934],
        [0x3e3620, 0x5e5020, 0xfabd2f],
        [0x3a2a3a, 0x563a56, 0xd3869b],
        [0x83a598, 0xfabd2f, 0xfb4934],
    )
}

fn nord() -> Theme {
    scheme(
        "Nord",
        true,
        "Nord",
        [0x2e3440, 0xd8dee9, 0x3b4252, 0x434c5e, 0x7b88a1, 0x81a1c1],
        [0x3b4252, 0x434c5e, 0x6a5f2e],
        [0x34463e, 0x465f4e, 0xa3be8c],
        [0x46343a, 0x6a3f48, 0xbf616a],
        [0x464230, 0x6a6040, 0xebcb8b],
        [0x40364a, 0x5a4a66, 0xb48ead],
        [0x88c0d0, 0xebcb8b, 0xbf616a],
    )
}

fn dracula() -> Theme {
    scheme(
        "Dracula",
        true,
        "Dracula",
        [0x282a36, 0xf8f8f2, 0x21222c, 0x44475a, 0x6272a4, 0xff79c6],
        [0x44475a, 0x4a5080, 0x6a5a20],
        [0x2e4a38, 0x3c6b4a, 0x50fa7b],
        [0x4a2e38, 0x6b3c4a, 0xff5555],
        [0x4a4830, 0x6b6540, 0xf1fa8c],
        [0x3f2e4a, 0x5a3c6b, 0xbd93f9],
        [0x8be9fd, 0xf1fa8c, 0xff5555],
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_theme_has_a_syntect_theme() {
        for theme in all() {
            assert!(
                crate::highlight::has_syntax_theme(&theme.syntax),
                "{} names unknown syntect theme {}",
                theme.name,
                theme.syntax
            );
        }
    }

    #[test]
    fn paint_round_trips() {
        for p in [Paint::Default, Paint::Ansi(3), Paint::Rgb(1, 2, 255)] {
            let json = serde_json::to_string(&p).unwrap();
            let back: Paint = serde_json::from_str(&json).unwrap();
            assert_eq!(p, back);
        }
        assert_eq!(Paint::Rgb(1, 2, 255).css(), "#0102ff");
    }

    #[test]
    fn lookup_is_case_insensitive() {
        assert_eq!(
            Theme::named("catppuccin latte").unwrap().name,
            "Catppuccin Latte"
        );
        assert!(Theme::named("nope").is_none());
    }
}
