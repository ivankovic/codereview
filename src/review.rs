//! `REVIEW.md`: the actionable review comments, pending and completed.
//!
//! The format is the one `nvim-review` writes, so both tools can work on the same file:
//!
//! ```text
//! # Pending
//! - [Author] (2026-09-10 14:03:11) In src/main.rs on line 12: Wrong error type - "let x = ..."
//! - In src/lib.rs on line 3-7: Duplicated below
//! - In src/tui: Too many screens
//!
//! # Completed
//! - In README.md on line 1: Typo
//! ```
//!
//! Author and timestamp are optional. A range is `on line A-B`. The whole `on line` clause is
//! optional too: without it the comment is about the path as a whole, which is how a file or a
//! directory is commented on. The trailing quoted text is the line the comment was left on,
//! kept so the comment can be found again after the file changes (see [`crate::anchor`]); a
//! comment on a path has none. Anything the parser does not recognise (blank lines, prose,
//! custom headings) is preserved verbatim, so the file round-trips through this module
//! unchanged apart from the comment lines it edits. A path is read up to the first `: `, and
//! the `on line` clause may only be left out on a bullet at the left margin, so a nested prose
//! bullet stays prose but a top-level one reading `- In some cases: ...` parses as a comment on
//! the path `some cases`.

use std::path::Path;

use anyhow::{Context, Result};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::sync::OnceLock;

pub const FILE_NAME: &str = "REVIEW.md";
pub const PENDING: &str = "Pending";
pub const COMPLETED: &str = "Completed";

/// How much of the anchor line is kept in the file.
pub const ANCHOR_MAX_LEN: usize = 80;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Comment {
    /// Which heading the comment sits under: `Pending`, `Completed`, or a custom one.
    pub section: String,
    pub path: String,
    /// 1-based, inclusive. `None` when the comment is about the whole path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_line: Option<usize>,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<String>,
    /// The (trimmed, possibly truncated) text of the first line the comment was left on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anchor: Option<String>,
}

/// A comment is one line of Markdown. Text that carries line breaks of its own would be
/// read back as headings and further comments, so they become spaces on the way in.
fn one_line(text: String) -> String {
    if text.contains(['\n', '\r']) {
        text.split(['\n', '\r'])
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(" ")
    } else {
        text
    }
}

impl Comment {
    /// A comment on `path` lines `line..=end_line`.
    pub fn new(
        path: impl Into<String>,
        line: usize,
        end_line: usize,
        text: impl Into<String>,
    ) -> Self {
        Self {
            line: Some(line),
            end_line: Some(end_line.max(line)),
            ..Self::on_path(path, text)
        }
    }

    /// A comment on a whole file or directory, with no line range.
    pub fn on_path(path: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            section: PENDING.to_string(),
            path: one_line(path.into()),
            line: None,
            end_line: None,
            text: one_line(text.into()),
            author: None,
            timestamp: None,
            anchor: None,
        }
    }

    pub fn is_pending(&self) -> bool {
        self.section == PENDING
    }

    pub fn covers(&self, line: usize) -> bool {
        let (Some(first), Some(last)) = (self.line, self.end_line) else {
            return false;
        };
        (first..=last).contains(&line)
    }

    /// `line` or `line-end_line`, or `None` for a comment on the whole path.
    pub fn line_label(&self) -> Option<String> {
        Some(line_label(self.line?, self.end_line))
    }

    pub fn location(&self) -> String {
        match self.line_label() {
            Some(label) => format!("{}:{label}", self.path),
            None => self.path.clone(),
        }
    }

    /// Sets the anchor from the source line, trimmed and truncated the way nvim-review does.
    pub fn with_anchor_from(mut self, source_line: &str) -> Self {
        self.anchor = anchor_text(source_line);
        self
    }

    fn render(&self) -> String {
        let mut out = String::from("- ");
        if let Some(author) = &self.author {
            out.push('[');
            out.push_str(author);
            out.push_str("] ");
        }
        if let Some(ts) = &self.timestamp {
            out.push('(');
            out.push_str(ts);
            out.push_str(") ");
        }
        out.push_str("In ");
        out.push_str(&self.path);
        if let Some(label) = self.line_label() {
            out.push_str(&format!(" on line {label}"));
        }
        out.push_str(&format!(": {}", self.text));
        if let Some(anchor) = &self.anchor {
            out.push_str(" - \"");
            out.push_str(&anchor.replace('\\', "\\\\").replace('"', "\\\""));
            out.push('"');
        }
        out
    }
}

/// `line`, or `line-end` when `end` is past it.
pub fn line_label(line: usize, end: Option<usize>) -> String {
    match end {
        Some(end) if end > line => format!("{line}-{end}"),
        _ => line.to_string(),
    }
}

/// The trimmed first line, cut to [`ANCHOR_MAX_LEN`] with an ellipsis, or `None` when blank.
pub fn anchor_text(source_line: &str) -> Option<String> {
    let line = source_line.lines().next().unwrap_or("").trim();
    if line.is_empty() {
        return None;
    }
    if line.chars().count() > ANCHOR_MAX_LEN {
        let cut: String = line.chars().take(ANCHOR_MAX_LEN - 3).collect();
        Some(format!("{cut}..."))
    } else {
        Some(line.to_string())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Item {
    Comment(Comment),
    Raw(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Section {
    heading: String,
    items: Vec<Item>,
}

/// The whole file, comments plus every line that is not one.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ReviewFile {
    preamble: Vec<String>,
    sections: Vec<Section>,
}

impl ReviewFile {
    pub fn parse(text: &str) -> Self {
        let mut file = Self::default();
        for raw in text.lines() {
            if let Some(heading) = heading_of(raw) {
                file.sections.push(Section {
                    heading: heading.to_string(),
                    items: Vec::new(),
                });
                continue;
            }
            let Some(section) = file.sections.last_mut() else {
                file.preamble.push(raw.to_string());
                continue;
            };
            match parse_comment_line(raw, &section.heading) {
                Some(comment) => section.items.push(Item::Comment(comment)),
                None => section.items.push(Item::Raw(raw.to_string())),
            }
        }
        file
    }

    pub fn load(root: &Path) -> Result<Self> {
        let path = root.join(FILE_NAME);
        match std::fs::read_to_string(&path) {
            Ok(text) => Ok(Self::parse(&text)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e).with_context(|| format!("cannot read {}", path.display())),
        }
    }

    pub fn save(&self, root: &Path) -> Result<()> {
        let path = root.join(FILE_NAME);
        std::fs::write(&path, self.render())
            .with_context(|| format!("cannot write {}", path.display()))
    }

    pub fn render(&self) -> String {
        let mut out = String::new();
        for line in &self.preamble {
            out.push_str(line);
            out.push('\n');
        }
        for (i, section) in self.sections.iter().enumerate() {
            if i > 0 || !self.preamble.is_empty() {
                // A blank line before each heading, unless the previous line already is one.
                if !out.ends_with("\n\n") && !out.is_empty() {
                    out.push('\n');
                }
            }
            out.push_str("# ");
            out.push_str(&section.heading);
            out.push('\n');
            for item in &section.items {
                match item {
                    Item::Comment(c) => out.push_str(&c.render()),
                    Item::Raw(r) => out.push_str(r),
                }
                out.push('\n');
            }
        }
        out
    }

    pub fn sections(&self) -> Vec<&str> {
        self.sections.iter().map(|s| s.heading.as_str()).collect()
    }

    pub fn comments(&self) -> Vec<&Comment> {
        self.sections
            .iter()
            .flat_map(|s| s.items.iter())
            .filter_map(|item| match item {
                Item::Comment(c) => Some(c),
                Item::Raw(_) => None,
            })
            .collect()
    }

    pub fn comments_mut(&mut self) -> Vec<&mut Comment> {
        self.sections
            .iter_mut()
            .flat_map(|s| s.items.iter_mut())
            .filter_map(|item| match item {
                Item::Comment(c) => Some(c),
                Item::Raw(_) => None,
            })
            .collect()
    }

    pub fn comments_for(&self, path: &str) -> Vec<&Comment> {
        self.comments()
            .into_iter()
            .filter(|c| c.path == path)
            .collect()
    }

    pub fn pending_count(&self) -> usize {
        self.comments().iter().filter(|c| c.is_pending()).count()
    }

    fn section_mut(&mut self, heading: &str) -> &mut Section {
        if let Some(i) = self.sections.iter().position(|s| s.heading == heading) {
            return &mut self.sections[i];
        }
        // Pending goes first, everything else at the end, so a fresh file reads
        // Pending / Completed in that order.
        let section = Section {
            heading: heading.to_string(),
            items: Vec::new(),
        };
        if heading == PENDING {
            self.sections.insert(0, section);
            &mut self.sections[0]
        } else {
            self.sections.push(section);
            self.sections.last_mut().expect("just pushed")
        }
    }

    /// Adds a comment under its section, dropping the "No pending comments" placeholder
    /// nvim-review leaves in an empty section.
    pub fn add(&mut self, comment: Comment) {
        let section = self.section_mut(&comment.section.clone());
        section.items.retain(|item| match item {
            Item::Raw(r) => !r.trim().eq_ignore_ascii_case("No pending comments"),
            Item::Comment(_) => true,
        });
        // Insert before trailing blank lines so the section stays compact.
        let mut at = section.items.len();
        while at > 0 && matches!(&section.items[at - 1], Item::Raw(r) if r.trim().is_empty()) {
            at -= 1;
        }
        section.items.insert(at, Item::Comment(comment));
    }

    /// Removes the comment equal to `target`, returning whether one was found.
    pub fn remove(&mut self, target: &Comment) -> bool {
        for section in &mut self.sections {
            if let Some(i) = section
                .items
                .iter()
                .position(|item| matches!(item, Item::Comment(c) if c == target))
            {
                section.items.remove(i);
                return true;
            }
        }
        false
    }

    /// Replaces `target` with `updated` in place, or under `updated.section` when it changed.
    pub fn replace(&mut self, target: &Comment, updated: Comment) -> bool {
        if target.section == updated.section {
            for section in &mut self.sections {
                for item in &mut section.items {
                    if matches!(item, Item::Comment(c) if c == target) {
                        *item = Item::Comment(updated);
                        return true;
                    }
                }
            }
            false
        } else if self.remove(target) {
            self.add(updated);
            true
        } else {
            false
        }
    }

    /// Moves a comment between Pending and Completed.
    pub fn toggle(&mut self, target: &Comment) -> bool {
        let mut updated = target.clone();
        updated.section = if target.is_pending() {
            COMPLETED.to_string()
        } else {
            PENDING.to_string()
        };
        self.replace(target, updated)
    }

    /// Makes sure Pending and Completed both exist, for a file created from nothing.
    pub fn ensure_default_sections(&mut self) {
        self.section_mut(PENDING);
        self.section_mut(COMPLETED);
    }
}

fn heading_of(line: &str) -> Option<&str> {
    let rest = line.strip_prefix('#')?;
    let rest = rest.strip_prefix(' ').or_else(|| rest.strip_prefix('\t'))?;
    Some(rest.trim())
}

fn comment_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r#"^\s*-\s*(?:\[([^\]]*)\]\s*)?(?:\(([^)]*)\)\s*)?In ((?:[^:]|:[^ ])+?)(?: on line (\d+)(?:-(\d+))?)?: (.*)$"#,
        )
        .expect("valid regex")
    })
}

/// Parses one bullet; `None` for anything that is not a comment line.
pub fn parse_comment_line(line: &str, section: &str) -> Option<Comment> {
    let caps = comment_regex().captures(line)?;
    let author = caps.get(1).map(|m| m.as_str().trim().to_string());
    let timestamp = caps.get(2).map(|m| m.as_str().trim().to_string());
    let path = caps.get(3)?.as_str().to_string();
    let line_no: Option<usize> = match caps.get(4) {
        Some(m) => Some(m.as_str().parse().ok()?),
        None => None,
    };
    let end_line = line_no.map(|first| {
        caps.get(5)
            .and_then(|m| m.as_str().parse().ok())
            .unwrap_or(first)
            .max(first)
    });
    // An indented bullet without a line clause is a nested prose bullet, not a comment on a
    // path; reading it as one would reflow the file when it is written back.
    if line_no.is_none() && line.starts_with(char::is_whitespace) {
        return None;
    }
    let rest = caps.get(6)?.as_str();
    let (text, anchor) = split_anchor(rest);
    Some(Comment {
        section: section.to_string(),
        path,
        line: line_no,
        end_line,
        text,
        author: author.filter(|a| !a.is_empty()),
        timestamp: timestamp.filter(|t| !t.is_empty()),
        anchor,
    })
}

/// Splits `text - "anchor"` into its two parts. The anchor is the last ` - "..."` group ending
/// the line; a comment that contains ` - "` itself keeps it as long as it is not at the end.
fn split_anchor(rest: &str) -> (String, Option<String>) {
    let rest = rest.trim_end();
    if !rest.ends_with('"') {
        return (rest.to_string(), None);
    }
    let Some(idx) = rest.rfind(" - \"") else {
        return (rest.to_string(), None);
    };
    let text = rest[..idx].trim_end().to_string();
    let quoted = &rest[idx + 4..rest.len() - 1];
    let anchor = quoted.replace("\\\"", "\"").replace("\\\\", "\\");
    (text, Some(anchor))
}

/// Local time in nvim-review's default `timestamp_format`.
pub fn now_timestamp() -> String {
    chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "# Pending\n- [Ann] (2026-09-10 12:00:00) In src/main.rs on line 12: Wrong type - \"let x = 1;\"\n- In src/lib.rs on line 3-7: Duplicated\n\n# Completed\n- In README.md on line 1: Typo\n";

    #[test]
    fn parse_and_round_trip() {
        let file = ReviewFile::parse(SAMPLE);
        let comments = file.comments();
        assert_eq!(comments.len(), 3);
        assert_eq!(comments[0].author.as_deref(), Some("Ann"));
        assert_eq!(
            comments[0].timestamp.as_deref(),
            Some("2026-09-10 12:00:00")
        );
        assert_eq!(comments[0].anchor.as_deref(), Some("let x = 1;"));
        assert_eq!(comments[0].text, "Wrong type");
        assert_eq!(comments[1].line, Some(3));
        assert_eq!(comments[1].end_line, Some(7));
        assert!(!comments[2].is_pending());
        assert_eq!(file.render(), SAMPLE);
    }

    #[test]
    fn add_toggle_remove() {
        let mut file = ReviewFile::parse(SAMPLE);
        let c = Comment::new("x.rs", 5, 5, "Hm").with_anchor_from("  fn a() {  ");
        file.add(c.clone());
        assert_eq!(file.pending_count(), 3);
        assert!(
            file.render()
                .contains("- In x.rs on line 5: Hm - \"fn a() {\"")
        );
        assert!(file.toggle(&c));
        assert_eq!(file.pending_count(), 2);
        let moved = file
            .comments()
            .into_iter()
            .find(|k| k.path == "x.rs")
            .cloned()
            .unwrap();
        assert_eq!(moved.section, COMPLETED);
        assert!(file.remove(&moved));
        assert_eq!(file.comments().len(), 3);
    }

    #[test]
    fn placeholder_and_prose_preserved() {
        let text = "Intro line\n\n# Pending\nNo pending comments\n\n# Notes\nsome prose\n- not a comment\n";
        let mut file = ReviewFile::parse(text);
        assert_eq!(file.render(), text);
        file.add(Comment::new("a", 1, 1, "t"));
        let out = file.render();
        assert!(!out.contains("No pending comments"));
        assert!(out.contains("# Pending\n- In a on line 1: t\n\n# Notes"));
    }

    #[test]
    fn empty_file_gets_default_sections() {
        let mut file = ReviewFile::default();
        file.ensure_default_sections();
        assert_eq!(file.render(), "# Pending\n\n# Completed\n");
    }

    #[test]
    fn quotes_in_anchor() {
        let c = Comment::new("a", 1, 1, "t").with_anchor_from("say \"hi\"");
        let line = c.render();
        let back = parse_comment_line(&line, PENDING).unwrap();
        assert_eq!(back.anchor.as_deref(), Some("say \"hi\""));
        assert_eq!(back.text, "t");
    }

    #[test]
    fn nested_prose_bullets_stay_prose() {
        let text = "# Notes\n- Thoughts:\n  - In practice: this works\n";
        let file = ReviewFile::parse(text);
        assert!(file.comments().is_empty());
        assert_eq!(file.render(), text);
    }

    /// A comment is one line of the file. Text carrying its own line breaks would be read
    /// back as headings and further comments, so they must not survive.
    #[test]
    fn a_comment_cannot_forge_more_of_the_file() {
        let text = "fine\n# Completed\n- [Someone] In src/a.rs on line 1: forged";
        let comment = Comment::new("src/a.rs", 1, 1, text);
        assert!(!comment.text.contains('\n'));
        assert_eq!(
            comment.text,
            "fine # Completed - [Someone] In src/a.rs on line 1: forged"
        );
        let mut file = ReviewFile::default();
        file.ensure_default_sections();
        file.add(comment);
        let rendered = file.render();
        assert_eq!(
            rendered.lines().filter(|l| l.starts_with("# ")).count(),
            2,
            "only the two real headings: {rendered}"
        );
        let reread = ReviewFile::parse(&rendered);
        assert_eq!(reread.comments().len(), 1);
        assert_eq!(Comment::on_path("a\nb", "t").path, "a b");
    }

    #[test]
    fn comment_on_a_whole_path() {
        let text = "# Pending\n- In src/tui: too many screens\n- [Ann] In docs/a b.md: rewrite\n";
        let file = ReviewFile::parse(text);
        let comments = file.comments();
        assert_eq!(comments[0].path, "src/tui");
        assert_eq!(comments[0].line, None);
        assert_eq!(comments[0].end_line, None);
        assert_eq!(comments[0].text, "too many screens");
        assert_eq!(comments[0].line_label(), None);
        assert_eq!(comments[0].location(), "src/tui");
        assert!(!comments[0].covers(1));
        assert_eq!(comments[1].path, "docs/a b.md");
        assert_eq!(comments[1].author.as_deref(), Some("Ann"));
        assert_eq!(file.render(), text);
    }

    #[test]
    fn line_clause_wins_over_text_that_mentions_lines() {
        let c = parse_comment_line("- In a.rs: see on line 12: fix it", PENDING).unwrap();
        assert_eq!((c.path.as_str(), c.line), ("a.rs", None));
        assert_eq!(c.text, "see on line 12: fix it");
        let c = parse_comment_line("- In a.rs on line 5: see on line 12: fix it", PENDING).unwrap();
        assert_eq!((c.path.as_str(), c.line), ("a.rs", Some(5)));
        assert_eq!(c.text, "see on line 12: fix it");
        // A path with a colon followed by a space is not a path, so the bullet is left alone.
        assert!(
            parse_comment_line("- In note: a b on line 5: x", PENDING)
                .is_some_and(|c| c.path == "note")
        );
    }

    #[test]
    fn path_comment_round_trips() {
        let c = Comment::on_path("src/tui", "split this up: it does too much");
        assert_eq!(c.render(), "- In src/tui: split this up: it does too much");
        assert_eq!(parse_comment_line(&c.render(), PENDING).unwrap(), c);
    }

    #[test]
    fn dash_in_text_without_anchor() {
        let c = parse_comment_line("- In a on line 2: use x - not y", PENDING).unwrap();
        assert_eq!(c.text, "use x - not y");
        assert_eq!(c.anchor, None);
    }
}
