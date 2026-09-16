//! `NOTES.md`: observations about the code that are not tasks.
//!
//! ```text
//! # Notes
//!
//! ## General
//! - The crate is split by front end; core never depends on either.
//!
//! ## src/repo.rs
//! - Shells out to git on purpose, see PLAN.md.
//! ```
//!
//! A `##` heading names a file (repository-relative) or `General`. Bullets under it are notes;
//! a note continues on following indented lines. Everything else is preserved verbatim.

use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

pub const FILE_NAME: &str = "NOTES.md";
pub const GENERAL: &str = "General";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Note {
    /// A file path, or [`GENERAL`].
    pub target: String,
    pub text: String,
}

impl Note {
    pub fn is_general(&self) -> bool {
        self.target == GENERAL
    }

    fn render(&self) -> String {
        let mut lines = self.text.lines();
        let mut out = format!("- {}", lines.next().unwrap_or(""));
        for line in lines {
            out.push_str("\n  ");
            out.push_str(line);
        }
        out
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Item {
    Note(Note),
    Raw(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Section {
    target: String,
    items: Vec<Item>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct NotesFile {
    preamble: Vec<String>,
    sections: Vec<Section>,
}

impl NotesFile {
    pub fn parse(text: &str) -> Self {
        let mut file = Self::default();
        for raw in text.lines() {
            if let Some(target) = raw.strip_prefix("## ") {
                file.sections.push(Section {
                    target: target.trim().to_string(),
                    items: Vec::new(),
                });
                continue;
            }
            let Some(section) = file.sections.last_mut() else {
                file.preamble.push(raw.to_string());
                continue;
            };
            if let Some(first) = raw.strip_prefix("- ") {
                section.items.push(Item::Note(Note {
                    target: section.target.clone(),
                    text: first.to_string(),
                }));
            } else if (raw.starts_with("  ") || raw.starts_with('\t'))
                && matches!(section.items.last(), Some(Item::Note(_)))
            {
                if let Some(Item::Note(note)) = section.items.last_mut() {
                    note.text.push('\n');
                    note.text.push_str(raw.trim_start());
                }
            } else {
                section.items.push(Item::Raw(raw.to_string()));
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
        if self.preamble.is_empty() {
            out.push_str("# Notes\n");
        }
        for line in &self.preamble {
            out.push_str(line);
            out.push('\n');
        }
        for section in &self.sections {
            if !out.ends_with("\n\n") && !out.is_empty() {
                out.push('\n');
            }
            out.push_str("## ");
            out.push_str(&section.target);
            out.push('\n');
            for item in &section.items {
                match item {
                    Item::Note(n) => out.push_str(&n.render()),
                    Item::Raw(r) => out.push_str(r),
                }
                out.push('\n');
            }
        }
        out
    }

    pub fn notes(&self) -> Vec<&Note> {
        self.sections
            .iter()
            .flat_map(|s| s.items.iter())
            .filter_map(|item| match item {
                Item::Note(n) => Some(n),
                Item::Raw(_) => None,
            })
            .collect()
    }

    pub fn notes_for(&self, target: &str) -> Vec<&Note> {
        self.notes()
            .into_iter()
            .filter(|n| n.target == target)
            .collect()
    }

    pub fn add(&mut self, note: Note) {
        let idx = match self.sections.iter().position(|s| s.target == note.target) {
            Some(i) => i,
            None => {
                let section = Section {
                    target: note.target.clone(),
                    items: Vec::new(),
                };
                if note.is_general() {
                    self.sections.insert(0, section);
                    0
                } else {
                    self.sections.push(section);
                    self.sections.len() - 1
                }
            }
        };
        let items = &mut self.sections[idx].items;
        let mut at = items.len();
        while at > 0 && matches!(&items[at - 1], Item::Raw(r) if r.trim().is_empty()) {
            at -= 1;
        }
        items.insert(at, Item::Note(note));
    }

    pub fn remove(&mut self, target: &Note) -> bool {
        for section in &mut self.sections {
            if let Some(i) = section
                .items
                .iter()
                .position(|item| matches!(item, Item::Note(n) if n == target))
            {
                section.items.remove(i);
                return true;
            }
        }
        false
    }

    pub fn replace(&mut self, target: &Note, updated: Note) -> bool {
        for section in &mut self.sections {
            for item in &mut section.items {
                if matches!(item, Item::Note(n) if n == target) {
                    *item = Item::Note(updated);
                    return true;
                }
            }
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str =
        "# Notes\n\n## General\n- One\n  continued\n- Two\n\n## src/a.rs\n- Three\n";

    #[test]
    fn round_trip() {
        let file = NotesFile::parse(SAMPLE);
        let notes = file.notes();
        assert_eq!(notes.len(), 3);
        assert_eq!(notes[0].text, "One\ncontinued");
        assert_eq!(notes[2].target, "src/a.rs");
        assert_eq!(file.render(), SAMPLE);
    }

    #[test]
    fn add_to_new_and_existing_sections() {
        let mut file = NotesFile::default();
        file.add(Note {
            target: "src/b.rs".into(),
            text: "B".into(),
        });
        file.add(Note {
            target: GENERAL.into(),
            text: "G".into(),
        });
        file.add(Note {
            target: "src/b.rs".into(),
            text: "B2".into(),
        });
        assert_eq!(
            file.render(),
            "# Notes\n\n## General\n- G\n\n## src/b.rs\n- B\n- B2\n"
        );
    }
}
