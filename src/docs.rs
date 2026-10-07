//! The documentation a test demonstrates: the sections of the repository's Markdown that say
//! what the test shows working.
//!
//! A test can name them, which settles it. Otherwise the closest sections are found by the
//! words they share with the test's name and its steps, weighed by how rare each word is
//! across all the sections, so that a word every section uses counts for nothing and one only
//! a few use counts for a lot. That is a guess, and the front ends say so: a reviewer reads a
//! guessed section to check it, not to rely on it.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

/// One section of a Markdown file: a heading and what is under it until the next heading.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Section {
    pub path: String,
    /// 1-based line of the heading.
    pub line: usize,
    pub heading: String,
}

/// A section a test leads to, and whether the test named it or it was guessed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DocLink {
    #[serde(flatten)]
    pub section: Section,
    pub declared: bool,
    /// The words it shares with the test, for a guess: why it was picked.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub shared: Vec<String>,
}

/// The Markdown that is the tool's own bookkeeping, not documentation of the software.
const NOT_DOCS: &[&str] = &["REVIEW.md", "NOTES.md", "OVERVIEW.md", "CHANGELOG.md"];

/// At most this many files, each at most this big, are read.
const MOST_FILES: usize = 200;
const MOST_BYTES: u64 = 1 << 20;

/// Words too common in prose about software to say anything about which section is meant.
const STOP: &[&str] = &[
    "a", "an", "the", "and", "or", "of", "to", "in", "on", "at", "by", "for", "with", "is", "are",
    "be", "it", "its", "as", "that", "this", "what", "when", "which", "from", "into", "one",
    "each", "every", "not", "no", "can", "does", "do", "has", "have", "if", "then", "so", "but",
    "all", "how", "test", "tests", "there", "their", "they", "them", "here", "up", "out", "after",
    "rs", "md", "js", "src", "py", "ts", "where", "way", "go", "goes", "back", "end", "ends",
];

/// A word as it is compared: lower case, with a plural or a tense taken off.
fn stem(word: &str) -> String {
    for suffix in ["ing", "ed", "es", "s"] {
        if word.len() > suffix.len() + 3 {
            if let Some(rest) = word.strip_suffix(suffix) {
                return rest.to_string();
            }
        }
    }
    word.to_string()
}

/// The words of `text` that can tell sections apart.
fn significant(text: &str) -> Vec<String> {
    crate::spec::words(text)
        .into_iter()
        .filter(|w| {
            w.len() > 1 && !STOP.contains(&w.as_str()) && !w.chars().all(|c| c.is_ascii_digit())
        })
        .map(|w| stem(&w))
        .collect()
}

/// A section with the words of its heading and of its body.
struct Read {
    section: Section,
    heading: HashSet<String>,
    body: HashSet<String>,
}

/// Every section of every Markdown file in `files`, read from the repository at `root`.
fn sections(root: &std::path::Path, files: &[String]) -> Vec<Read> {
    let mut out = Vec::new();
    let docs = files
        .iter()
        .filter(|f| f.to_ascii_lowercase().ends_with(".md"))
        .filter(|f| !NOT_DOCS.contains(&f.rsplit('/').next().unwrap_or(f)))
        .take(MOST_FILES);
    for path in docs {
        let full = root.join(path);
        if std::fs::metadata(&full).map_or(true, |m| m.len() > MOST_BYTES) {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&full) else {
            continue;
        };
        let mut fenced = false;
        let mut current: Option<(Section, String)> = None;
        let close = |current: &mut Option<(Section, String)>, out: &mut Vec<Read>| {
            if let Some((section, body)) = current.take() {
                out.push(Read {
                    heading: significant(&section.heading).into_iter().collect(),
                    body: significant(&body).into_iter().collect(),
                    section,
                });
            }
        };
        for (i, line) in text.lines().enumerate() {
            if line.trim_start().starts_with("```") {
                fenced = !fenced;
            }
            let hashes = line.chars().take_while(|c| *c == '#').count();
            if !fenced && (1..=6).contains(&hashes) && line[hashes..].starts_with(' ') {
                close(&mut current, &mut out);
                current = Some((
                    Section {
                        path: path.clone(),
                        line: i + 1,
                        heading: line[hashes..].trim().to_string(),
                    },
                    String::new(),
                ));
            } else if let Some((_, body)) = current.as_mut() {
                body.push_str(line);
                body.push('\n');
            }
        }
        close(&mut current, &mut out);
    }
    out
}

/// The sections `declared` names, as `path#heading` (the heading as written, or as a link
/// to it would spell it), then, if it names none, the closest to `words`: at most `most`,
/// each sharing at least two of them.
pub fn for_test(
    root: &std::path::Path,
    files: &[String],
    declared: &[String],
    words: &str,
    most: usize,
) -> Vec<DocLink> {
    let all = sections(root, files);
    let anchor = |h: &str| {
        h.to_lowercase()
            .chars()
            .filter_map(|c| match c {
                c if c.is_alphanumeric() => Some(c),
                ' ' | '-' => Some('-'),
                _ => None,
            })
            .collect::<String>()
    };
    let named: Vec<DocLink> = declared
        .iter()
        .filter_map(|d| {
            let (path, heading) = d.split_once('#').unwrap_or((d, ""));
            all.iter()
                .find(|r| {
                    r.section.path == path
                        && (heading.is_empty()
                            || r.section.heading.eq_ignore_ascii_case(heading)
                            || anchor(&r.section.heading) == anchor(heading))
                })
                .map(|r| DocLink {
                    section: r.section.clone(),
                    declared: true,
                    shared: Vec::new(),
                })
        })
        .collect();
    if !named.is_empty() || !declared.is_empty() {
        return named;
    }

    let wanted: HashSet<String> = significant(words).into_iter().collect();
    let mut df: HashMap<&str, usize> = HashMap::new();
    for r in &all {
        for w in r.heading.union(&r.body) {
            *df.entry(w.as_str()).or_default() += 1;
        }
    }
    let n = all.len().max(1) as f64;
    // A long section shares words with anything, so a match in one counts for less, the way
    // BM25 weighs a term in a long document.
    let length = |r: &Read| r.heading.len() + r.body.len();
    let average = all.iter().map(length).sum::<usize>() as f64 / n;
    let mut scored: Vec<(f64, &Read, Vec<String>)> = all
        .iter()
        .filter_map(|r| {
            let mut shared: Vec<String> = wanted
                .iter()
                .filter(|w| r.heading.contains(*w) || r.body.contains(*w))
                .cloned()
                .collect();
            if shared.len() < 2 {
                return None;
            }
            shared.sort();
            let damp = 1.0 + 1.2 * (0.25 + 0.75 * length(r) as f64 / average.max(1.0));
            let score: f64 = shared
                .iter()
                .map(|w| {
                    let idf = (1.0 + n / *df.get(w.as_str()).unwrap_or(&1) as f64).ln();
                    if r.heading.contains(w) {
                        3.0 * idf
                    } else {
                        2.2 * idf / damp
                    }
                })
                .sum();
            Some((score, r, shared))
        })
        .collect();
    scored.sort_by(|a, b| b.0.total_cmp(&a.0));
    scored
        .into_iter()
        .take(most)
        .map(|(_, r, shared)| DocLink {
            section: r.section.clone(),
            declared: false,
            shared,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_test_leads_to_the_section_it_names_or_the_closest_one() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::write(
            root.join("README.md"),
            "# tool\n\nIt reviews code.\n\n## Comments\n\nA comment is left on a line and saved \
             to the review file.\n\n```\n# not a heading\n```\n\n## Pictures\n\nA picture is \
             shown fitted to the screen.\n\n## Colours\n\nThe colour scheme is saved.\n",
        )
        .unwrap();
        std::fs::write(
            root.join("REVIEW.md"),
            "# Pending\n\n- a comment on a line\n",
        )
        .unwrap();
        let files = vec!["README.md".to_string(), "REVIEW.md".to_string()];

        let guessed = for_test(root, &files, &[], "a comment left on a line is saved", 2);
        assert_eq!(guessed[0].section.heading, "Comments");
        assert_eq!(guessed[0].section.line, 5);
        assert!(!guessed[0].declared);
        assert!(
            guessed[0].shared.contains(&"comment".to_string()),
            "{guessed:?}"
        );
        assert!(
            guessed.iter().all(|g| g.section.path == "README.md"),
            "the review file is not documentation"
        );

        let named = for_test(root, &files, &["README.md#pictures".into()], "anything", 2);
        assert_eq!(named.len(), 1);
        assert_eq!(
            (named[0].section.heading.as_str(), named[0].declared),
            ("Pictures", true)
        );
        // A name that leads nowhere is not replaced by a guess: the test said what it meant.
        assert!(for_test(root, &files, &["README.md#gone".into()], "comment line", 2).is_empty());
        assert!(for_test(root, &files, &[], "quantum flux", 2).is_empty());
    }
}
