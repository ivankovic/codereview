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

//! Where a reader can go from where they are. Given a place in a file — a line and a column — it
//! works out what is there (the name under the cursor, whether the line defines it or uses it,
//! the definition around it) and the reasonable next places, in groups: the definition, the
//! usages, what the definition around the line uses, the tests that ran the line or name what it
//! is in, the documents that mention it, the name's places among the review range's files, the
//! commit that last changed the line, and the review's comments on the definition.
//! Both front ends show the same groups from here: the terminal's `J` menu and default `Enter`,
//! the page's menu on a name.
//!
//! Nothing here resolves a name to what it means: the index is tree-sitter definitions and
//! identifier occurrences, so a name defined twice has two definitions, and the usages of a
//! common name include another thing's. Each place says where it is, and the reader decides.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::session::Session;
use crate::symbols::Symbol;

/// One place to go: a line of a file, what is there, and why it is offered.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Place {
    pub path: String,
    /// 1-based.
    pub line: usize,
    /// The line, trimmed, or what the place is (a test's sentence, a heading).
    pub text: String,
    /// What it is to the place the reader is at: `in parse_review`, `ran it`, `under Saving`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// A commit to show the file's change in, rather than the file: the history group's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
}

/// What a group of places is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Definition,
    Usages,
    /// What the definition around the line uses that the repository defines.
    Uses,
    Tests,
    Docs,
    /// The name's definitions and usages among the files the review range changed.
    InRange,
    /// The commit that last changed the line.
    History,
    /// The review's comments on the definition around the line, or on the whole file.
    Comments,
}

impl Kind {
    /// The key that opens the group in a menu.
    pub fn key(self) -> char {
        match self {
            Kind::Definition => 'd',
            Kind::Usages => 'u',
            Kind::Uses => 'c',
            Kind::Tests => 't',
            Kind::Docs => 'o',
            Kind::InRange => 'r',
            Kind::History => 'h',
            Kind::Comments => 'm',
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Kind::Definition => "Definition",
            Kind::Usages => "Usages",
            Kind::Uses => "It uses",
            Kind::Tests => "Tests",
            Kind::Docs => "Docs",
            Kind::InRange => "In this range",
            Kind::History => "History",
            Kind::Comments => "Comments",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Group {
    pub kind: Kind,
    /// What a menu calls it, and the letter that moves to it there: sent with the group so
    /// the page draws a group it has not heard of as well as the ones it has.
    pub title: String,
    pub key: char,
    pub places: Vec<Place>,
}

/// What is at a place, and where one can go from it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Here {
    /// The name under the cursor, if it is on one.
    pub name: Option<String>,
    /// Whether the line defines that name, rather than uses it.
    pub defines: bool,
    /// The innermost definition the line is in, as `kind name`.
    pub within: Option<String>,
    /// The groups with anything in them, in the order a menu lists them.
    pub groups: Vec<Group>,
}

impl Here {
    pub fn group(&self, kind: Kind) -> Option<&Group> {
        self.groups.iter().find(|g| g.kind == kind)
    }

    /// Where `Enter` goes: on a use, its definition — the one in this file first, then the
    /// nearest by path; on a definition, the usages, as a list to pick from.
    pub fn default_step(&self, from: &str) -> Option<Step> {
        if !self.defines
            && let Some(defs) = self.group(Kind::Definition)
        {
            let best = defs
                .places
                .iter()
                .max_by_key(|p| shared_prefix(&p.path, from))?;
            return Some(match defs.places.len() {
                1 => Step::Go(best.clone()),
                _ => Step::Choose(Kind::Definition),
            });
        }
        self.group(Kind::Usages).map(|_| Step::Choose(Kind::Usages))
    }
}

/// What the default step is: straight to one place, or a group to choose from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    Go(Place),
    Choose(Kind),
}

/// How many leading path components two paths share: nearness, for picking among definitions.
fn shared_prefix(a: &str, b: &str) -> usize {
    a.split('/')
        .zip(b.split('/'))
        .take_while(|(x, y)| x == y)
        .count()
}

/// At most this many places a group lists: past that, a list is a search, which `/` is for.
const MOST: usize = 200;

/// What is at `line` (0-based) and byte `column` of `path`, and where one can go from there,
/// with no review range open.
pub fn here(session: &mut Session, path: &str, line: usize, column: usize) -> Here {
    here_in(session, path, line, column, &HashSet::new())
}

/// [`here`], with the files the open review range changed, for the "in this range" group.
pub fn here_in(
    session: &mut Session,
    path: &str,
    line: usize,
    column: usize,
    changed: &HashSet<String>,
) -> Here {
    let name = session.identifier_at(path, line, column);
    let symbols = session.symbols_in(path);
    // The innermost definition whose lines hold this one.
    let within = symbols
        .iter()
        .filter(|s| (s.line..=s.end_line).contains(&(line + 1)))
        .max_by_key(|s| s.line)
        .cloned();
    let mut groups = Vec::new();
    let mut defines = false;
    if let Some(name) = &name {
        let defs = session.definitions(name);
        defines = defs.iter().any(|d| d.path == path && d.line == line + 1);
        let definitions: Vec<Place> = defs
            .iter()
            .filter(|d| !(d.path == path && d.line == line + 1))
            .map(|d| Place {
                path: d.path.clone(),
                line: d.line,
                text: d.text.clone(),
                note: Some(d.kind.clone()),
                commit: None,
            })
            .collect();
        push(&mut groups, Kind::Definition, definitions);
        let usages: Vec<Place> = session
            .references(name)
            .into_iter()
            .filter(|r| !(r.path == path && r.line == line + 1))
            .filter(|r| !defs.iter().any(|d| d.path == r.path && d.line == r.line))
            // A document mentioning the name is the docs group's.
            .filter(|r| !crate::docs::is_document(&r.path))
            .take(MOST)
            .map(|r| {
                let note = enclosing(session, &r.path, r.line).map(|s| format!("in {}", s.name));
                Place {
                    path: r.path,
                    line: r.line,
                    text: r.text,
                    note,
                    commit: None,
                }
            })
            .collect();
        push(&mut groups, Kind::Usages, usages);
    }
    if let Some(def) = &within {
        push(&mut groups, Kind::Uses, uses(session, def));
    }
    push(
        &mut groups,
        Kind::Tests,
        tests(session, path, line + 1, within.as_ref()),
    );
    let topic = name
        .clone()
        .or_else(|| within.as_ref().map(|s| s.name.clone()));
    if let Some(topic) = &topic {
        push(&mut groups, Kind::Docs, docs(session, topic));
    }
    if let Some(name) = &name
        && !changed.is_empty()
    {
        push(
            &mut groups,
            Kind::InRange,
            in_range(session, name, path, line + 1, changed),
        );
    }
    push(&mut groups, Kind::History, history(session, path, line));
    push(
        &mut groups,
        Kind::Comments,
        comments(session, path, line + 1, within.as_ref()),
    );
    Here {
        name,
        defines,
        within: within.map(|s| format!("{} {}", s.kind, s.name)),
        groups,
    }
}

fn push(groups: &mut Vec<Group>, kind: Kind, places: Vec<Place>) {
    if !places.is_empty() {
        groups.push(Group {
            kind,
            title: kind.title().to_string(),
            key: kind.key(),
            places,
        });
    }
}

/// The innermost definition of `path` around 1-based `line`.
fn enclosing(session: &mut Session, path: &str, line: usize) -> Option<Symbol> {
    session
        .symbols_in(path)
        .into_iter()
        .filter(|s| s.line <= line && line <= s.end_line)
        .max_by_key(|s| s.line)
}

/// The tests that went through 1-based `line` of `path`: every end-to-end test whose recorded
/// run executed it, then every test that names the definition around it.
fn tests(session: &mut Session, path: &str, line: usize, within: Option<&Symbol>) -> Vec<Place> {
    let root = session.root().to_path_buf();
    let mut out: Vec<Place> = Vec::new();
    for recording in crate::recording::find(&root) {
        let Some(ran) = crate::recording::lines_run(&root, &recording) else {
            continue;
        };
        if ran
            .get(path)
            .is_some_and(|lines| lines.contains(&(line as u32)))
        {
            out.push(Place {
                path: recording.file.clone(),
                line: recording.line.max(1),
                text: recording.test.replace('_', " "),
                note: Some("its run went through this line".into()),
                commit: None,
            });
        }
    }
    if let Some(def) = within {
        let test_code = |session: &mut Session, p: &str, l: usize| {
            session
                .index()
                .test_rows(p)
                .iter()
                .any(|(a, b)| (*a..=*b).contains(&(l.saturating_sub(1) as u32)))
        };
        let refs = session.references(&def.name);
        for r in refs {
            if (r.path == path && r.line == line) || !test_code(session, &r.path, r.line) {
                continue;
            }
            let test = enclosing(session, &r.path, r.line);
            let place = Place {
                path: r.path.clone(),
                line: test.as_ref().map_or(r.line, |t| t.line),
                text: test
                    .as_ref()
                    .map_or(r.text.clone(), |t| t.name.replace('_', " ")),
                note: Some(format!("names {}", def.name)),
                commit: None,
            };
            if !out
                .iter()
                .any(|p| p.path == place.path && p.line == place.line)
            {
                out.push(place);
            }
        }
    }
    out.truncate(MOST);
    out
}

/// The lines of the repository's documents that mention `name` as a word, each under the
/// heading it is in.
fn docs(session: &Session, name: &str) -> Vec<Place> {
    if name.chars().count() < 3 {
        return Vec::new();
    }
    let root = session.root();
    let mut out = Vec::new();
    for path in session.files.iter().filter(|f| crate::docs::is_document(f)) {
        let Ok(text) = std::fs::read_to_string(root.join(path)) else {
            continue;
        };
        let mut heading: Option<String> = None;
        for (i, line) in text.lines().enumerate() {
            if let Some(h) = line.strip_prefix('#') {
                heading = Some(h.trim_start_matches('#').trim().to_string());
                continue;
            }
            if mentions(line, name) {
                out.push(Place {
                    path: path.clone(),
                    line: i + 1,
                    text: line.trim().to_string(),
                    note: heading.as_ref().map(|h| format!("under {h}")),
                    commit: None,
                });
            }
            if out.len() >= MOST {
                return out;
            }
        }
    }
    out
}

/// What `def` uses that the repository defines: every name in its lines, in the order they first
/// come, that has a definition other than `def` itself, at that definition — in `def`'s file
/// first, then the nearest by path.
fn uses(session: &mut Session, def: &Symbol) -> Vec<Place> {
    let Ok(Some(bytes)) = session.repo.read_working(&def.path) else {
        return Vec::new();
    };
    let text = String::from_utf8_lossy(&bytes);
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    // The definition's own line names it; the body is what it uses.
    for line in text
        .lines()
        .skip(def.line)
        .take(def.end_line.saturating_sub(def.line))
    {
        for word in words(line) {
            if word == def.name || word.len() < 2 || !seen.insert(word.to_string()) {
                continue;
            }
            let found = session.definitions(word);
            let Some(best) = found
                .iter()
                .filter(|d| !(d.path == def.path && d.line == def.line))
                .max_by_key(|d| (d.path == def.path, shared_prefix(&d.path, &def.path)))
            else {
                continue;
            };
            out.push(Place {
                path: best.path.clone(),
                line: best.line,
                text: best.text.clone(),
                note: Some(format!("{} {}", best.kind, best.name)),
                commit: None,
            });
            if out.len() >= MOST {
                return out;
            }
        }
    }
    out
}

/// The identifiers of a line of code: runs of letters, digits and underscores that do not start
/// with a digit.
fn words(line: &str) -> impl Iterator<Item = &str> {
    line.split(|c: char| !(c.is_alphanumeric() || c == '_'))
        .filter(|w| {
            w.chars()
                .next()
                .is_some_and(|c| c.is_alphabetic() || c == '_')
        })
}

/// Where `name` is defined or used in the files the range changed, other than here.
fn in_range(
    session: &mut Session,
    name: &str,
    path: &str,
    line: usize,
    changed: &HashSet<String>,
) -> Vec<Place> {
    let defs: Vec<(String, usize)> = session
        .definitions(name)
        .into_iter()
        .map(|d| (d.path, d.line))
        .collect();
    session
        .references(name)
        .into_iter()
        .filter(|r| changed.contains(&r.path) && !(r.path == path && r.line == line))
        .take(MOST)
        .map(|r| {
            let defines = defs.iter().any(|(p, l)| *p == r.path && *l == r.line);
            Place {
                note: Some(if defines { "defined here" } else { "used here" }.to_string()),
                path: r.path,
                line: r.line,
                text: r.text,
                commit: None,
            }
        })
        .collect()
}

/// The commit that last changed 0-based `line` of `path`, by `git blame`; nothing for a line not
/// committed yet, or a file git does not track.
fn history(session: &Session, path: &str, line: usize) -> Vec<Place> {
    let Ok(blame) = session.repo.blame(path) else {
        return Vec::new();
    };
    let Some(b) = blame.get(line) else {
        return Vec::new();
    };
    if b.hash.chars().all(|c| c == '0') {
        return Vec::new();
    }
    vec![Place {
        path: path.to_string(),
        line: line + 1,
        text: b.summary.clone(),
        note: Some(format!(
            "{} · {} · {}",
            &b.hash[..b.hash.len().min(7)],
            b.author,
            b.date
        )),
        commit: Some(b.hash.clone()),
    }]
}

/// The review's comments on `path` within `within`'s lines, or on 1-based `line` when it is in
/// no definition, and those on the whole file.
fn comments(session: &Session, path: &str, line: usize, within: Option<&Symbol>) -> Vec<Place> {
    let (from, to) = within.map_or((line, line), |d| (d.line, d.end_line));
    session
        .review
        .comments()
        .into_iter()
        .filter(|c| c.path == path)
        .filter(|c| {
            c.line.is_none_or(|l| {
                let end = c.end_line.unwrap_or(l);
                l <= to && from <= end
            })
        })
        .map(|c| Place {
            path: c.path.clone(),
            line: c.line.unwrap_or(1),
            text: c.text.clone(),
            note: Some(match (&c.author, c.line) {
                (Some(a), Some(_)) => format!("{a} · {}", c.section),
                (Some(a), None) => format!("{a} · on the whole file · {}", c.section),
                (None, Some(_)) => c.section.clone(),
                (None, None) => format!("on the whole file · {}", c.section),
            }),
            commit: None,
        })
        .collect()
}

/// Whether `line` has `name` as a whole word: not inside a longer identifier.
fn mentions(line: &str, name: &str) -> bool {
    let word = |c: char| c.is_alphanumeric() || c == '_';
    line.match_indices(name).any(|(at, _)| {
        let before = line[..at].chars().next_back();
        let after = line[at + name.len()..].chars().next();
        !before.is_some_and(word) && !after.is_some_and(word)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::tests::{scratch_repo, scratch_session};

    fn write(root: &std::path::Path, path: &str, text: &str) {
        let path = root.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    #[test]
    fn a_name_is_whole_words_only() {
        assert!(mentions("see `parse_review` here", "parse_review"));
        assert!(!mentions("see reparse_review", "parse_review"));
        assert!(!mentions("parse_reviewer", "parse_review"));
    }

    /// From a use: the definition, the other usages with the function each is in, the tests
    /// that name the function the line is in, and the documents that mention the name.
    #[test]
    fn from_a_use_the_definition_usages_tests_and_docs() {
        let dir = scratch_repo();
        let root = dir.path();
        write(
            root,
            "src/core.rs",
            "pub fn parse_review() {}\n\npub fn load() {\n    parse_review();\n}\n\npub fn save() {\n    parse_review();\n}\n\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn loading_works() {\n        super::load();\n    }\n}\n",
        );
        write(
            root,
            "README.md",
            "# tool\n\n## Parsing\n\nEverything goes through parse_review first.\n",
        );
        let mut session = scratch_session(root);
        session.refresh().unwrap();
        // Line 4, `parse_review();` inside `load`, on the name.
        let here = super::here(&mut session, "src/core.rs", 3, 6);
        assert_eq!(here.name.as_deref(), Some("parse_review"));
        assert!(!here.defines);
        assert_eq!(here.within.as_deref(), Some("function load"));
        let defs = here.group(Kind::Definition).expect("its definition");
        assert_eq!(
            (defs.places[0].path.as_str(), defs.places[0].line),
            ("src/core.rs", 1)
        );
        let usages = here.group(Kind::Usages).expect("the other use");
        assert_eq!(usages.places.len(), 1, "{usages:?}");
        assert_eq!(usages.places[0].line, 8);
        assert_eq!(usages.places[0].note.as_deref(), Some("in save"));
        let tests = here.group(Kind::Tests).expect("the test that names load");
        assert_eq!(tests.places[0].text, "loading works");
        let docs = here.group(Kind::Docs).expect("the README mentions it");
        assert_eq!(docs.places[0].note.as_deref(), Some("under Parsing"));
        assert_eq!(
            here.default_step("src/core.rs"),
            Some(Step::Go(defs.places[0].clone()))
        );
    }

    /// From a definition, `Enter` offers its usages rather than going nowhere.
    #[test]
    fn from_a_definition_the_default_is_its_usages() {
        let dir = scratch_repo();
        let root = dir.path();
        write(
            root,
            "src/core.rs",
            "pub fn parse_review() {}\n\npub fn load() {\n    parse_review();\n}\n",
        );
        let mut session = scratch_session(root);
        session.refresh().unwrap();
        let here = here(&mut session, "src/core.rs", 0, 8);
        assert!(here.defines);
        assert!(here.group(Kind::Definition).is_none(), "{here:?}");
        assert_eq!(
            here.default_step("src/core.rs"),
            Some(Step::Choose(Kind::Usages))
        );
    }

    /// From inside a definition: what it uses, the commit that last changed the line, the
    /// review's comments on it, and, with a range open, the name's places in what it changed.
    #[test]
    fn what_it_uses_its_history_its_comments_and_the_range() {
        let dir = scratch_repo();
        let root = dir.path();
        write(
            root,
            "src/core.rs",
            "pub fn parse_review() {}\n\npub fn load() {\n    parse_review();\n}\n",
        );
        write(
            root,
            "src/other.rs",
            "pub fn more() {\n    crate::parse_review();\n}\n",
        );
        let git = |args: &[&str]| {
            assert!(
                std::process::Command::new("git")
                    .args(args)
                    .current_dir(root)
                    .status()
                    .unwrap()
                    .success()
            )
        };
        git(&["add", "."]);
        git(&["commit", "-q", "-m", "load the review"]);
        let mut session = scratch_session(root);
        session.refresh().unwrap();
        session
            .add_comment("src/core.rs", Some((4, 4)), "why here?", None, None)
            .unwrap();
        let changed: HashSet<String> = ["src/other.rs".to_string()].into();
        let here = here_in(&mut session, "src/core.rs", 3, 6, &changed);
        let uses = here.group(Kind::Uses).expect("load uses parse_review");
        assert_eq!(
            uses.places[0].note.as_deref(),
            Some("function parse_review")
        );
        let range = here.group(Kind::InRange).expect("other.rs is in the range");
        assert_eq!(range.places.len(), 1, "{range:?}");
        assert_eq!(range.places[0].path, "src/other.rs");
        let history = here.group(Kind::History).expect("the line is committed");
        assert_eq!(history.places[0].text, "load the review");
        assert!(history.places[0].commit.is_some());
        let comments = here.group(Kind::Comments).expect("the comment on load");
        assert_eq!(comments.places[0].text, "why here?");
        // Without a range there is no range group.
        let here = super::here(&mut session, "src/core.rs", 3, 6);
        assert!(here.group(Kind::InRange).is_none());
    }
}
