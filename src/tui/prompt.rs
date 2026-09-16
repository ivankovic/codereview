//! A one-line text input drawn over the status bar: comment text, note text, search, line
//! number.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Stylize;
use ratatui::text::Line;
use ratatui::widgets::Paragraph;
use unicode_width::UnicodeWidthStr;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PromptKind {
    Comment {
        path: String,
        /// The lines commented on, or `None` for the whole file or directory.
        lines: Option<(usize, usize)>,
    },
    EditComment,
    Note {
        path: Option<String>,
    },
    EditNote,
    Search,
    GoToLine,
    /// The tree filter, which applies as it is typed. `was` is what it was before, put
    /// back when the prompt is cancelled.
    Filter {
        was: String,
    },
    /// Text for the agent, with embedded context `(uri, text)`.
    Agent {
        context: Vec<(String, String)>,
    },
    SymbolSearch,
}

#[derive(Debug, Clone)]
pub struct Prompt {
    pub kind: PromptKind,
    pub label: String,
    pub text: String,
    /// Byte offset of the cursor in `text`.
    pub cursor: usize,
}

impl Prompt {
    pub fn new(kind: PromptKind, label: impl Into<String>, initial: &str) -> Self {
        Self {
            kind,
            label: label.into(),
            text: initial.to_string(),
            cursor: initial.len(),
        }
    }

    pub fn insert(&mut self, c: char) {
        self.text.insert(self.cursor, c);
        self.cursor += c.len_utf8();
    }

    pub fn backspace(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let prev = self.text[..self.cursor]
            .char_indices()
            .next_back()
            .map(|(i, _)| i)
            .unwrap_or(0);
        self.text.replace_range(prev..self.cursor, "");
        self.cursor = prev;
    }

    pub fn delete(&mut self) {
        if self.cursor >= self.text.len() {
            return;
        }
        let next = self.text[self.cursor..]
            .char_indices()
            .nth(1)
            .map(|(i, _)| self.cursor + i)
            .unwrap_or(self.text.len());
        self.text.replace_range(self.cursor..next, "");
    }

    pub fn left(&mut self) {
        self.cursor = self.text[..self.cursor]
            .char_indices()
            .next_back()
            .map(|(i, _)| i)
            .unwrap_or(0);
    }

    pub fn right(&mut self) {
        self.cursor = self.text[self.cursor..]
            .char_indices()
            .nth(1)
            .map(|(i, _)| self.cursor + i)
            .unwrap_or(self.text.len());
    }

    pub fn home(&mut self) {
        self.cursor = 0;
    }

    pub fn end(&mut self) {
        self.cursor = self.text.len();
    }

    pub fn delete_word(&mut self) {
        let before = &self.text[..self.cursor];
        let trimmed = before.trim_end();
        let start = trimmed
            .rfind(char::is_whitespace)
            .map(|i| i + 1)
            .unwrap_or(0);
        self.text.replace_range(start..self.cursor, "");
        self.cursor = start;
    }

    pub fn draw(&self, frame: &mut Frame, area: Rect) {
        let label = format!("{}: ", self.label);
        let width = area.width as usize;
        let avail = width.saturating_sub(label.width());
        // Scroll so the cursor stays visible.
        let before_cursor = &self.text[..self.cursor];
        let cursor_col = before_cursor.width();
        let skip = cursor_col.saturating_sub(avail.saturating_sub(1));
        let mut shown = String::new();
        let mut col = 0;
        for ch in self.text.chars() {
            let w = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
            if col + w > skip && shown.width() + w <= avail {
                shown.push(ch);
            }
            col += w;
        }
        let line: Line = vec![label.clone().bold().cyan(), shown.into()].into();
        frame.render_widget(Paragraph::new(line), area);
        let x = area.x + (label.width() + cursor_col - skip).min(width.saturating_sub(1)) as u16;
        frame.set_cursor_position((x, area.y));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn editing() {
        let mut p = Prompt::new(PromptKind::Search, "search", "héllo");
        p.backspace();
        assert_eq!(p.text, "héll");
        p.left();
        p.left();
        p.insert('X');
        assert_eq!(p.text, "héXll");
        p.home();
        p.delete();
        assert_eq!(p.text, "éXll");
        p.end();
        p.delete_word();
        assert_eq!(p.text, "");
    }
}
