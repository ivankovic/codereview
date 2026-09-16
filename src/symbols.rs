//! Symbol navigation without a language server: definitions and identifier occurrences,
//! extracted with the tree-sitter grammars codediff already carries, for every file in the
//! repository. Good enough to answer "where is this defined" and "where is this used" in a
//! tree that changes too fast for anyone to keep a language server happy, and it works the
//! same for every language that has a grammar.
//!
//! Definitions come from one generic rule: a node whose kind ends in `_definition`,
//! `_declaration`, `_declarator`, `_item`, `_spec`, `_specifier` or `_signature` and has a
//! `name` child field
//! (or a `type` field, for a Rust `impl`). Occurrences are every leaf node whose kind contains
//! `identifier`. Files without a grammar (Makefiles) get a small regex fallback.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use codediff::code::Language;
use codediff::code::language::{language_for_path_and_content, to_treesitter};
use serde::{Deserialize, Serialize};
use tree_sitter::{Node, Parser, Point};

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
const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;

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
    /// ones no longer listed.
    pub fn refresh(&mut self, paths: &[String]) {
        let listed: std::collections::HashSet<&str> = paths.iter().map(String::as_str).collect();
        self.files.retain(|p, _| listed.contains(p.as_str()));
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
                }
                (None, None) => {}
            }
        }
        for (path, entry) in index_files(&self.root, &stale) {
            let entry = self.take_in(entry);
            self.files.insert(path, entry);
        }
    }

    /// Replaces one file's entry from text held in memory rather than read from disk. The
    /// entry carries no modification time, so the next `refresh` sees it as stale and indexes
    /// the file on disk again: this is for looking something up now, not for overriding what
    /// the repository says.
    pub fn update_text(&mut self, path: &str, text: &str) {
        let (symbols, idents) = index_text(path, text);
        let entry = self.take_in(RawEntry {
            len: text.len() as u64,
            modified: None,
            symbols,
            idents,
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
        }
    }

    pub fn file_count(&self) -> usize {
        self.files.len()
    }

    pub fn symbol_count(&self) -> usize {
        self.files.values().map(|f| f.symbols.len()).sum()
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
                        let (symbols, idents) = index_text(path, &text);
                        out.push((
                            path.clone(),
                            RawEntry {
                                len: meta.len(),
                                modified: meta.modified().ok(),
                                symbols,
                                idents,
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

/// Definitions and identifier occurrences of one file.
fn index_text(path: &str, text: &str) -> (Vec<Symbol>, Vec<Ident>) {
    match parse(path, text) {
        Some((language, tree)) => {
            let mut walk = FileWalk::new(path, text, language);
            walk.run(tree.root_node());
            walk.finish()
        }
        None => regex_index(path, text),
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

    #[test]
    fn rust_definitions_and_identifiers() {
        let text = "pub struct Foo { x: i32 }\nimpl Foo {\n    pub fn new() -> Self { Foo { x: 1 } }\n}\nfn main() { let f = Foo::new(); }\n";
        let (symbols, idents) = index_text("a.rs", text);
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
        let (py, _) = index_text(
            "a.py",
            "class A:\n    def m(self):\n        pass\n\ndef f(x):\n    return x\n",
        );
        let names: Vec<&str> = py.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["A", "m", "f"]);
        assert_eq!(py[1].container.as_deref(), Some("A"));

        let (ts, _) = index_text(
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

        let (html, _) = index_text("a.html", "<div id=\"main\"><span id=\"x\"></span></div>\n");
        let names: Vec<&str> = html.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["main", "x"]);

        let (mk, idents) = index_text(
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
