//! The tests a repository defines, found by reading it rather than by running it.
//!
//! Someone opening a repository they had an agent build did not write its tests either, but
//! the tests are the one place where what the system is supposed to do is written down and
//! checked by a machine. Agents name them as sentences often enough —
//! `a_password_opens_a_session_and_guessing_is_slowed` — that the list, grouped by file, reads
//! as a specification. So this finds them, in the same parse the symbol index already makes,
//! and says which lines of each file are test code, so that the index can tell which of the
//! project's names the tests actually mention.
//!
//! Each language by the convention its own runner uses, and nothing cleverer:
//!
//! - **Rust**: functions under `#[test]` or a runtime's `#[tokio::test]`, with `#[ignore]`
//!   marking one skipped. A `#[cfg(test)]` module is test code — its file too, when it is
//!   declared as `#[cfg(test)] mod fakes;` — and so is every file under a `tests` directory.
//! - **Python**: `test*` functions, at module level or in a `Test*` class, in the files pytest
//!   collects — `test_*.py`, `*_test.py` — and anything under a `tests` directory. A `skip`
//!   decorator marks one skipped; `skipif` does not, since it may well run.
//! - **Go**: `TestXxx` functions in `_test.go` files.
//! - **JavaScript and TypeScript**: `it(...)` and `test(...)` calls with a string for a name,
//!   in `*.test.*` and `*.spec.*` files and under `__tests__` or a `test`/`tests` directory,
//!   named with the `describe` blocks around them. `.skip`, `.todo` and an `x` in front mark
//!   one skipped.

use std::sync::OnceLock;

use omnidiff::code::Language;
use regex::Regex;
use serde::{Deserialize, Serialize};
use tree_sitter::Node;

use crate::symbols::Reach;

/// How deep a JavaScript walk goes before giving up on a file. Test files nest callbacks, but
/// not hundreds deep; a file that does is not one worth overflowing the stack for.
const MAX_DEPTH: usize = 200;

/// One test, where it is and what it says it checks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TestCase {
    /// As written: the function's name, or the string a `test(...)` call was given.
    pub name: String,
    /// The same as a phrase: underscores to spaces, the `test` prefix gone, and the class or
    /// `describe` blocks it is in leading, joined with ` › `.
    pub label: String,
    pub path: String,
    /// 1-based.
    pub line: usize,
    /// Marked to be skipped: `#[ignore]`, `@pytest.mark.skip`, `it.skip`.
    pub ignored: bool,
}

/// What one file contributes: its tests, and which of its lines are test code, as 0-based
/// inclusive row ranges. A whole test file is one range to `u32::MAX`.
#[derive(Debug, Clone, Default)]
pub struct Found {
    pub tests: Vec<TestCase>,
    pub code: Vec<(u32, u32)>,
    /// Rust modules declared here under `#[cfg(test)]` without a body, by name: their files
    /// are test code throughout, which only the index, knowing every path, can resolve.
    pub test_modules: Vec<String>,
}

impl Found {
    /// Whether 0-based `row` is test code.
    pub fn covers(code: &[(u32, u32)], row: u32) -> bool {
        code.iter().any(|&(from, to)| from <= row && row <= to)
    }

    /// Whether the whole file is test code.
    pub fn whole(code: &[(u32, u32)]) -> bool {
        code.iter().any(|&(from, to)| from == 0 && to == u32::MAX)
    }
}

/// The tests in one parsed file.
pub fn find(path: &str, language: Language, root: Node, bytes: &[u8]) -> Found {
    let mut found = Found::default();
    let whole = is_test_file(path, language);
    if whole {
        found.code.push((0, u32::MAX));
    }
    let mut walk = Walk {
        path,
        bytes,
        found: &mut found,
    };
    match language {
        Language::Rust => walk.rust(root),
        Language::Python if whole => walk.python(root, None),
        Language::Go if whole => walk.go(root),
        Language::JavaScript | Language::TypeScript | Language::TSX if whole => {
            walk.javascript(root, &mut Vec::new(), false, 0)
        }
        _ => {}
    }
    found
}

/// Whether `path` is in a language whose tests this module finds, by its extension: the
/// question "does anything test this" only means something for those.
pub fn testable(path: &str) -> bool {
    let ext = path.rsplit_once('.').map(|(_, e)| e).unwrap_or("");
    matches!(
        ext,
        "rs" | "py" | "go" | "js" | "jsx" | "mjs" | "cjs" | "ts" | "tsx" | "mts" | "cts"
    )
}

/// The files a Rust module `name` declared in `path` can live in: beside a `lib.rs`,
/// `main.rs` or `mod.rs`, or in the directory named after any other file.
pub fn module_files(path: &str, name: &str) -> Vec<String> {
    let (dir, file) = match path.rsplit_once('/') {
        Some((d, f)) => (format!("{d}/"), f),
        None => (String::new(), path),
    };
    let stem = file.strip_suffix(".rs").unwrap_or(file);
    let base = if matches!(stem, "lib" | "main" | "mod") {
        dir
    } else {
        format!("{dir}{stem}/")
    };
    vec![format!("{base}{name}.rs"), format!("{base}{name}/mod.rs")]
}

/// Whether the runner for `language` treats the whole of `path` as tests.
fn is_test_file(path: &str, language: Language) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    let under =
        |dir: &str| path.starts_with(&format!("{dir}/")) || path.contains(&format!("/{dir}/"));
    match language {
        Language::Rust => under("tests"),
        Language::Python => {
            (name.starts_with("test_") || name.ends_with("_test.py") || name == "conftest.py")
                || under("tests")
        }
        Language::Go => name.ends_with("_test.go"),
        Language::JavaScript | Language::TypeScript | Language::TSX => {
            name.contains(".test.")
                || name.contains(".spec.")
                || under("__tests__")
                || under("tests")
                || under("test")
        }
        _ => false,
    }
}

struct Walk<'a, 'f> {
    path: &'a str,
    bytes: &'a [u8],
    found: &'f mut Found,
}

impl Walk<'_, '_> {
    fn text(&self, node: Node) -> &str {
        node.utf8_text(self.bytes).unwrap_or("")
    }

    fn push(&mut self, name: &str, label: String, at: Node, ignored: bool) {
        self.found.tests.push(TestCase {
            name: name.to_string(),
            label,
            path: self.path.to_string(),
            line: at.start_position().row + 1,
            ignored,
        });
    }

    fn mark(&mut self, node: Node) {
        self.found.code.push((
            node.start_position().row as u32,
            node.end_position().row as u32,
        ));
    }

    /// A list of items — a file, or a module's body — read in order, since an attribute is a
    /// sibling of the item it belongs to rather than a part of it.
    fn rust(&mut self, list: Node) {
        static TEST: OnceLock<Regex> = OnceLock::new();
        static CFG_TEST: OnceLock<Regex> = OnceLock::new();
        static IGNORE: OnceLock<Regex> = OnceLock::new();
        let test = TEST.get_or_init(|| {
            Regex::new(r"^#\[\s*(?:[A-Za-z_][A-Za-z0-9_]*\s*::\s*)*test\s*[\](]").expect("regex")
        });
        let cfg_test = CFG_TEST
            .get_or_init(|| Regex::new(r"^#\[\s*cfg\s*\(\s*test\s*\)\s*\]").expect("regex"));
        let ignore = IGNORE.get_or_init(|| Regex::new(r"^#\[\s*ignore\b").expect("regex"));

        let mut attributes: Vec<Node> = Vec::new();
        let mut cursor = list.walk();
        for item in list.named_children(&mut cursor) {
            match item.kind() {
                "attribute_item" => attributes.push(item),
                "line_comment" | "block_comment" => {}
                kind => {
                    let has = |re: &Regex| attributes.iter().any(|a| re.is_match(self.text(*a)));
                    if kind == "function_item" && has(test) {
                        if let Some(name) = item.child_by_field_name("name") {
                            let name = self.text(name).to_string();
                            let ignored = has(ignore);
                            self.push(&name, phrase(&name), item, ignored);
                            self.mark(item);
                        }
                    } else if kind == "mod_item" {
                        let body = item.child_by_field_name("body");
                        if has(cfg_test) {
                            self.mark(item);
                            if body.is_none() {
                                if let Some(name) = item.child_by_field_name("name") {
                                    let name = self.text(name).to_string();
                                    self.found.test_modules.push(name);
                                }
                            }
                        }
                        if let Some(body) = body {
                            self.rust(body);
                        }
                    }
                    attributes.clear();
                }
            }
        }
    }

    /// A module or a class body. `class` is the `Test*` class being read, if any.
    fn python(&mut self, list: Node, class: Option<&str>) {
        static SKIP: OnceLock<Regex> = OnceLock::new();
        let skip = SKIP.get_or_init(|| Regex::new(r"\bskip\b").expect("regex"));
        let mut cursor = list.walk();
        for item in list.named_children(&mut cursor) {
            let (definition, ignored) = match item.kind() {
                "decorated_definition" => {
                    let mut c = item.walk();
                    let ignored = item
                        .named_children(&mut c)
                        .filter(|d| d.kind() == "decorator")
                        .any(|d| skip.is_match(self.text(d)));
                    match item.child_by_field_name("definition") {
                        Some(d) => (d, ignored),
                        None => continue,
                    }
                }
                _ => (item, false),
            };
            let Some(name) = definition.child_by_field_name("name") else {
                continue;
            };
            let name = self.text(name).to_string();
            match definition.kind() {
                "function_definition" if name.starts_with("test") => {
                    let label = match class {
                        Some(c) => format!("{} › {}", phrase(c), phrase(&name)),
                        None => phrase(&name),
                    };
                    self.push(&name, label, item, ignored);
                }
                "class_definition" if class.is_none() && name.starts_with("Test") => {
                    if let Some(body) = definition.child_by_field_name("body") {
                        self.python(body, Some(&name));
                    }
                }
                _ => {}
            }
        }
    }

    fn go(&mut self, file: Node) {
        let mut cursor = file.walk();
        for item in file.named_children(&mut cursor) {
            if item.kind() != "function_declaration" {
                continue;
            }
            let Some(name) = item.child_by_field_name("name") else {
                continue;
            };
            let name = self.text(name).to_string();
            // `Testify` is not a test; `Test`, `TestParse` and `Test_parse` are.
            let rest = name.strip_prefix("Test");
            if rest.is_some_and(|r| !r.starts_with(|c: char| c.is_lowercase())) {
                self.push(&name, phrase(&name), item, false);
            }
        }
    }

    /// Every call in the file, with the `describe` names it is inside of and whether one of
    /// them is skipped.
    fn javascript(&mut self, node: Node, describes: &mut Vec<String>, skipped: bool, depth: usize) {
        if depth > MAX_DEPTH {
            return;
        }
        if node.kind() == "call_expression" {
            if let Some((what, skip, name)) = self.js_call(node) {
                let ignored = skipped || skip;
                if what == "describe" {
                    describes.push(name);
                    if let Some(args) = node.child_by_field_name("arguments") {
                        self.javascript(args, describes, ignored, depth + 1);
                    }
                    describes.pop();
                    return;
                }
                let label = describes
                    .iter()
                    .map(String::as_str)
                    .chain([name.as_str()])
                    .collect::<Vec<_>>()
                    .join(" › ");
                self.push(&name, label, node, ignored);
                return;
            }
        }
        let mut cursor = node.walk();
        let children: Vec<Node> = node.named_children(&mut cursor).collect();
        for child in children {
            self.javascript(child, describes, skipped, depth + 1);
        }
    }

    /// `("test" | "describe", skipped, name)` when `call` is `it`, `test` or `describe`, in
    /// any of their spellings, given a string for a name.
    fn js_call(&self, call: Node) -> Option<(&'static str, bool, String)> {
        let function = call.child_by_field_name("function")?;
        let (base, modifier) = match function.kind() {
            "identifier" => (self.text(function), ""),
            "member_expression" => {
                let object = function.child_by_field_name("object")?;
                let property = function.child_by_field_name("property")?;
                if object.kind() != "identifier" {
                    return None;
                }
                (self.text(object), self.text(property))
            }
            _ => return None,
        };
        let (what, x) = match base {
            "it" | "test" => ("test", false),
            "xit" | "xtest" => ("test", true),
            "describe" => ("describe", false),
            "xdescribe" => ("describe", true),
            _ => return None,
        };
        let skip = match modifier {
            "" | "only" | "concurrent" => x,
            "skip" | "todo" => true,
            _ => return None,
        };
        let args = call.child_by_field_name("arguments")?;
        let first = args.named_child(0)?;
        if !matches!(first.kind(), "string" | "template_string") {
            return None;
        }
        let raw = self.text(first);
        let name = raw
            .get(1..raw.len().saturating_sub(1))
            .unwrap_or(raw)
            .to_string();
        Some((what, skip, name))
    }
}

/// A function name as a phrase: `a_config_file_round_trips` reads "a config file round trips",
/// `test_parses_empty` reads "parses empty", `TestParseConfig` reads "parse config".
pub fn phrase(name: &str) -> String {
    let rest = name
        .strip_prefix("test_")
        .or_else(|| name.strip_prefix("Test_"))
        .or_else(|| name.strip_prefix("Test").filter(|r| !r.is_empty()))
        .or_else(|| name.strip_prefix("test").filter(|r| !r.is_empty()))
        .unwrap_or(name);
    // Camel case into words, for Go and for Python's `Test*` classes; a run of capitals is
    // an abbreviation and stays one word.
    let mut words: Vec<String> = Vec::new();
    let mut word = String::new();
    let chars: Vec<char> = rest.chars().collect();
    for (i, &c) in chars.iter().enumerate() {
        if c == '_' || c == ' ' {
            if !word.is_empty() {
                words.push(std::mem::take(&mut word));
            }
            continue;
        }
        let starts_word = c.is_uppercase()
            && !word.is_empty()
            && (word
                .chars()
                .last()
                .is_some_and(|p| p.is_lowercase() || p.is_ascii_digit())
                || chars.get(i + 1).is_some_and(|n| n.is_lowercase()));
        if starts_word {
            words.push(std::mem::take(&mut word));
        }
        word.push(c);
    }
    if !word.is_empty() {
        words.push(word);
    }
    words
        .into_iter()
        .map(|w| {
            // `Parse` becomes `parse`; `HTTP` stays as it is.
            if w.chars().skip(1).all(|c| !c.is_uppercase()) {
                w.to_lowercase()
            } else {
                w
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// The tests of a repository as the overview shows them: grouped by the file they are in, and
/// beside them the files the rest of the tree leans on that no test mentions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Inventory {
    pub files: Vec<TestFile>,
    pub total: usize,
    pub ignored: usize,
    /// Leaned on, highest first, and never named by test code: where a change would break
    /// the most with nothing to say so.
    pub untested: Vec<Reach>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestFile {
    pub path: String,
    pub tests: Vec<TestCase>,
}

impl Inventory {
    /// What `OVERVIEW.md` says, in the shape the page draws: tests without line numbers, since
    /// the file keeps none, and the leaned-on files it names, at most `most` of them.
    pub fn from_facts(facts: &crate::facts::Facts, most: usize) -> Self {
        let files: Vec<TestFile> = facts
            .tests_by_file
            .iter()
            .map(|(path, tests)| TestFile {
                path: path.clone(),
                tests: tests
                    .iter()
                    .map(|(label, ignored)| TestCase {
                        name: label.clone(),
                        label: label.clone(),
                        path: path.clone(),
                        line: 0,
                        ignored: *ignored,
                    })
                    .collect(),
            })
            .collect();
        let total = files.iter().map(|f| f.tests.len()).sum();
        let ignored = files
            .iter()
            .flat_map(|f| &f.tests)
            .filter(|t| t.ignored)
            .count();
        Self {
            files,
            total,
            ignored,
            untested: facts
                .untested
                .iter()
                .take(most)
                .map(|(path, names)| Reach {
                    path: path.clone(),
                    score: 0.0,
                    names: names.clone(),
                })
                .collect(),
        }
    }

    /// Groups `tests`, which come sorted by path and line.
    pub fn of(tests: Vec<TestCase>, untested: Vec<Reach>) -> Self {
        let total = tests.len();
        let ignored = tests.iter().filter(|t| t.ignored).count();
        let mut files: Vec<TestFile> = Vec::new();
        for test in tests {
            match files.last_mut() {
                Some(file) if file.path == test.path => file.tests.push(test),
                _ => files.push(TestFile {
                    path: test.path.clone(),
                    tests: vec![test],
                }),
            }
        }
        Self {
            files,
            total,
            ignored,
            untested,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use omnidiff::code::language::to_treesitter;

    fn found(path: &str, language: Language, text: &str) -> Found {
        let mut parser = tree_sitter::Parser::new();
        parser
            .set_language(&to_treesitter(&language).unwrap())
            .unwrap();
        let tree = parser.parse(text, None).unwrap();
        find(path, language, tree.root_node(), text.as_bytes())
    }

    fn names(f: &Found) -> Vec<(&str, bool)> {
        f.tests
            .iter()
            .map(|t| (t.label.as_str(), t.ignored))
            .collect()
    }

    #[test]
    fn rust_tests_are_the_functions_under_a_test_attribute() {
        let text = "\
fn helper() {}

#[cfg(test)]
mod tests {
    use super::*;

    /// Says what it checks.
    #[test]
    fn a_config_file_round_trips() {}

    #[tokio::test(flavor = \"multi_thread\")]
    async fn the_server_answers() {}

    #[test]
    #[ignore = \"needs podman\"]
    fn behind_nginx() {}

    fn not_a_test() {}
}
";
        let f = found("src/config.rs", Language::Rust, text);
        assert_eq!(
            names(&f),
            vec![
                ("a config file round trips", false),
                ("the server answers", false),
                ("behind nginx", true)
            ]
        );
        assert_eq!(
            f.tests[0].line, 9,
            "the function's own line, past its attributes"
        );
        // The module is test code, the helper above it is not.
        assert!(!Found::covers(&f.code, 0));
        assert!(Found::covers(&f.code, 4));
        assert!(!Found::whole(&f.code));
        // A file under `tests/` is test code throughout.
        let f = found("tests/browser.rs", Language::Rust, "fn setup() {}\n");
        assert!(f.tests.is_empty());
        assert!(Found::whole(&f.code));
    }

    #[test]
    fn python_tests_are_what_pytest_collects() {
        let text = "\
import pytest

def test_parses_empty():
    pass

@pytest.mark.skip(reason=\"slow\")
def test_the_whole_tree():
    pass

@pytest.mark.skipif(True, reason=\"sometimes\")
def test_sometimes():
    pass

class TestConfig:
    def test_loads_defaults(self):
        pass

def helper():
    pass
";
        let f = found("tests/test_config.py", Language::Python, text);
        assert_eq!(
            names(&f),
            vec![
                ("parses empty", false),
                ("the whole tree", true),
                ("sometimes", false),
                ("config › loads defaults", false)
            ]
        );
        // The same functions outside a file pytest collects are not tests.
        assert!(
            found("src/config.py", Language::Python, text)
                .tests
                .is_empty()
        );
    }

    #[test]
    fn go_tests_are_test_functions_in_test_files() {
        let text = "package x\n\nfunc TestParseConfig(t *testing.T) {}\nfunc Testify() {}\nfunc helper() {}\n";
        let f = found("config_test.go", Language::Go, text);
        assert_eq!(names(&f), vec![("parse config", false)]);
    }

    #[test]
    fn javascript_tests_carry_their_describe_blocks() {
        let text = "\
describe('the review file', () => {
  it('keeps a comment on its line', () => {});
  it.skip('survives a rename', () => {});
  describe.skip('when empty', () => {
    test('says so', () => {});
  });
});
test(`stands alone`, () => {});
xit('is off', () => {});
";
        let f = found("src/review.test.ts", Language::TypeScript, text);
        assert_eq!(
            names(&f),
            vec![
                ("the review file › keeps a comment on its line", false),
                ("the review file › survives a rename", true),
                ("the review file › when empty › says so", true),
                ("stands alone", false),
                ("is off", true),
            ]
        );
    }

    /// `#[cfg(test)] mod fakes;` makes a whole other file test code, wherever Rust looks for it.
    #[test]
    fn a_test_module_declared_elsewhere_is_found_by_name() {
        let f = found(
            "src/lib.rs",
            Language::Rust,
            "#[cfg(test)]\nmod fakes;\nmod session;\n",
        );
        assert_eq!(f.test_modules, vec!["fakes"]);
        assert_eq!(
            module_files("src/lib.rs", "fakes"),
            vec!["src/fakes.rs", "src/fakes/mod.rs"]
        );
        assert_eq!(
            module_files("src/web.rs", "tests"),
            vec!["src/web/tests.rs", "src/web/tests/mod.rs"]
        );
        assert_eq!(module_files("main.rs", "cli"), vec!["cli.rs", "cli/mod.rs"]);
    }

    #[test]
    fn a_name_reads_as_a_phrase() {
        assert_eq!(
            phrase("a_config_file_round_trips"),
            "a config file round trips"
        );
        assert_eq!(phrase("test_parses_empty"), "parses empty");
        assert_eq!(phrase("TestParseConfig"), "parse config");
        assert_eq!(phrase("TestHTTPServer_starts"), "HTTP server starts");
        assert_eq!(phrase("test"), "test");
    }
}
