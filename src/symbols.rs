//! Symbol navigation without a language server: definitions and identifier occurrences,
//! extracted with the tree-sitter grammars omnidiff already carries, for every file in the
//! repository. Good enough to answer "where is this defined" and "where is this used" in a
//! tree that changes too fast for anyone to keep a language server happy, and it works the
//! same for every language that has a grammar.
//!
//! Definitions come from one generic rule: a node whose kind ends in `_definition`,
//! `_declaration`, `_declarator`, `_item`, `_spec`, `_specifier` or `_signature` and has a
//! `name` child field
//! (or a `type` field, for a Rust `impl`). Occurrences are every leaf node whose kind contains
//! `identifier`. Files without a grammar (Makefiles) get a small regex fallback.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use omnidiff::code::Language;
use omnidiff::code::language::{language_for_path_and_content, to_treesitter};
use serde::{Deserialize, Serialize};
use tree_sitter::{Node, Parser, Point};

use crate::testsuite::{Found, TestCase};

/// The most occurrences one query answers with. A common identifier in a large repository
/// has tens of thousands, and each one costs a line of text and a file read.
pub const MAX_REFERENCES: usize = 2000;

/// Whether `name` equals the already-lowercased `lower`, ignoring case.
fn eq_fold(name: &str, lower: &str) -> bool {
    if name.is_ascii() {
        name.eq_ignore_ascii_case(lower)
    } else {
        name.to_lowercase() == lower
    }
}

/// Whether `name` starts with the already-lowercased `lower`, ignoring case.
fn starts_with_fold(name: &str, lower: &str) -> bool {
    if !name.is_ascii() {
        return name.to_lowercase().starts_with(lower);
    }
    name.as_bytes()
        .get(..lower.len())
        .is_some_and(|head| head.eq_ignore_ascii_case(lower.as_bytes()))
}

/// Whether `name` holds the already-lowercased `lower` anywhere, ignoring case.
fn contains_fold(name: &str, lower: &str) -> bool {
    if !name.is_ascii() {
        return name.to_lowercase().contains(lower);
    }
    if lower.is_empty() {
        return true;
    }
    lower.len() <= name.len()
        && name
            .as_bytes()
            .windows(lower.len())
            .any(|w| w.eq_ignore_ascii_case(lower.as_bytes()))
}

/// Files above this size are not indexed; nothing a reviewer navigates by symbol is that big.
/// The overview's TODO count draws the same line, for the same reason.
pub(crate) const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Symbol {
    pub name: String,
    /// `function`, `struct`, `class`, `method`, `impl`, `target`, ...
    pub kind: String,
    pub path: String,
    /// 1-based.
    pub line: usize,
    /// 0-based byte column of the name.
    pub column: usize,
    pub end_line: usize,
    /// The enclosing definition's name, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub container: Option<String>,
    /// The source line, trimmed.
    pub text: String,
}

/// One occurrence of an identifier.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Location {
    pub path: String,
    /// 1-based.
    pub line: usize,
    /// 0-based byte column.
    pub column: usize,
    pub text: String,
}

#[derive(Debug, Clone)]
struct Ident {
    name: String,
    line: u32,
    column: u32,
}

#[derive(Debug, Clone)]
struct FileEntry {
    len: u64,
    modified: Option<SystemTime>,
    symbols: Vec<Symbol>,
    /// Occurrences, with the name interned: a common identifier appears tens of thousands
    /// of times in a large repository and its name is worth storing once.
    idents: Vec<Occurrence>,
    /// The tests the file defines, which of its rows are test code, and the modules it
    /// declares for tests only.
    tests: Vec<TestCase>,
    test_code: Vec<(u32, u32)>,
    test_modules: Vec<String>,
}

/// One occurrence of an identifier, by name number.
#[derive(Debug, Clone, Copy)]
struct Occurrence {
    name: u32,
    line: u32,
    column: u32,
}

/// The names seen so far, each with a number. Names are never dropped: there are only so
/// many distinct identifiers in a repository, and a number that meant something must keep
/// meaning it.
#[derive(Debug, Default)]
struct Names {
    ids: HashMap<String, u32>,
}

impl Names {
    fn intern(&mut self, name: &str) -> u32 {
        if let Some(&id) = self.ids.get(name) {
            return id;
        }
        let id = self.ids.len() as u32;
        self.ids.insert(name.to_string(), id);
        id
    }

    fn id_of(&self, name: &str) -> Option<u32> {
        self.ids.get(name).copied()
    }
}

/// Whether a name is this project's vocabulary or the language's.
///
/// Matching is by spelling and nothing else — there is no resolver here — so a file defining
/// a method called `push` collects every `Vec::push` in the repository. What separates the
/// two, without knowing a single language's standard library, is shape: `Session`,
/// `parse_review` and `AnchorState` are a project's own words, while `push`, `open`, `text`
/// and `len` are everybody's. A capital, a separator, or length is a coarse test, and it
/// costs some genuinely local short names like `diff` — which are also the ones most likely
/// to collide with everybody else's.
fn worth_counting(name: &str) -> bool {
    name.chars().count() >= 8
        || name.chars().any(|c| c.is_uppercase())
        || name.contains('_')
        || name.contains('-')
}

/// How many files may define a name before it says nothing about who leans on whom. Three
/// files with a `Config` between them are still telling you something; twenty with a `new`
/// are not.
const SHARED_NAME: usize = 3;

/// How far one file's definitions reach into the rest of the tree.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Reach {
    pub path: String,
    /// Other files reached, with each name shared out among the files that define it. A
    /// number to sort by and to compare with the others here; it means nothing on its own.
    pub score: f64,
    /// The names that carried it, widest first.
    pub names: Vec<String>,
}

#[derive(Debug, Default)]
pub struct Index {
    root: PathBuf,
    files: HashMap<String, FileEntry>,
    names: Names,
}

impl Index {
    /// Indexes `paths` (repository-relative) under `root`, in parallel.
    pub fn build(root: &Path, paths: &[String]) -> Self {
        let mut index = Self {
            root: root.to_path_buf(),
            files: HashMap::new(),
            names: Names::default(),
        };
        index.refresh(paths);
        index
    }

    /// Re-indexes files whose size or modification time changed, indexes new ones, and drops
    /// ones no longer listed. Says whether anything did change, so what was worked out from
    /// the index need not be worked out again when nothing did.
    pub fn refresh(&mut self, paths: &[String]) -> bool {
        let listed: std::collections::HashSet<&str> = paths.iter().map(String::as_str).collect();
        let before = self.files.len();
        self.files.retain(|p, _| listed.contains(p.as_str()));
        let mut changed = self.files.len() != before;
        let mut stale = Vec::new();
        for p in paths {
            let meta = std::fs::metadata(self.root.join(p)).ok();
            match (self.files.get(p.as_str()), meta) {
                (Some(entry), Some(meta)) => {
                    if entry.len != meta.len() || entry.modified != meta.modified().ok() {
                        stale.push(p.clone());
                    }
                }
                (None, Some(_)) => stale.push(p.clone()),
                // Listed but gone from disk: nothing to navigate to any more.
                (Some(_), None) => {
                    self.files.remove(p.as_str());
                    changed = true;
                }
                (None, None) => {}
            }
        }
        changed |= !stale.is_empty();
        for (path, entry) in index_files(&self.root, &stale) {
            let entry = self.take_in(entry);
            self.files.insert(path, entry);
        }
        changed
    }

    /// Replaces one file's entry from text held in memory rather than read from disk. The
    /// entry carries no modification time, so the next `refresh` sees it as stale and indexes
    /// the file on disk again: this is for looking something up now, not for overriding what
    /// the repository says.
    pub fn update_text(&mut self, path: &str, text: &str) {
        let (symbols, idents, found) = index_text(path, text);
        let entry = self.take_in(RawEntry {
            len: text.len() as u64,
            modified: None,
            symbols,
            idents,
            found,
        });
        self.files.insert(path.to_string(), entry);
    }

    /// Turns a worker's entry into a stored one, giving every name its number.
    fn take_in(&mut self, raw: RawEntry) -> FileEntry {
        let idents = raw
            .idents
            .into_iter()
            .map(|i| Occurrence {
                name: self.names.intern(&i.name),
                line: i.line,
                column: i.column,
            })
            .collect();
        FileEntry {
            len: raw.len,
            modified: raw.modified,
            symbols: raw.symbols,
            idents,
            tests: raw.found.tests,
            test_code: raw.found.code,
            test_modules: raw.found.test_modules,
        }
    }

    pub fn file_count(&self) -> usize {
        self.files.len()
    }

    pub fn symbol_count(&self) -> usize {
        self.files.values().map(|f| f.symbols.len()).sum()
    }

    /// How far each file's definitions reach: for every name a file defines, the number of
    /// *other* files that mention it, divided by how many files define that name.
    ///
    /// Three things keep this from being a list of whoever spelled `new` most often, and the
    /// first two were found by running it over a codebase whose answer was already known:
    ///
    /// - **Fields are not definitions here.** `path`, `name` and `text` are declared by
    ///   twenty different structs and written in every file that touches one, which drowned
    ///   everything else. A file is leaned on for what it *is* — its types, its functions,
    ///   its modules — not for having a field with a popular name.
    /// - **A name defined in more than [`SHARED_NAME`] files is nobody's.** `new`, `run` and
    ///   `tests` say nothing about who depends on whom, and no amount of dividing rescues
    ///   them; they are dropped outright.
    /// - **A plain short word is everybody's**, since nothing here resolves a name to what it
    ///   means: see [`worth_counting`].
    ///
    /// What is left is divided anyway: a name defined in one place and used in forty is that
    /// file's reach, and one defined in three is worth a third to each.
    ///
    /// One pass over the symbols and one over the occurrences, rather than a query per name:
    /// [`definitions`](Self::definitions) walks every file, and there are thousands of names.
    /// Returns every file that reaches anything, highest first, each with the names that
    /// carried it.
    pub fn reach(&self) -> Vec<Reach> {
        // Files as indices, so the sets below are sets of integers.
        let paths: Vec<&String> = {
            let mut p: Vec<&String> = self.files.keys().collect();
            p.sort();
            p
        };
        // Which files define each name, and which mention it.
        let defined = self.defined(&paths, false);
        let mut mentioned: HashMap<u32, HashSet<usize>> = HashMap::new();
        for (i, path) in paths.iter().enumerate() {
            for occurrence in &self.files[*path].idents {
                mentioned.entry(occurrence.name).or_default().insert(i);
            }
        }

        // A name's weight, and where it was spent.
        let mut scores: HashMap<usize, (f64, Vec<(String, usize)>)> = HashMap::new();
        let by_id: HashMap<u32, &str> = self
            .names
            .ids
            .iter()
            .map(|(name, id)| (*id, name.as_str()))
            .collect();
        for (id, definers) in &defined {
            if definers.len() > SHARED_NAME {
                continue;
            }
            let Some(users) = mentioned.get(id) else {
                continue;
            };
            let share = 1.0 / definers.len() as f64;
            for &definer in definers {
                let elsewhere = users.iter().filter(|&&u| u != definer).count();
                if elsewhere == 0 {
                    continue;
                }
                let entry = scores.entry(definer).or_insert((0.0, Vec::new()));
                entry.0 += elsewhere as f64 * share;
                if let Some(name) = by_id.get(id) {
                    entry.1.push(((*name).to_string(), elsewhere));
                }
            }
        }

        let mut out: Vec<Reach> = scores
            .into_iter()
            .map(|(file, (score, mut names))| {
                // The names that carried the file, widest first, and only a few of them.
                names.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
                names.truncate(4);
                Reach {
                    path: paths[file].to_string(),
                    score,
                    names: names.into_iter().map(|(name, _)| name).collect(),
                }
            })
            .collect();
        out.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(a.path.cmp(&b.path))
        });
        out
    }

    /// Which of `paths` (by position) define each name worth counting: not a field, and not a
    /// plain short word. With `outside_tests`, a name defined in test code does not count —
    /// a test's own helpers are not code under test.
    fn defined(&self, paths: &[&String], outside_tests: bool) -> HashMap<u32, Vec<usize>> {
        let test_files = if outside_tests {
            self.test_files()
        } else {
            HashSet::new()
        };
        let mut defined: HashMap<u32, Vec<usize>> = HashMap::new();
        for (i, path) in paths.iter().enumerate() {
            let entry = &self.files[*path];
            if test_files.contains(path.as_str()) {
                continue;
            }
            for symbol in &entry.symbols {
                if !worth_counting(&symbol.name) || symbol.kind == "field" {
                    continue;
                }
                if outside_tests && Found::covers(&entry.test_code, symbol.line as u32 - 1) {
                    continue;
                }
                if let Some(id) = self.names.id_of(&symbol.name) {
                    let files = defined.entry(id).or_default();
                    if !files.contains(&i) {
                        files.push(i);
                    }
                }
            }
        }
        defined
    }

    /// The rows of `path` that are test code, 0-based and inclusive: a test module inside a
    /// file of code, or the whole of a test file.
    pub fn test_rows(&self, path: &str) -> Vec<(u32, u32)> {
        self.files
            .get(path)
            .map(|f| f.test_code.clone())
            .unwrap_or_default()
    }

    /// The files that are test code throughout: under a `tests` directory, named the way a
    /// runner collects them, or declared by a Rust module under `#[cfg(test)]` elsewhere.
    fn test_files(&self) -> HashSet<&str> {
        let mut out: HashSet<&str> = self
            .files
            .iter()
            .filter(|(_, f)| Found::whole(&f.test_code))
            .map(|(p, _)| p.as_str())
            .collect();
        for (path, entry) in &self.files {
            for name in &entry.test_modules {
                for candidate in crate::testsuite::module_files(path, name) {
                    if let Some((p, _)) = self.files.get_key_value(&candidate) {
                        out.insert(p.as_str());
                    }
                }
            }
        }
        out
    }

    /// The files each test names, by the test's path and first line: whatever its body
    /// mentions that is defined outside test code, by the same rules [`Self::untested`]
    /// uses. A test that drives the system from outside — a browser, a command line — names
    /// nothing, and is not credited with what it reaches that way.
    pub fn test_reach(&self) -> HashMap<(String, usize), Vec<String>> {
        let paths: Vec<&String> = {
            let mut p: Vec<&String> = self.files.keys().collect();
            p.sort();
            p
        };
        let defined = self.defined(&paths, true);
        let mut out = HashMap::new();
        for path in &paths {
            let entry = &self.files[*path];
            for test in &entry.tests {
                let from = test.line.saturating_sub(1) as u32;
                let to = test.end.max(test.line).saturating_sub(1) as u32;
                let mut files: BTreeSet<&str> = BTreeSet::new();
                for occurrence in entry
                    .idents
                    .iter()
                    .filter(|o| (from..=to).contains(&o.line))
                {
                    let Some(definers) = defined.get(&occurrence.name) else {
                        continue;
                    };
                    if definers.len() > SHARED_NAME {
                        continue;
                    }
                    files.extend(definers.iter().map(|&d| paths[d].as_str()));
                }
                out.insert(
                    ((*path).clone(), test.line),
                    files.into_iter().map(str::to_string).collect(),
                );
            }
        }
        out
    }

    /// The files whose code a test could reach: in a language whose tests are found here,
    /// and not test code throughout.
    pub fn code_files(&self) -> Vec<String> {
        let tests = self.test_files();
        let mut out: Vec<String> = self
            .files
            .keys()
            .filter(|p| crate::testsuite::testable(p) && !tests.contains(p.as_str()))
            .cloned()
            .collect();
        out.sort();
        out
    }

    /// Every test the repository defines, by path then line.
    pub fn tests(&self) -> Vec<TestCase> {
        let mut out: Vec<TestCase> = self
            .files
            .values()
            .flat_map(|f| f.tests.iter().cloned())
            .collect();
        out.sort_by(|a, b| a.path.cmp(&b.path).then(a.line.cmp(&b.line)));
        out
    }

    /// Of the files [`reach`](Self::reach) ranks, the ones no test code mentions, in the same
    /// order: leaned on the most, and with nothing to say when a change breaks them.
    ///
    /// A file counts as reached when it has tests of its own, or when test code anywhere names
    /// something it defines outside its tests, with the same rules `reach` uses for what a
    /// name is worth. Only files in a language whose tests are found here are listed: a
    /// Makefile or a page of HTML leaned on by everything is not untested, it is untestable
    /// in the sense this means. Only test code counts: a Rust file with a
    /// test module also imports half the crate in its ordinary code, and none of that is
    /// tested by being imported. Nothing here resolves a name, so a file can be counted as
    /// reached by a test that only shares a word with it; it is never counted the other way.
    pub fn untested(&self) -> Vec<Reach> {
        let paths: Vec<&String> = {
            let mut p: Vec<&String> = self.files.keys().collect();
            p.sort();
            p
        };
        let defined = self.defined(&paths, true);
        let test_files = self.test_files();
        let mut reached: HashSet<&str> = HashSet::new();
        for path in &paths {
            let entry = &self.files[*path];
            if !entry.tests.is_empty() {
                reached.insert(path.as_str());
            }
            let whole = test_files.contains(path.as_str());
            if entry.test_code.is_empty() && !whole {
                continue;
            }
            for occurrence in &entry.idents {
                if !whole && !Found::covers(&entry.test_code, occurrence.line) {
                    continue;
                }
                let Some(definers) = defined.get(&occurrence.name) else {
                    continue;
                };
                if definers.len() > SHARED_NAME {
                    continue;
                }
                for &d in definers {
                    reached.insert(paths[d].as_str());
                }
            }
        }
        self.reach()
            .into_iter()
            .filter(|r| {
                crate::testsuite::testable(&r.path)
                    && !reached.contains(r.path.as_str())
                    && !test_files.contains(r.path.as_str())
            })
            .collect()
    }

    /// Every definition named exactly `name`, by path then line.
    pub fn definitions(&self, name: &str) -> Vec<Symbol> {
        let mut out: Vec<Symbol> = self
            .files
            .values()
            .flat_map(|f| f.symbols.iter().filter(|s| s.name == name).cloned())
            .collect();
        out.sort_by(|a, b| a.path.cmp(&b.path).then(a.line.cmp(&b.line)));
        out
    }

    /// Every occurrence of `name`, definitions included, by path then line, at most
    /// [`MAX_REFERENCES`] of them. Each file holding one is read to quote the line, so the
    /// cap bounds the disk work as well as the answer. The cap is what
    /// keeps a one-letter identifier in a large repository from reading most of the working
    /// tree on every request; `len() == MAX_REFERENCES` means there were probably more.
    pub fn references(&self, name: &str) -> Vec<Location> {
        let mut out = Vec::new();
        let Some(wanted) = self.names.id_of(name) else {
            return out;
        };
        // Sorted, so the cap keeps the same occurrences from one call to the next.
        let mut paths: Vec<&String> = self.files.keys().collect();
        paths.sort();
        for path in paths {
            if out.len() >= MAX_REFERENCES {
                break;
            }
            let entry = &self.files[path];
            let hits: Vec<&Occurrence> = entry
                .idents
                .iter()
                .filter(|i| i.name == wanted)
                .take(MAX_REFERENCES - out.len())
                .collect();
            if hits.is_empty() {
                continue;
            }
            let text = std::fs::read_to_string(self.root.join(path)).unwrap_or_default();
            let lines: Vec<&str> = text.lines().collect();
            for hit in hits {
                out.push(Location {
                    path: path.clone(),
                    line: hit.line as usize + 1,
                    column: hit.column as usize,
                    text: lines
                        .get(hit.line as usize)
                        .map(|l| l.trim().to_string())
                        .unwrap_or_default(),
                });
            }
        }
        out.sort_by(|a, b| {
            a.path
                .cmp(&b.path)
                .then(a.line.cmp(&b.line))
                .then(a.column.cmp(&b.column))
        });
        out
    }

    /// The definitions in one file, in source order.
    pub fn symbols_in(&self, path: &str) -> Vec<Symbol> {
        let mut out = self
            .files
            .get(path)
            .map(|f| f.symbols.clone())
            .unwrap_or_default();
        out.sort_by_key(|s| (s.line, s.column));
        out
    }

    /// Definitions whose name contains `query` (case-insensitive), names starting with it
    /// first, then shorter names first.
    pub fn search(&self, query: &str) -> Vec<Symbol> {
        let q = query.to_lowercase();
        if q.is_empty() {
            return Vec::new();
        }
        let mut out: Vec<(u8, Symbol)> = self
            .files
            .values()
            .flat_map(|f| f.symbols.iter())
            .filter_map(|s| {
                // Ranked without lowercasing every name in the repository on every
                // keystroke: names are nearly always ASCII, and only the rest pay a copy.
                let rank = if eq_fold(&s.name, &q) {
                    0
                } else if starts_with_fold(&s.name, &q) {
                    1
                } else if contains_fold(&s.name, &q) {
                    2
                } else {
                    return None;
                };
                Some((rank, s.clone()))
            })
            .collect();
        out.sort_by(|a, b| {
            a.0.cmp(&b.0)
                .then(a.1.name.len().cmp(&b.1.name.len()))
                .then(a.1.name.cmp(&b.1.name))
                .then(a.1.path.cmp(&b.1.path))
                .then(a.1.line.cmp(&b.1.line))
        });
        out.into_iter().map(|(_, s)| s).collect()
    }
}

/// What a worker produces: names as it found them, before they are given numbers.
struct RawEntry {
    len: u64,
    modified: Option<SystemTime>,
    symbols: Vec<Symbol>,
    idents: Vec<Ident>,
    found: Found,
}

/// Parses each file on a pool of threads; unreadable, binary and oversized files are skipped.
fn index_files(root: &Path, paths: &[String]) -> Vec<(String, RawEntry)> {
    if paths.is_empty() {
        return Vec::new();
    }
    let threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(2)
        .min(paths.len())
        .max(1);
    let chunk = paths.len().div_ceil(threads);
    std::thread::scope(|scope| {
        let handles: Vec<_> = paths
            .chunks(chunk)
            .map(|chunk| {
                scope.spawn(move || {
                    let mut out = Vec::with_capacity(chunk.len());
                    for path in chunk {
                        let full = root.join(path);
                        let Ok(meta) = std::fs::metadata(&full) else {
                            continue;
                        };
                        if !meta.is_file() || meta.len() > MAX_FILE_BYTES {
                            continue;
                        }
                        let Ok(bytes) = std::fs::read(&full) else {
                            continue;
                        };
                        if crate::repo::is_binary(&bytes) {
                            continue;
                        }
                        let text = String::from_utf8_lossy(&bytes);
                        let (symbols, idents, found) = index_text(path, &text);
                        out.push((
                            path.clone(),
                            RawEntry {
                                len: meta.len(),
                                modified: meta.modified().ok(),
                                symbols,
                                idents,
                                found,
                            },
                        ));
                    }
                    out
                })
            })
            .collect();
        handles
            .into_iter()
            .flat_map(|h| h.join().unwrap_or_default())
            .collect()
    })
}

fn parse(path: &str, text: &str) -> Option<(Language, tree_sitter::Tree)> {
    let language = language_for_path_and_content(Path::new(path), text)?;
    let ts = to_treesitter(&language)?;
    let mut parser = Parser::new();
    parser.set_language(&ts).ok()?;
    let tree = parser.parse(text, None)?;
    Some((language, tree))
}

/// The tests one file defines, parsed on its own: for a file as it stood at some commit,
/// which the index, holding only the working tree, cannot say.
pub fn tests_of(path: &str, text: &str) -> Vec<TestCase> {
    match parse(path, text) {
        Some((language, tree)) => {
            crate::testsuite::find(path, language, tree.root_node(), text.as_bytes()).tests
        }
        None => Vec::new(),
    }
}

/// What one file defines, parsed on its own, in source order.
pub fn symbols_of(path: &str, text: &str) -> Vec<Symbol> {
    index_text(path, text).0
}

/// Definitions, identifier occurrences and tests of one file.
fn index_text(path: &str, text: &str) -> (Vec<Symbol>, Vec<Ident>, Found) {
    match parse(path, text) {
        Some((language, tree)) => {
            let mut walk = FileWalk::new(path, text, language);
            walk.run(tree.root_node());
            let (symbols, idents) = walk.finish();
            let found = crate::testsuite::find(path, language, tree.root_node(), text.as_bytes());
            (symbols, idents, found)
        }
        None => {
            let (symbols, idents) = regex_index(path, text);
            (symbols, idents, Found::default())
        }
    }
}

const DEFINITION_SUFFIXES: &[&str] = &[
    "_definition",
    "_declaration",
    "_declarator",
    "_item",
    "_spec",
    "_specifier",
    "_signature",
];

/// Iterative pre-order walk; `containers` is the stack of enclosing definitions with the
/// byte where each ends.
/// One walk over one file's syntax tree, collecting what it defines and every name it
/// mentions. The state is here rather than in a parameter list because most of it is
/// scratch that no caller has any use for.
struct FileWalk<'a> {
    bytes: &'a [u8],
    lines: Vec<&'a str>,
    path: &'a str,
    language: Language,
    /// Definitions the walk is inside, each with the byte where it ends. What a name is
    /// defined in is whatever is on top when the name is reached.
    containers: Vec<(String, usize)>,
    symbols: Vec<Symbol>,
    idents: Vec<Ident>,
}

impl<'a> FileWalk<'a> {
    fn new(path: &'a str, text: &'a str, language: Language) -> Self {
        Self {
            bytes: text.as_bytes(),
            lines: text.lines().collect(),
            path,
            language,
            containers: Vec::new(),
            symbols: Vec::new(),
            idents: Vec::new(),
        }
    }

    fn finish(self) -> (Vec<Symbol>, Vec<Ident>) {
        (self.symbols, self.idents)
    }

    fn run(&mut self, root: Node) {
        let mut cursor = root.walk();
        let mut stack: Vec<Node> = vec![root];
        while let Some(node) = stack.pop() {
            self.leave_containers_ending_before(node.start_byte());
            let kind = node.kind();
            if node.child_count() == 0 {
                self.note_identifier(node, kind);
                continue;
            }
            self.note_definition(node, kind);
            // Push children in reverse so they pop in source order.
            let children: Vec<Node> = node.children(&mut cursor).collect();
            for child in children.into_iter().rev() {
                stack.push(child);
            }
        }
    }

    /// The walk is in source order, so a definition whose end is behind us is one we have
    /// left.
    fn leave_containers_ending_before(&mut self, byte: usize) {
        while self.containers.last().is_some_and(|(_, end)| byte >= *end) {
            self.containers.pop();
        }
    }

    /// A leaf whose kind mentions "identifier" is a name being used: the grammars differ on
    /// what they call it, and all of them agree on that much.
    fn note_identifier(&mut self, node: Node, kind: &str) {
        if !node.is_named() || !kind.contains("identifier") {
            return;
        }
        if let Ok(name) = node.utf8_text(self.bytes) {
            self.idents.push(Ident {
                name: name.to_string(),
                line: node.start_position().row as u32,
                column: node.start_position().column as u32,
            });
        }
    }

    fn note_definition(&mut self, node: Node, kind: &str) {
        let Some((name, label, name_node)) = definition_of(node, kind, self.language, self.bytes)
        else {
            return;
        };
        let line = name_node.start_position().row;
        self.symbols.push(Symbol {
            name: name.clone(),
            kind: label,
            path: self.path.to_string(),
            line: line + 1,
            column: name_node.start_position().column,
            end_line: node.end_position().row + 1,
            container: self.containers.last().map(|(n, _)| n.clone()),
            text: self
                .lines
                .get(line)
                .map(|l| l.trim().to_string())
                .unwrap_or_default(),
        });
        self.containers.push((name, node.end_byte()));
    }
}

/// `(name, kind label, the name node)` when `node` defines something.
fn definition_of<'a>(
    node: Node<'a>,
    kind: &str,
    language: Language,
    bytes: &[u8],
) -> Option<(String, String, Node<'a>)> {
    if language == Language::HTML {
        // `<div id="x">`: the id is what people navigate to in a page.
        if kind == "attribute" {
            let mut cursor = node.walk();
            let mut children = node.children(&mut cursor);
            let attr_name = children.next()?;
            if attr_name.utf8_text(bytes).ok()? != "id" {
                return None;
            }
            let value = node
                .children(&mut node.walk())
                .find(|c| c.kind() == "quoted_attribute_value")?;
            let inner = value
                .children(&mut value.walk())
                .find(|c| c.kind() == "attribute_value")?;
            let name = inner.utf8_text(bytes).ok()?.to_string();
            return Some((name, "id".into(), inner));
        }
        return None;
    }
    if kind.contains("parameter") || kind.contains("argument") || kind == "let_declaration" {
        return None;
    }
    let suffix = DEFINITION_SUFFIXES.iter().find(|s| kind.ends_with(*s))?;
    let name_node = node.child_by_field_name("name").or_else(|| {
        (kind == "impl_item")
            .then(|| node.child_by_field_name("type"))
            .flatten()
    })?;
    let name = name_node.utf8_text(bytes).ok()?.trim().to_string();
    if name.is_empty() || name.contains('\n') || name.len() > 120 {
        return None;
    }
    let base = &kind[..kind.len() - suffix.len()];
    let label = match base {
        "function" | "func" => "function",
        "method" => "method",
        "impl" => "impl",
        "class" | "abstract_class" => "class",
        "struct" => "struct",
        "enum" => "enum",
        "trait" | "interface" => base,
        "type" | "type_alias" => "type",
        "const" | "static" | "variable" | "lexical" => base,
        "mod" | "module" | "namespace" => "module",
        "macro" => "macro",
        "field" | "property" => "field",
        "enum_variant" | "variant" => "variant",
        "union" => "union",
        other => other,
    };
    Some((name, label.to_string(), name_node))
}

/// Makefiles and anything else without a grammar: targets, variables and words.
fn regex_index(path: &str, text: &str) -> (Vec<Symbol>, Vec<Ident>) {
    use regex::Regex;
    use std::sync::OnceLock;
    static TARGET: OnceLock<Regex> = OnceLock::new();
    static VARIABLE: OnceLock<Regex> = OnceLock::new();
    static WORD: OnceLock<Regex> = OnceLock::new();
    let target =
        TARGET.get_or_init(|| Regex::new(r"^([A-Za-z0-9_./%-]+)\s*:(?:[^=]|$)").expect("regex"));
    let variable = VARIABLE
        .get_or_init(|| Regex::new(r"^([A-Za-z_][A-Za-z0-9_]*)\s*[:?+!]?=").expect("regex"));
    let word = WORD.get_or_init(|| Regex::new(r"[A-Za-z_][A-Za-z0-9_]*").expect("regex"));
    let is_make = {
        let name = path.rsplit('/').next().unwrap_or(path);
        name.eq_ignore_ascii_case("makefile")
            || name.eq_ignore_ascii_case("gnumakefile")
            || name.ends_with(".mk")
    };
    let mut symbols = Vec::new();
    let mut idents = Vec::new();
    for (row, line) in text.lines().enumerate() {
        if is_make {
            if let Some(c) = target.captures(line) {
                let m = c.get(1).expect("group");
                symbols.push(Symbol {
                    name: m.as_str().to_string(),
                    kind: "target".into(),
                    path: path.to_string(),
                    line: row + 1,
                    column: m.start(),
                    end_line: row + 1,
                    container: None,
                    text: line.trim().to_string(),
                });
            } else if let Some(c) = variable.captures(line) {
                let m = c.get(1).expect("group");
                symbols.push(Symbol {
                    name: m.as_str().to_string(),
                    kind: "variable".into(),
                    path: path.to_string(),
                    line: row + 1,
                    column: m.start(),
                    end_line: row + 1,
                    container: None,
                    text: line.trim().to_string(),
                });
            }
        }
        for m in word.find_iter(line) {
            idents.push(Ident {
                name: m.as_str().to_string(),
                line: row as u32,
                column: m.start() as u32,
            });
        }
    }
    (symbols, idents)
}

/// The identifier under (row, byte column) in `text`, or the word there when the file has
/// no grammar or the node is not an identifier.
pub fn identifier_at(path: &str, text: &str, row: usize, column: usize) -> Option<String> {
    if let Some((_, tree)) = parse(path, text) {
        let point = Point::new(row, column);
        let mut node = tree
            .root_node()
            .named_descendant_for_point_range(point, point)?;
        // A click just past the end of an identifier lands on the parent; try one step back.
        if !node.kind().contains("identifier") && column > 0 {
            let back = Point::new(row, column - 1);
            if let Some(n) = tree
                .root_node()
                .named_descendant_for_point_range(back, back)
            {
                node = n;
            }
        }
        if node.kind().contains("identifier") && node.child_count() == 0 {
            return node.utf8_text(text.as_bytes()).ok().map(str::to_string);
        }
    }
    word_at(text.lines().nth(row)?, column)
}

/// The `[A-Za-z0-9_]+` word covering byte `column` of `line`.
pub fn word_at(line: &str, column: usize) -> Option<String> {
    let is_word = |c: char| c.is_alphanumeric() || c == '_';
    // The column arrives from a click or an API call: it may land anywhere, including
    // inside a character, and slicing there would panic.
    let mut column = column.min(line.len());
    while column > 0 && !line.is_char_boundary(column) {
        column -= 1;
    }
    let mut start = column;
    while start > 0 {
        let prev = line[..start].chars().next_back()?;
        if !is_word(prev) {
            break;
        }
        start -= prev.len_utf8();
    }
    let mut end = column;
    for c in line[column..].chars() {
        if !is_word(c) {
            break;
        }
        end += c.len_utf8();
    }
    if end > start {
        let word = &line[start..end];
        if word
            .chars()
            .next()
            .is_some_and(|c| c.is_alphabetic() || c == '_')
        {
            return Some(word.to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The ranking behind "what does this codebase lean on": a file whose names are used
    /// everywhere outranks one nobody calls, and a name everyone defines counts for little.
    #[test]
    fn reach_finds_what_the_rest_of_the_tree_leans_on() {
        let dir = tempfile::tempdir().unwrap();
        // `core` defines what the two others call.
        std::fs::write(
            dir.path().join("core.rs"),
            "pub fn parse_review() {}\npub fn write_review() {}\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("one.rs"),
            "fn run() { parse_review(); write_review(); }\n",
        )
        .unwrap();
        std::fs::write(dir.path().join("two.rs"), "fn run() { parse_review(); }\n").unwrap();
        // A file nobody calls, defining a name everybody else also defines.
        std::fs::write(dir.path().join("lonely.rs"), "fn run() {}\n").unwrap();
        let files: Vec<String> = ["core.rs", "one.rs", "two.rs", "lonely.rs"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let index = Index::build(dir.path(), &files);

        let reach = index.reach();
        assert_eq!(reach[0].path, "core.rs", "{reach:?}");
        assert!(
            reach[0].names.contains(&"parse_review".to_string()),
            "{reach:?}"
        );
        // `run` is defined in three files, so it is worth a third to each and cannot put a
        // file that nothing calls above the one everything calls.
        let lonely = reach.iter().find(|r| r.path == "lonely.rs");
        assert!(lonely.is_none_or(|l| l.score < reach[0].score), "{reach:?}");
    }

    /// Only what test code names counts as tested. `app.rs` has a test module, which makes it
    /// tested, and calls `store.rs` from its ordinary code, which must not make `store.rs` look
    /// tested; its test names `parse_review`, which does make `review.rs` tested.
    #[test]
    fn untested_is_what_no_test_code_names() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("review.rs"), "pub fn parse_review() {}\n").unwrap();
        std::fs::write(dir.path().join("store.rs"), "pub fn save_to_disk() {}\n").unwrap();
        std::fs::write(
            dir.path().join("app.rs"),
            "fn run_app() { save_to_disk(); parse_review(); }\n\
             #[cfg(test)]\nmod tests {\n    #[test]\n    fn it_parses() { parse_review(); }\n}\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("main.rs"),
            "fn main() { run_app(); save_to_disk(); }\n",
        )
        .unwrap();
        // Leaned on by everything, and not a language anything here tests.
        std::fs::write(dir.path().join("Makefile"), "SAVE_TARGET = x\n").unwrap();
        std::fs::write(dir.path().join("deploy.rs"), "fn go() { SAVE_TARGET; }\n").unwrap();
        // A module for tests only, in a file of its own: test code throughout, so what it
        // names is tested and it is not itself a gap.
        std::fs::write(
            dir.path().join("lib.rs"),
            "#[cfg(test)]\nmod fakes;\nfn go() { write_journal(); fake_journal(); }\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("fakes.rs"),
            "pub fn fake_journal() { write_journal(); }\n",
        )
        .unwrap();
        std::fs::write(dir.path().join("journal.rs"), "pub fn write_journal() {}\n").unwrap();
        let files: Vec<String> = [
            "lib.rs",
            "fakes.rs",
            "journal.rs",
            "review.rs",
            "store.rs",
            "app.rs",
            "main.rs",
            "Makefile",
            "deploy.rs",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let index = Index::build(dir.path(), &files);

        let tests = index.tests();
        assert_eq!(tests.len(), 1, "{tests:?}");
        assert_eq!(tests[0].label, "it parses");
        let untested: Vec<String> = index.untested().into_iter().map(|r| r.path).collect();
        assert!(untested.contains(&"store.rs".to_string()), "{untested:?}");
        assert!(
            !untested.contains(&"app.rs".to_string()),
            "a file with tests of its own is tested: {untested:?}"
        );
        assert!(!untested.contains(&"review.rs".to_string()), "{untested:?}");
        assert!(!untested.contains(&"Makefile".to_string()), "{untested:?}");
        assert!(!untested.contains(&"fakes.rs".to_string()), "{untested:?}");
        assert!(
            !untested.contains(&"journal.rs".to_string()),
            "{untested:?}"
        );
    }

    #[test]
    fn rust_definitions_and_identifiers() {
        let text = "pub struct Foo { x: i32 }\nimpl Foo {\n    pub fn new() -> Self { Foo { x: 1 } }\n}\nfn main() { let f = Foo::new(); }\n";
        let (symbols, idents, _) = index_text("a.rs", text);
        let names: Vec<(&str, &str)> = symbols
            .iter()
            .map(|s| (s.kind.as_str(), s.name.as_str()))
            .collect();
        assert_eq!(
            names,
            vec![
                ("struct", "Foo"),
                ("field", "x"),
                ("impl", "Foo"),
                ("function", "new"),
                ("function", "main")
            ]
        );
        let new = symbols.iter().find(|s| s.name == "new").unwrap();
        assert_eq!(new.container.as_deref(), Some("Foo"));
        assert_eq!(new.line, 3);
        assert!(idents.iter().filter(|i| i.name == "Foo").count() >= 4);
        assert!(idents.iter().any(|i| i.name == "new" && i.line == 4));
    }

    #[test]
    fn python_typescript_html_makefile() {
        let (py, _, _) = index_text(
            "a.py",
            "class A:\n    def m(self):\n        pass\n\ndef f(x):\n    return x\n",
        );
        let names: Vec<&str> = py.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["A", "m", "f"]);
        assert_eq!(py[1].container.as_deref(), Some("A"));

        let (ts, _, _) = index_text(
            "a.ts",
            "export interface I { a: number }\nexport class C implements I {\n  a = 1;\n  go(): void {}\n}\nconst k = () => 1;\nfunction g() {}\n",
        );
        let names: Vec<(&str, &str)> = ts
            .iter()
            .map(|s| (s.kind.as_str(), s.name.as_str()))
            .collect();
        assert!(names.contains(&("interface", "I")));
        assert!(names.contains(&("class", "C")));
        assert!(names.contains(&("method", "go")));
        assert!(names.contains(&("function", "g")));
        assert!(names.contains(&("variable", "k")));

        let (html, _, _) = index_text("a.html", "<div id=\"main\"><span id=\"x\"></span></div>\n");
        let names: Vec<&str> = html.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["main", "x"]);

        let (mk, idents, _) = index_text(
            "Makefile",
            "CC = gcc\nall: build\n\techo $(CC)\nbuild:\n\t$(CC) -o x\n",
        );
        let names: Vec<(&str, &str)> = mk
            .iter()
            .map(|s| (s.kind.as_str(), s.name.as_str()))
            .collect();
        assert_eq!(
            names,
            vec![("variable", "CC"), ("target", "all"), ("target", "build")]
        );
        assert_eq!(idents.iter().filter(|i| i.name == "CC").count(), 3);
    }

    /// The column comes from a click or an API call and may land inside a character.
    #[test]
    fn a_column_inside_a_character_is_not_a_panic() {
        let line = "h\u{e9}llo world";
        assert_eq!(word_at(line, 2).as_deref(), Some("h\u{e9}llo"));
        assert_eq!(word_at(line, 0).as_deref(), Some("h\u{e9}llo"));
        assert_eq!(word_at(line, line.len()).as_deref(), Some("world"));
        assert_eq!(word_at(line, 9999).as_deref(), Some("world"));
        assert_eq!(word_at("", 5), None);
        assert_eq!(
            identifier_at("a.txt", "h\u{e9}llo", 0, 2).as_deref(),
            Some("h\u{e9}llo")
        );
    }

    /// The fast paths must agree with lowercasing, which is what they replace.
    #[test]
    fn folding_matches_lowercasing() {
        for (name, query) in [
            ("Foo", "foo"),
            ("foo", "foo"),
            ("BarFoo", "foo"),
            ("f", "foo"),
            ("\u{c4}pfel", "\u{e4}pfel"),
            ("Stra\u{df}e", "stra\u{df}e"),
            ("nothing", "foo"),
            ("Foo", ""),
            // An ASCII name against a query whose byte length is not its character count.
            ("foo", "f\u{f6}o"),
            ("f\u{f6}o", "foo"),
        ] {
            let lower = name.to_lowercase();
            assert_eq!(eq_fold(name, query), lower == query, "eq {name} {query}");
            assert_eq!(
                starts_with_fold(name, query),
                lower.starts_with(query),
                "starts {name} {query}"
            );
            assert_eq!(
                contains_fold(name, query),
                lower.contains(query),
                "contains {name} {query}"
            );
        }
    }

    #[test]
    fn index_queries() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("a.rs"),
            "pub fn alpha() {}\nfn beta() { alpha(); }\n",
        )
        .unwrap();
        std::fs::create_dir(dir.path().join("src")).unwrap();
        std::fs::write(
            dir.path().join("src/b.rs"),
            "use crate::alpha;\nfn gamma() { alpha(); alpha(); }\n",
        )
        .unwrap();
        let paths = vec![
            "a.rs".to_string(),
            "src/b.rs".to_string(),
            "missing.rs".to_string(),
        ];
        let mut index = Index::build(dir.path(), &paths);
        assert_eq!(index.file_count(), 2);
        assert_eq!(index.symbol_count(), 3);
        let defs = index.definitions("alpha");
        assert_eq!(defs.len(), 1);
        assert_eq!((defs[0].path.as_str(), defs[0].line), ("a.rs", 1));
        let refs = index.references("alpha");
        assert_eq!(refs.len(), 5);
        assert_eq!(refs[0].path, "a.rs");
        assert_eq!(refs[2].path, "src/b.rs");
        assert_eq!(refs[2].text, "use crate::alpha;");
        assert_eq!(
            index
                .search("A")
                .iter()
                .map(|s| s.name.as_str())
                .collect::<Vec<_>>(),
            vec!["alpha", "beta", "gamma"]
        );
        assert_eq!(index.search("gam")[0].name, "gamma");

        // An edit is picked up by refresh; a removed file is dropped.
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(dir.path().join("a.rs"), "pub fn alpha() {}\n").unwrap();
        std::fs::remove_file(dir.path().join("src/b.rs")).unwrap();
        index.refresh(&paths);
        assert_eq!(index.file_count(), 1);
        assert_eq!(index.references("alpha").len(), 1);
        index.update_text("x.py", "def alpha():\n    pass\n");
        assert_eq!(index.definitions("alpha").len(), 2);
    }

    #[test]
    fn identifier_under_cursor() {
        let text = "fn main() {\n    let total = compute(items);\n}\n";
        assert_eq!(identifier_at("a.rs", text, 1, 8).as_deref(), Some("total"));
        assert_eq!(
            identifier_at("a.rs", text, 1, 16).as_deref(),
            Some("compute")
        );
        assert_eq!(
            identifier_at("a.rs", text, 1, 23).as_deref(),
            Some("compute")
        );
        assert_eq!(identifier_at("a.rs", text, 1, 24).as_deref(), Some("items"));
        assert_eq!(identifier_at("a.rs", text, 0, 3).as_deref(), Some("main"));
        assert_eq!(
            identifier_at("Makefile", "all: build\n", 0, 6).as_deref(),
            Some("build")
        );
        assert_eq!(word_at("a.b_c(d)", 3).as_deref(), Some("b_c"));
        assert_eq!(word_at("  42", 3), None);
    }
}
