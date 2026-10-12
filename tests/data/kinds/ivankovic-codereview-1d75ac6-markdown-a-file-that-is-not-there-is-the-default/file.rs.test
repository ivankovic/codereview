//! What REVIEW.md and NOTES.md have in common.
//!
//! Both are a Markdown file a person may edit by hand: a preamble, then sections under
//! headings, each holding entries of one kind and whatever prose someone wrote around them.
//! Both are read whole, rewritten whole, and must come back looking as they went in. The
//! grammar of a line differs between them, and lives in their own modules; the file handling
//! and the one rule about where a new entry goes are here.

use std::path::Path;

use anyhow::{Context, Result};

/// Reads the file at `root/name` and parses it. A file that is not there is not an error:
/// a repository with nothing written about it yet is the ordinary case.
pub fn load<T: Default>(root: &Path, name: &str, parse: impl Fn(&str) -> T) -> Result<T> {
    let path = root.join(name);
    match std::fs::read_to_string(&path) {
        Ok(text) => Ok(parse(&text)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(T::default()),
        Err(e) => Err(e).with_context(|| format!("cannot read {}", path.display())),
    }
}

pub fn save(root: &Path, name: &str, text: &str) -> Result<()> {
    let path = root.join(name);
    std::fs::write(&path, text).with_context(|| format!("cannot write {}", path.display()))
}

/// Puts `item` at the end of a section's items, but above any blank lines trailing it, so
/// that a new entry joins the ones already there instead of appearing after the gap that
/// separates the section from the next heading.
pub fn append_to_section<T>(items: &mut Vec<T>, item: T, is_blank: impl Fn(&T) -> bool) {
    let mut at = items.len();
    while at > 0 && is_blank(&items[at - 1]) {
        at -= 1;
    }
    items.insert(at, item);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_entry_goes_above_the_blank_lines() {
        let mut items = vec!["one".to_string(), String::new(), String::new()];
        append_to_section(&mut items, "two".to_string(), |s| s.trim().is_empty());
        assert_eq!(items, vec!["one", "two", "", ""]);

        // Nothing there yet, and nothing but blanks, both work.
        let mut empty: Vec<String> = Vec::new();
        append_to_section(&mut empty, "one".to_string(), |s| s.trim().is_empty());
        assert_eq!(empty, vec!["one"]);
        let mut blanks = vec![String::new()];
        append_to_section(&mut blanks, "one".to_string(), |s| s.trim().is_empty());
        assert_eq!(blanks, vec!["one", ""]);
    }

    #[test]
    fn a_file_that_is_not_there_is_the_default() {
        let dir = tempfile::tempdir().unwrap();
        let count = |text: &str| text.lines().count();
        assert_eq!(load(dir.path(), "NOPE.md", count).unwrap(), 0);
        save(dir.path(), "THERE.md", "a\nb\n").unwrap();
        assert_eq!(load(dir.path(), "THERE.md", count).unwrap(), 2);
    }
}
