/*  This file is part of codereview.
 *
 *  Copyright (C) 2026 Marko Ivankovic
 *
 *  This program is free software: you can redistribute it and/or modify
 *  it under the terms of the GNU Affero General Public License as published
 *  by the Free Software Foundation, version 3 of the License.
 *
 *  This program is distributed in the hope that it will be useful,
 *  but WITHOUT ANY WARRANTY; without even the implied warranty of
 *  MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
 *  GNU Affero General Public License for more details.
 *
 *  You should have received a copy of the GNU Affero General Public License
 *  along with this program.  If not, see <https://www.gnu.org/licenses/>.
 */

//! One search for going somewhere new: what is typed is looked for in the open file, the
//! definitions, the paths, the tests, the documents' headings and, slower and so asked for on
//! its own, every tracked file's text. Results come in those groups, the best first within each.
//! A name and a path match fuzzily — the letters in order, a word's first letters worth more —
//! so `gRv` finds `getReviewer` and `revrs` finds `src/review.rs`.
//!
//! What is near ranks higher: something in the file open, in its directory, or in the review
//! range's files. A prefix narrows the search to one group: `f:` paths, `s:` symbols, `t:` tests,
//! `d:` documents, `c:` comments; `"…"` is exact text and `/…/` a regular expression, both
//! through the text search. Both front ends ask here.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::session::Session;

/// Which group a result is in, in the order they are shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum Group {
    InFile,
    Symbols,
    Files,
    Tests,
    Docs,
    Comments,
    Text,
}

impl Group {
    pub fn title(self) -> &'static str {
        match self {
            Group::InFile => "In this file",
            Group::Symbols => "Symbols",
            Group::Files => "Files",
            Group::Tests => "Tests",
            Group::Docs => "Docs",
            Group::Comments => "Comments",
            Group::Text => "Text",
        }
    }
}

/// One result: where it is, and what to show of it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hit {
    pub group: Group,
    pub path: String,
    /// 1-based; 0 for a path, which opens at its top.
    pub line: usize,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// Higher is better, within its group.
    #[serde(skip)]
    pub score: i64,
}

/// Where the reader is, for ranking what is near higher.
#[derive(Debug, Clone, Default)]
pub struct Near {
    /// The file open, and its text, which "In this file" searches.
    pub file: Option<(String, String)>,
    /// The review range's files.
    pub changed: HashSet<String>,
}

/// What a query asks for: which groups, and what to match.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Query {
    pub only: Option<Group>,
    pub text: String,
    /// Exact text (`"…"`), or a regular expression (`/…/`): the text search only.
    pub literal: Literal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Literal {
    No,
    Exact,
    Regex,
}

impl Query {
    pub fn parse(raw: &str) -> Self {
        let raw = raw.trim();
        let prefixed = [
            ("f:", Group::Files),
            ("s:", Group::Symbols),
            ("t:", Group::Tests),
            ("d:", Group::Docs),
            ("c:", Group::Comments),
        ]
        .into_iter()
        .find_map(|(p, g)| raw.strip_prefix(p).map(|rest| (g, rest.trim())));
        if let Some((group, text)) = prefixed {
            return Query {
                only: Some(group),
                text: text.to_string(),
                literal: Literal::No,
            };
        }
        if raw.len() >= 2 && raw.starts_with('"') && raw.ends_with('"') {
            return Query {
                only: Some(Group::Text),
                text: raw[1..raw.len() - 1].to_string(),
                literal: Literal::Exact,
            };
        }
        if raw.len() >= 2 && raw.starts_with('/') && raw.ends_with('/') {
            return Query {
                only: Some(Group::Text),
                text: raw[1..raw.len() - 1].to_string(),
                literal: Literal::Regex,
            };
        }
        Query {
            only: None,
            text: raw.to_string(),
            literal: Literal::No,
        }
    }

    fn wants(&self, group: Group) -> bool {
        self.only.is_none_or(|g| g == group)
    }
}

/// At most this many results a group keeps.
pub const MOST: usize = 50;

/// Everything but the text search, which is slower and asked for with [`text`]: the groups in
/// their order, each best first.
pub fn search(session: &mut Session, raw: &str, near: &Near) -> Vec<(Group, Vec<Hit>)> {
    let q = Query::parse(raw);
    if q.text.is_empty() || q.literal != Literal::No {
        return Vec::new();
    }
    let mut groups: Vec<(Group, Vec<Hit>)> = Vec::new();
    let nearness = |path: &str| -> i64 {
        let mut bonus = 0;
        if near.file.as_ref().is_some_and(|(p, _)| p == path) {
            bonus += 30;
        } else if let Some((p, _)) = &near.file
            && dir_of(p) == dir_of(path)
        {
            bonus += 12;
        }
        if near.changed.contains(path) {
            bonus += 20;
        }
        bonus
    };

    if q.wants(Group::InFile)
        && let Some((path, text)) = &near.file
    {
        let hits: Vec<Hit> = text
            .lines()
            .enumerate()
            .filter(|(_, line)| contains_smart(line, &q.text))
            .take(MOST)
            .map(|(i, line)| Hit {
                group: Group::InFile,
                path: path.clone(),
                line: i + 1,
                text: line.trim().to_string(),
                note: None,
                score: -(i as i64),
            })
            .collect();
        push(&mut groups, Group::InFile, hits);
    }

    if q.wants(Group::Symbols) {
        let mut hits: Vec<Hit> = session
            .index()
            .symbols()
            .filter_map(|s| {
                let score = fuzzy(&q.text, &s.name)?;
                Some(Hit {
                    group: Group::Symbols,
                    path: s.path.clone(),
                    line: s.line,
                    text: s.name.clone(),
                    note: Some(format!("{} · {}:{}", s.kind, s.path, s.line)),
                    score: score + nearness(&s.path),
                })
            })
            .collect();
        best(&mut hits);
        push(&mut groups, Group::Symbols, hits);
    }

    if q.wants(Group::Files) {
        let mut hits: Vec<Hit> = session
            .files
            .iter()
            .filter_map(|path| {
                let score = fuzzy(&q.text, path)?;
                Some(Hit {
                    group: Group::Files,
                    path: path.clone(),
                    line: 0,
                    text: path.clone(),
                    note: None,
                    // A file's own name matching beats its directories doing it.
                    score: score
                        + fuzzy(&q.text, name_of(path)).map_or(0, |s| s / 2)
                        + nearness(path),
                })
            })
            .collect();
        best(&mut hits);
        push(&mut groups, Group::Files, hits);
    }

    if q.wants(Group::Tests) {
        let tests = session.index().tests();
        let mut hits: Vec<Hit> = tests
            .into_iter()
            .filter_map(|t| {
                let score = fuzzy_words(&q.text, &t.label)?;
                Some(Hit {
                    group: Group::Tests,
                    path: t.path.clone(),
                    line: t.line,
                    text: t.label.clone(),
                    note: Some(format!("{}:{}", t.path, t.line)),
                    score: score + nearness(&t.path),
                })
            })
            .collect();
        best(&mut hits);
        push(&mut groups, Group::Tests, hits);
    }

    if q.wants(Group::Docs) {
        let mut hits = Vec::new();
        for path in session.files.iter().filter(|f| crate::docs::is_document(f)) {
            let Ok(text) = std::fs::read_to_string(session.root().join(path)) else {
                continue;
            };
            for (i, line) in text.lines().enumerate() {
                let Some(heading) = line.strip_prefix('#') else {
                    continue;
                };
                let heading = heading.trim_start_matches('#').trim();
                if let Some(score) = fuzzy_words(&q.text, heading) {
                    hits.push(Hit {
                        group: Group::Docs,
                        path: path.clone(),
                        line: i + 1,
                        text: heading.to_string(),
                        note: Some(format!("{path}:{}", i + 1)),
                        score,
                    });
                }
            }
        }
        best(&mut hits);
        push(&mut groups, Group::Docs, hits);
    }

    if q.wants(Group::Comments) {
        let hits: Vec<Hit> = session
            .review
            .comments()
            .iter()
            .filter(|c| contains_smart(&c.text, &q.text))
            .take(MOST)
            .map(|c| Hit {
                group: Group::Comments,
                path: c.path.clone(),
                line: c.line.unwrap_or(0),
                text: c.text.clone(),
                note: Some(match c.line {
                    Some(l) => format!("{}:{l}", c.path),
                    None => c.path.clone(),
                }),
                score: 0,
            })
            .collect();
        push(&mut groups, Group::Comments, hits);
    }
    groups
}

/// The text search: every tracked file's lines that hold the query — smart case, or exactly,
/// or as a regular expression — through `git grep`, which is what makes it slow enough to be
/// asked for on its own.
pub fn text(repo: &crate::repo::Repo, raw: &str, near: &Near) -> Vec<Hit> {
    let q = Query::parse(raw);
    if q.text.is_empty() || !q.wants(Group::Text) {
        return Vec::new();
    }
    let ignore_case = q.literal == Literal::No && q.text.chars().all(|c| !c.is_uppercase());
    let found = repo
        .grep(&q.text, q.literal == Literal::Regex, ignore_case, MOST * 4)
        .unwrap_or_default();
    let mut hits: Vec<Hit> = found
        .into_iter()
        .map(|(path, line, text)| {
            let score = if near.changed.contains(&path) { 20 } else { 0 }
                + if near.file.as_ref().is_some_and(|(p, _)| *p == path) {
                    30
                } else {
                    0
                };
            Hit {
                group: Group::Text,
                note: Some(format!("{path}:{line}")),
                path,
                line,
                text: text.trim().to_string(),
                score,
            }
        })
        .collect();
    best(&mut hits);
    hits
}

fn push(groups: &mut Vec<(Group, Vec<Hit>)>, group: Group, hits: Vec<Hit>) {
    if !hits.is_empty() {
        groups.push((group, hits));
    }
}

/// Best first, at most [`MOST`].
fn best(hits: &mut Vec<Hit>) {
    hits.sort_by(|a, b| {
        b.score
            .cmp(&a.score)
            .then(a.text.len().cmp(&b.text.len()))
            .then(a.path.cmp(&b.path))
    });
    hits.truncate(MOST);
}

fn dir_of(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(d, _)| d)
}

fn name_of(path: &str) -> &str {
    path.rsplit_once('/').map_or(path, |(_, n)| n)
}

/// Whether `text` holds `query`: case-insensitively unless the query has a capital.
fn contains_smart(text: &str, query: &str) -> bool {
    if query.chars().any(char::is_uppercase) {
        text.contains(query)
    } else {
        text.to_lowercase().contains(&query.to_lowercase())
    }
}

/// How well `query` matches `candidate` as letters in order, or `None` when it does not:
/// a letter at the start, after a separator or at a capital (a word's start) is worth more,
/// one right after the last is worth more, the whole name or its start matching is worth most,
/// and every letter skipped costs a little. Case-insensitive unless the query has a capital.
pub fn fuzzy(query: &str, candidate: &str) -> Option<i64> {
    if query.is_empty() {
        return None;
    }
    let exact_case = query.chars().any(char::is_uppercase);
    let fold = |c: char| {
        if exact_case {
            c
        } else {
            c.to_ascii_lowercase()
        }
    };
    let q: Vec<char> = query.chars().map(fold).collect();
    let c: Vec<char> = candidate.chars().collect();
    let mut score: i64 = 0;
    let mut qi = 0;
    let mut last: Option<usize> = None;
    for (i, &ch) in c.iter().enumerate() {
        if qi == q.len() {
            break;
        }
        // A capital, or the start of a word, matches a query letter of either case.
        if fold(ch) != q[qi]
            && !(exact_case && ch.eq_ignore_ascii_case(&q[qi]) && starts_word(&c, i))
        {
            continue;
        }
        let mut gain = 1;
        if starts_word(&c, i) {
            gain += 8;
        }
        if last.is_some_and(|l| l + 1 == i) {
            gain += 5;
        }
        if let Some(l) = last {
            score -= (i - l - 1).min(6) as i64;
        } else {
            score -= i.min(12) as i64;
        }
        score += gain;
        last = Some(i);
        qi += 1;
    }
    if qi < q.len() {
        return None;
    }
    let whole: String = c.iter().map(|ch| fold(*ch)).collect();
    let q: String = q.into_iter().collect();
    if whole == q {
        score += 100;
    } else if whole.starts_with(&q) {
        score += 40;
    }
    Some(score)
}

/// Whether `c[i]` starts a word: the first letter, after a separator, or a capital after a
/// lower-case letter.
fn starts_word(c: &[char], i: usize) -> bool {
    if i == 0 {
        return true;
    }
    let prev = c[i - 1];
    !prev.is_alphanumeric() || (c[i].is_uppercase() && prev.is_lowercase())
}

/// A sentence matched word by word: every word of the query a prefix of some word of the
/// sentence, in any order. For tests and headings, which are read as words.
fn fuzzy_words(query: &str, sentence: &str) -> Option<i64> {
    let words: Vec<String> = sentence
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_lowercase)
        .collect();
    let mut score = 0;
    for q in query.split_whitespace().map(str::to_lowercase) {
        let best = words
            .iter()
            .filter(|w| w.starts_with(&q))
            .map(|w| if *w == q { 10 } else { 6 })
            .max()?;
        score += best;
    }
    (score > 0).then_some(score)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::tests::{scratch_repo, scratch_session};

    /// One query finds a definition, a path, a test, a heading and, asked for apart, the text,
    /// and what the file open or the range holds comes first.
    #[test]
    fn one_query_looks_everywhere_and_near_comes_first() {
        let dir = scratch_repo();
        let root = dir.path();
        let put = |path: &str, text: &str| {
            let path = root.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        };
        put("src/review.rs", "pub fn save_review() {}\n");
        put("lib/review.rs", "pub fn save_review() {}\n");
        put(
            "tests/saving.rs",
            "#[test]\nfn a_review_survives_saving() {\n    save_review();\n}\n",
        );
        put("README.md", "# Tool\n\n## Saving a review\n\nSave often.\n");
        let mut session = scratch_session(root);
        session.refresh().unwrap();
        let near = Near {
            file: None,
            changed: ["lib/review.rs".to_string()].into(),
        };
        let groups = search(&mut session, "save_review", &near);
        let symbols = &groups.iter().find(|(g, _)| *g == Group::Symbols).unwrap().1;
        assert_eq!(
            symbols[0].path, "lib/review.rs",
            "the range's first: {symbols:?}"
        );
        let groups = search(&mut session, "revrs", &near);
        let files = &groups.iter().find(|(g, _)| *g == Group::Files).unwrap().1;
        assert_eq!(files[0].path, "lib/review.rs", "{files:?}");
        let groups = search(&mut session, "survives saving", &near);
        assert!(
            groups
                .iter()
                .any(|(g, h)| *g == Group::Tests && h[0].path == "tests/saving.rs")
        );
        let groups = search(&mut session, "d:saving", &near);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].1[0].text, "Saving a review");
        let text = text(&session.repo, "save often", &near);
        assert_eq!((text[0].path.as_str(), text[0].line), ("README.md", 5));
        assert!(
            super::text(&session.repo, "\"save often\"", &near).is_empty(),
            "exact is exact"
        );
    }

    #[test]
    fn a_name_matches_by_its_words_first_letters() {
        assert!(fuzzy("gRv", "getReviewer").is_some());
        assert!(fuzzy("revrs", "src/review.rs").is_some());
        assert!(fuzzy("zz", "getReviewer").is_none());
        // The whole name beats its start, which beats letters scattered through it.
        let whole = fuzzy("review", "review").unwrap();
        let start = fuzzy("review", "review_file").unwrap();
        let scattered = fuzzy("review", "read_every_view").unwrap();
        assert!(
            whole > start && start > scattered,
            "{whole} {start} {scattered}"
        );
    }

    #[test]
    fn a_prefix_narrows_the_search() {
        assert_eq!(Query::parse("f: main").only, Some(Group::Files));
        assert_eq!(Query::parse("t:saves").text, "saves");
        let exact = Query::parse("\"fn main\"");
        assert_eq!(
            (exact.only, exact.literal),
            (Some(Group::Text), Literal::Exact)
        );
        assert_eq!(Query::parse("/a.*b/").literal, Literal::Regex);
        assert_eq!(Query::parse("plain").only, None);
    }

    #[test]
    fn a_sentence_matches_word_by_word() {
        assert!(fuzzy_words("comment restart", "a comment survives a restart").is_some());
        assert!(fuzzy_words("surv", "a comment survives a restart").is_some());
        assert!(fuzzy_words("nothing", "a comment survives a restart").is_none());
    }
}
