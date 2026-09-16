//! Re-anchoring comments after the file they refer to has changed.
//!
//! A comment carries a line number and the text that line had when the comment was written. If
//! the file still has that text on that line, nothing to do. If not, and the text occurs exactly
//! once elsewhere, the comment has moved there. If it occurs several times, the closest
//! occurrence wins. If it occurs nowhere, the comment is stale: shown at its recorded line,
//! marked so the reader knows not to trust it.

use serde::{Deserialize, Serialize};

use crate::review::{Comment, anchor_text};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnchorState {
    /// The recorded line still holds the recorded text (or there is no text to check).
    Exact,
    /// The text was found on a different line.
    Moved,
    /// The text is gone.
    Stale,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Anchored {
    pub comment: Comment,
    /// 1-based line where the comment is shown now, or `None` for a comment on the whole path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_line: Option<usize>,
    pub state: AnchorState,
}

impl Anchored {
    /// `line` or `line-end_line` where the comment sits now, `None` without a range.
    pub fn line_label(&self) -> Option<String> {
        Some(crate::review::line_label(self.line?, self.end_line))
    }

    /// The 0-based lines the comment is drawn over, `None` for a comment on the whole path.
    pub fn rows(&self) -> Option<std::ops::Range<usize>> {
        let first = self.line?;
        // Lines are 1-based, so `max(1)` matters only for a hand-written `on line 0`.
        Some(first.max(1) - 1..self.end_line.unwrap_or(first).max(1))
    }

    /// The 0-based line the comment is drawn under, `None` for a comment on the whole path.
    pub fn last_row(&self) -> Option<usize> {
        Some(self.rows()?.end - 1)
    }

    pub fn covers_row(&self, row: usize) -> bool {
        self.rows().is_some_and(|r| r.contains(&row))
    }

    /// `path:lines`, or just the path for a comment on the whole path.
    /// Where the comment is now, which is not where the file says once it has moved.
    pub fn location(&self) -> String {
        crate::review::location(&self.comment.path, self.line, self.end_line)
    }
}

/// Places `comment` in `lines` (the file's current content, one entry per line).
pub fn anchor(comment: &Comment, lines: &[&str]) -> Anchored {
    // A comment on the whole path has no line to place.
    let Some(first) = comment.line else {
        return Anchored {
            comment: comment.clone(),
            line: None,
            end_line: None,
            state: AnchorState::Exact,
        };
    };
    let span = comment.end_line.unwrap_or(first).saturating_sub(first);
    let placed = |line: usize, state| Anchored {
        comment: comment.clone(),
        line: Some(line),
        end_line: Some(line + span),
        state,
    };
    let Some(anchor) = &comment.anchor else {
        return placed(first, AnchorState::Exact);
    };
    let matches = |text: &str| anchor_text(text).as_deref() == Some(anchor.as_str());
    if let Some(current) = first.checked_sub(1).and_then(|i| lines.get(i)) {
        if matches(current) {
            return placed(first, AnchorState::Exact);
        }
    }
    let candidates: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter(|(_, text)| matches(text))
        .map(|(i, _)| i + 1)
        .collect();
    match candidates.iter().min_by_key(|&&l| l.abs_diff(first)) {
        Some(&line) => placed(line, AnchorState::Moved),
        None => placed(first, AnchorState::Stale),
    }
}

pub fn anchor_all(comments: &[&Comment], text: &str) -> Vec<Anchored> {
    let lines: Vec<&str> = text.lines().collect();
    comments.iter().map(|c| anchor(c, &lines)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn comment(line: usize, anchor: &str) -> Comment {
        let mut c = Comment::new("f", line, line + 1, "t");
        c.anchor = Some(anchor.to_string());
        c
    }

    #[test]
    fn exact_moved_stale() {
        let lines = ["a", "b", "c", "b"];
        assert_eq!(anchor(&comment(2, "b"), &lines).state, AnchorState::Exact);
        let moved = anchor(&comment(3, "b"), &lines);
        assert_eq!(moved.state, AnchorState::Moved);
        assert_eq!(moved.line, Some(2));
        assert_eq!(moved.end_line, Some(3));
        let moved = anchor(&comment(5, "b"), &lines);
        assert_eq!(moved.line, Some(4));
        assert_eq!(anchor(&comment(1, "zzz"), &lines).state, AnchorState::Stale);
        assert_eq!(
            anchor(&Comment::new("f", 9, 9, "t"), &lines).state,
            AnchorState::Exact
        );
    }

    #[test]
    fn path_comment_has_no_line() {
        let lines = ["a", "b"];
        let placed = anchor(&Comment::on_path("src", "t"), &lines);
        assert_eq!(placed.line, None);
        assert_eq!(placed.end_line, None);
        assert_eq!(placed.state, AnchorState::Exact);
        assert_eq!(placed.location(), "src");
    }
}
