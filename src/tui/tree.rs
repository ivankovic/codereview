//! The file tree on the left of the explorer: every path in the repository, folded by
//! directory, with a cursor and a filter.

use std::collections::BTreeSet;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Node {
    /// Repository-relative path; directories have no trailing slash.
    pub path: String,
    pub name: String,
    pub depth: usize,
    pub is_dir: bool,
}

#[derive(Debug, Default)]
pub struct Tree {
    /// Every node in display order, directories before files within a directory.
    nodes: Vec<Node>,
    collapsed: BTreeSet<String>,
    /// Indices into `nodes` currently shown.
    visible: Vec<usize>,
    pub cursor: usize,
    pub scroll: usize,
    pub filter: String,
}

impl Tree {
    pub fn new(files: &[String]) -> Self {
        let mut nodes = Vec::new();
        let mut seen_dirs: BTreeSet<String> = BTreeSet::new();
        // Build a sorted list where directories sort before files at each level.
        let mut entries: Vec<(Vec<String>, bool)> = Vec::new();
        for file in files {
            let parts: Vec<String> = file.split('/').map(str::to_string).collect();
            for i in 1..parts.len() {
                let dir = parts[..i].join("/");
                if seen_dirs.insert(dir) {
                    entries.push((parts[..i].to_vec(), true));
                }
            }
            entries.push((parts, false));
        }
        entries.sort_by(|a, b| {
            let (pa, da) = a;
            let (pb, db) = b;
            // Compare component by component; at the first difference, directories first.
            let n = pa.len().min(pb.len());
            for i in 0..n {
                let last_a = i == pa.len() - 1;
                let last_b = i == pb.len() - 1;
                let dir_a = !last_a || *da;
                let dir_b = !last_b || *db;
                if pa[i] == pb[i] && dir_a == dir_b {
                    continue;
                }
                if pa[i] == pb[i] {
                    // Same name, one a dir and one a file: dir first.
                    return dir_b.cmp(&dir_a);
                }
                return match (dir_a, dir_b) {
                    (true, false) => std::cmp::Ordering::Less,
                    (false, true) => std::cmp::Ordering::Greater,
                    _ => pa[i].cmp(&pb[i]),
                };
            }
            pa.len().cmp(&pb.len())
        });
        for (parts, is_dir) in entries {
            nodes.push(Node {
                path: parts.join("/"),
                name: parts.last().cloned().unwrap_or_default(),
                depth: parts.len() - 1,
                is_dir,
            });
        }
        let mut tree = Self {
            nodes,
            ..Default::default()
        };
        tree.rebuild();
        tree
    }

    /// Recomputes the visible rows after a fold or filter change.
    pub fn rebuild(&mut self) {
        let filter = self.filter.to_lowercase();
        self.visible.clear();
        let mut hidden_prefix: Option<String> = None;
        for (i, node) in self.nodes.iter().enumerate() {
            if let Some(prefix) = &hidden_prefix {
                if node.path.starts_with(prefix.as_str()) && node.path.len() > prefix.len() {
                    continue;
                }
                hidden_prefix = None;
            }
            if !filter.is_empty() {
                if node.is_dir {
                    // Show a directory only if something under it matches.
                    let has_match = self.nodes[i + 1..]
                        .iter()
                        .take_while(|n| n.path.starts_with(&format!("{}/", node.path)))
                        .any(|n| !n.is_dir && n.path.to_lowercase().contains(&filter));
                    if !has_match {
                        continue;
                    }
                } else if !node.path.to_lowercase().contains(&filter) {
                    continue;
                }
            } else if node.is_dir && self.collapsed.contains(&node.path) {
                self.visible.push(i);
                hidden_prefix = Some(format!("{}/", node.path));
                continue;
            }
            self.visible.push(i);
        }
        if self.cursor >= self.visible.len() {
            self.cursor = self.visible.len().saturating_sub(1);
        }
    }

    pub fn len(&self) -> usize {
        self.visible.len()
    }

    pub fn is_empty(&self) -> bool {
        self.visible.is_empty()
    }

    pub fn row(&self, i: usize) -> Option<&Node> {
        self.visible.get(i).map(|&n| &self.nodes[n])
    }

    pub fn current(&self) -> Option<&Node> {
        self.row(self.cursor)
    }

    /// The node with this path, whether or not it is currently visible.
    pub fn node(&self, path: &str) -> Option<&Node> {
        self.nodes.iter().find(|n| n.path == path)
    }

    pub fn is_collapsed(&self, path: &str) -> bool {
        self.collapsed.contains(path)
    }

    pub fn move_by(&mut self, delta: isize) {
        if self.visible.is_empty() {
            return;
        }
        let max = self.visible.len() as isize - 1;
        self.cursor = (self.cursor as isize + delta).clamp(0, max) as usize;
    }

    pub fn go_top(&mut self) {
        self.cursor = 0;
    }

    pub fn go_bottom(&mut self) {
        self.cursor = self.visible.len().saturating_sub(1);
    }

    /// Enter on a directory folds or unfolds it; returns the file path when on a file.
    pub fn activate(&mut self) -> Option<String> {
        let node = self.current()?.clone();
        if node.is_dir {
            if !self.collapsed.remove(&node.path) {
                self.collapsed.insert(node.path.clone());
            }
            self.rebuild();
            None
        } else {
            Some(node.path)
        }
    }

    pub fn collapse(&mut self) {
        let Some(node) = self.current().cloned() else {
            return;
        };
        if node.is_dir && !self.collapsed.contains(&node.path) {
            self.collapsed.insert(node.path);
        } else if let Some(parent) = node.path.rsplit_once('/').map(|(p, _)| p.to_string()) {
            // Jump to the parent directory and fold it.
            self.collapsed.insert(parent.clone());
            self.rebuild();
            if let Some(i) = self
                .visible
                .iter()
                .position(|&n| self.nodes[n].path == parent)
            {
                self.cursor = i;
            }
            return;
        }
        self.rebuild();
    }

    pub fn expand(&mut self) {
        let Some(node) = self.current().cloned() else {
            return;
        };
        if node.is_dir && self.collapsed.remove(&node.path) {
            self.rebuild();
        }
    }

    pub fn expand_all(&mut self) {
        self.collapsed.clear();
        self.rebuild();
    }

    pub fn collapse_all(&mut self) {
        self.collapsed = self
            .nodes
            .iter()
            .filter(|n| n.is_dir)
            .map(|n| n.path.clone())
            .collect();
        self.rebuild();
    }

    /// Moves the cursor to `path`, unfolding whatever hides it.
    pub fn select(&mut self, path: &str) {
        let mut prefix = String::new();
        for part in path.split('/') {
            if !prefix.is_empty() {
                self.collapsed.remove(&prefix);
                prefix.push('/');
            }
            prefix.push_str(part);
        }
        self.filter.clear();
        self.rebuild();
        if let Some(i) = self
            .visible
            .iter()
            .position(|&n| self.nodes[n].path == path)
        {
            self.cursor = i;
        }
    }

    /// Keeps the cursor within `height` rows of the scroll offset.
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

#[cfg(test)]
mod tests {
    use super::*;

    fn files() -> Vec<String> {
        [
            "src/main.rs",
            "src/tui/app.rs",
            "Cargo.toml",
            "README.md",
            "src/lib.rs",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect()
    }

    #[test]
    fn order_and_folding() {
        let mut t = Tree::new(&files());
        let paths: Vec<&str> = (0..t.len())
            .map(|i| t.row(i).unwrap().path.as_str())
            .collect();
        assert_eq!(
            paths,
            vec![
                "src",
                "src/tui",
                "src/tui/app.rs",
                "src/lib.rs",
                "src/main.rs",
                "Cargo.toml",
                "README.md"
            ]
        );
        assert!(t.activate().is_none()); // fold src
        assert_eq!(t.len(), 3);
        t.select("src/tui/app.rs");
        assert_eq!(t.current().unwrap().path, "src/tui/app.rs");
        t.collapse();
        assert_eq!(t.current().unwrap().path, "src/tui");
        assert!(t.is_collapsed("src/tui"));
    }

    #[test]
    fn filter_shows_matching_files_and_their_dirs() {
        let mut t = Tree::new(&files());
        t.filter = "app".into();
        t.rebuild();
        let paths: Vec<&str> = (0..t.len())
            .map(|i| t.row(i).unwrap().path.as_str())
            .collect();
        assert_eq!(paths, vec!["src", "src/tui", "src/tui/app.rs"]);
    }
}
