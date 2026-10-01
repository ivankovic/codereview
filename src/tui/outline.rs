//! The outline: what the open file defines, nested the way the file nests it, in the place
//! the file tree otherwise has. It follows the file's cursor, so it says where you are as
//! well as what is there, and Enter goes to what it names.

use crate::symbols::Symbol;

/// One row: a definition and how deep inside others it is.
#[derive(Debug, Clone)]
pub struct Item {
    pub symbol: Symbol,
    pub depth: usize,
}

#[derive(Debug, Clone, Default)]
pub struct Outline {
    /// The file this is the outline of.
    pub path: String,
    pub items: Vec<Item>,
    pub cursor: usize,
    pub scroll: usize,
}

impl Outline {
    /// The outline of `path` from what it defines. A definition is inside another when it
    /// starts and ends within the other's lines, which is nesting as the file writes it —
    /// a method in its `impl`, a test in its `mod tests` — with no language's rules needed.
    /// Fields are left out: they sit in the file right under their type, and in a panel a
    /// quarter of the screen wide a struct's dozen of them push the functions off it.
    pub fn new(path: &str, mut symbols: Vec<Symbol>) -> Self {
        symbols.retain(|s| s.kind != "field");
        symbols.sort_by(|a, b| a.line.cmp(&b.line).then(b.end_line.cmp(&a.end_line)));
        // The last line of each definition the walk is inside.
        let mut open: Vec<usize> = Vec::new();
        let items = symbols
            .into_iter()
            .map(|symbol| {
                while open.last().is_some_and(|&end| end < symbol.line) {
                    open.pop();
                }
                let depth = open.len();
                open.push(symbol.end_line);
                Item { symbol, depth }
            })
            .collect();
        Self {
            path: path.to_string(),
            items,
            cursor: 0,
            scroll: 0,
        }
    }

    pub fn current(&self) -> Option<&Item> {
        self.items.get(self.cursor)
    }

    pub fn move_by(&mut self, delta: isize) {
        if self.items.is_empty() {
            return;
        }
        let max = self.items.len() as isize - 1;
        self.cursor = (self.cursor as isize + delta).clamp(0, max) as usize;
    }

    pub fn go_top(&mut self) {
        self.cursor = 0;
    }

    pub fn go_bottom(&mut self) {
        self.cursor = self.items.len().saturating_sub(1);
    }

    /// Puts the cursor on where the file's 0-based `line` is: the innermost definition around
    /// it, or, between definitions, the last one that started before it.
    pub fn follow(&mut self, line: usize) {
        let line = line + 1;
        let around = self
            .items
            .iter()
            .rposition(|i| i.symbol.line <= line && line <= i.symbol.end_line);
        let before = || self.items.iter().rposition(|i| i.symbol.line <= line);
        if let Some(i) = around.or_else(before) {
            self.cursor = i;
        }
    }

    pub fn ensure_visible(&mut self, height: usize) {
        if height == 0 {
            return;
        }
        if self.cursor < self.scroll {
            self.scroll = self.cursor;
        } else if self.cursor >= self.scroll + height {
            self.scroll = self.cursor + 1 - height;
        }
    }
}

/// A definition's kind, short enough to sit before its name.
pub fn kind_label(kind: &str) -> &str {
    match kind {
        "function" => "fn",
        "module" => "mod",
        "variable" | "lexical" => "let",
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn symbol(name: &str, line: usize, end_line: usize) -> Symbol {
        Symbol {
            name: name.into(),
            kind: "function".into(),
            path: "a.rs".into(),
            line,
            column: 0,
            end_line,
            container: None,
            text: String::new(),
        }
    }

    /// Nesting comes from the lines alone, and the cursor follows the file to the innermost
    /// definition around a line.
    #[test]
    fn an_outline_nests_and_follows() {
        let mut o = Outline::new(
            "a.rs",
            vec![
                symbol("helper", 20, 22),
                symbol("Foo", 1, 10),
                symbol("new", 2, 4),
                symbol("inner", 1, 1),
                Symbol {
                    kind: "field".into(),
                    ..symbol("x", 3, 3)
                },
                symbol("tests", 12, 18),
                symbol("it_works", 13, 15),
            ],
        );
        let rows: Vec<(&str, usize)> = o
            .items
            .iter()
            .map(|i| (i.symbol.name.as_str(), i.depth))
            .collect();
        assert_eq!(
            rows,
            vec![
                ("Foo", 0),
                ("inner", 1),
                ("new", 1),
                ("tests", 0),
                ("it_works", 1),
                ("helper", 0)
            ]
        );
        o.follow(2); // line 3, inside `new`
        assert_eq!(o.current().unwrap().symbol.name, "new");
        o.follow(10); // line 11, between `Foo` and `tests`
        assert_eq!(o.current().unwrap().symbol.name, "new");
        o.follow(13);
        assert_eq!(o.current().unwrap().symbol.name, "it_works");
        o.move_by(10);
        assert_eq!(o.current().unwrap().symbol.name, "helper");
    }
}
