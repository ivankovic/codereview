//! The file tree on the left of the explorer: every path in the repository, folded by
//! directory, with a cursor and a filter. A directory of hundreds of files is a wall, and
//! the next directory is somewhere below it, so past the tenth file the rest fold behind a
//! "… N more" row — except the files the tree has something to say about, which stay in
//! view — the same as the tree in the browser.

use std::collections::{BTreeSet, HashMap};

/// How many files a directory shows before the rest fold behind "… N more".
pub const FOLD: usize = 10;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Node {
    /// Repository-relative path; directories have no trailing slash.
    pub path: String,
    pub name: String,
    pub depth: usize,
    pub is_dir: bool,
}

/// One row of the tree: a node, or the row a directory's file list ends on when it folds.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Row {
    Node(usize),
    /// "… N more" while `hidden` files are folded; "… fewer" when the directory is shown
    /// whole and could fold again. `dir` is `""` for the repository root.
    Fold {
        dir: String,
        depth: usize,
        hidden: usize,
    },
}

/// What a row shows, for drawing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Entry<'a> {
    Node(&'a Node),
    /// `hidden` files of the directory fold behind this row.
    More {
        depth: usize,
        hidden: usize,
    },
    /// The directory is shown whole; this row folds it back.
    Fewer {
        depth: usize,
    },
}

#[derive(Debug, Default)]
pub struct Tree {
    /// Every node in display order, directories before files within a directory.
    nodes: Vec<Node>,
    /// Directories the user folded. Public so that a test can check a refresh keeps them.
    pub(crate) collapsed: BTreeSet<String>,
    /// Directories the user asked to see whole, past the fold.
    shown: BTreeSet<String>,
    /// Files that never fold: the changed and the commented on. Whoever knows tells the
    /// tree with `set_keep`.
    keep: BTreeSet<String>,
    /// The rows currently shown.
    visible: Vec<Row>,
    pub cursor: usize,
    pub scroll: usize,
    pub filter: String,
}

/// The directory a path is in, `""` at the root.
fn parent_of(path: &str) -> &str {
    path.rsplit_once('/').map(|(p, _)| p).unwrap_or("")
}

/// The files of one directory as they are listed: how many are on screen, how many folded,
/// and whether the directory is shown whole by request.
struct Run {
    dir: String,
    depth: usize,
    listed: usize,
    hidden: usize,
    whole: bool,
}

impl Run {
    /// Ends the run, with its fold row when there is something to fold or unfold.
    fn end(run: &mut Option<Run>, visible: &mut Vec<Row>) {
        if let Some(r) = run.take() {
            if r.hidden > 0 || r.whole {
                visible.push(Row::Fold {
                    dir: r.dir,
                    depth: r.depth,
                    hidden: r.hidden,
                });
            }
        }
    }
}

impl Tree {
    /// Takes a new file list, keeping what the user set up: the folds, the filter and, when
    /// it is still there, the cursor's file.
    pub fn replace_files(&mut self, files: &[String]) {
        let here = self.current().map(|n| n.path.clone());
        let fresh = Tree::new(files);
        self.nodes = fresh.nodes;
        self.rebuild();
        if let Some(path) = here {
            self.select(&path);
        }
    }

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

    /// Recomputes the visible rows after a fold or filter change. A filter is a request to
    /// see everything it matches, so nothing folds past the tenth file while one is typed.
    pub fn rebuild(&mut self) {
        let filter = self.filter.to_lowercase();
        self.visible.clear();
        // Files per directory: a fold that hides a single file is not worth its row.
        let mut counts: HashMap<&str, usize> = HashMap::new();
        if filter.is_empty() {
            for n in &self.nodes {
                if !n.is_dir {
                    *counts.entry(parent_of(&n.path)).or_insert(0) += 1;
                }
            }
        }
        let mut run: Option<Run> = None;
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
            } else if node.is_dir {
                // Directories sort before files, so a directory ends the files before it.
                Run::end(&mut run, &mut self.visible);
                if self.collapsed.contains(&node.path) {
                    self.visible.push(Row::Node(i));
                    hidden_prefix = Some(format!("{}/", node.path));
                    continue;
                }
            } else {
                let dir = parent_of(&node.path);
                if run.as_ref().is_none_or(|r| r.dir != dir) {
                    Run::end(&mut run, &mut self.visible);
                    let long = counts.get(dir).copied().unwrap_or(0) > FOLD + 1;
                    run = Some(Run {
                        dir: dir.to_string(),
                        depth: node.depth,
                        listed: 0,
                        hidden: 0,
                        whole: long && self.shown.contains(dir),
                    });
                    // Only a long directory folds; a short one runs to its end unmarked.
                    if !long {
                        run = None;
                    }
                }
                if let Some(r) = run.as_mut() {
                    if !r.whole && r.listed >= FOLD && !self.keep.contains(&node.path) {
                        r.hidden += 1;
                        continue;
                    }
                    r.listed += 1;
                }
            }
            self.visible.push(Row::Node(i));
        }
        Run::end(&mut run, &mut self.visible);
        if self.cursor >= self.visible.len() {
            self.cursor = self.visible.len().saturating_sub(1);
        }
    }

    /// Names the files that never fold behind "… more": the changed and the commented on.
    /// The cursor stays on its file, unfolding whatever the change hides it behind.
    pub fn set_keep(&mut self, keep: BTreeSet<String>) {
        if keep == self.keep {
            return;
        }
        self.keep = keep;
        let here = self.current().map(|n| n.path.clone());
        self.rebuild();
        if let Some(path) = here {
            self.select(&path);
        }
    }

    pub fn len(&self) -> usize {
        self.visible.len()
    }

    pub fn is_empty(&self) -> bool {
        self.visible.is_empty()
    }

    /// The node on row `i`; none for a fold row, which is no file and no directory.
    pub fn row(&self, i: usize) -> Option<&Node> {
        match self.visible.get(i)? {
            Row::Node(n) => Some(&self.nodes[*n]),
            Row::Fold { .. } => None,
        }
    }

    /// Row `i` as it is drawn.
    pub fn entry(&self, i: usize) -> Option<Entry<'_>> {
        Some(match self.visible.get(i)? {
            Row::Node(n) => Entry::Node(&self.nodes[*n]),
            Row::Fold { depth, hidden, .. } if *hidden > 0 => Entry::More {
                depth: *depth,
                hidden: *hidden,
            },
            Row::Fold { depth, .. } => Entry::Fewer { depth: *depth },
        })
    }

    /// Where `path` is among the rows, if it is shown.
    fn position(&self, path: &str) -> Option<usize> {
        self.visible
            .iter()
            .position(|r| matches!(r, Row::Node(n) if self.nodes[*n].path == path))
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

    /// Enter on a directory folds or unfolds it, and on "… N more" or "… fewer" shows the
    /// rest of the directory or folds it back; returns the file path when on a file.
    pub fn activate(&mut self) -> Option<String> {
        if let Some(Row::Fold { dir, hidden, .. }) = self.visible.get(self.cursor).cloned() {
            if hidden > 0 {
                self.shown.insert(dir);
            } else {
                self.shown.remove(&dir);
            }
            self.rebuild();
            return None;
        }
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
        // A fold row belongs to its directory, like a file in it.
        let node = match self.visible.get(self.cursor).cloned() {
            Some(Row::Fold { dir, .. }) => {
                if dir.is_empty() {
                    return;
                }
                self.collapsed.insert(dir.clone());
                self.rebuild();
                if let Some(i) = self.position(&dir) {
                    self.cursor = i;
                }
                return;
            }
            Some(Row::Node(n)) => self.nodes[n].clone(),
            None => return,
        };
        if node.is_dir && !self.collapsed.contains(&node.path) {
            self.collapsed.insert(node.path);
        } else if let Some(parent) = node.path.rsplit_once('/').map(|(p, _)| p.to_string()) {
            // Jump to the parent directory and fold it.
            self.collapsed.insert(parent.clone());
            self.rebuild();
            if let Some(i) = self.position(&parent) {
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

    /// Puts the cursor on `path`, unfolding what it takes to see it, including the "… more"
    /// of its directory. A filter in force stays in force: the caller chose it, and a jump
    /// is not a reason to drop it. When the filter hides `path`, the cursor stays where it
    /// was.
    pub fn select(&mut self, path: &str) {
        let mut prefix = String::new();
        for part in path.split('/') {
            if !prefix.is_empty() {
                self.collapsed.remove(&prefix);
                prefix.push('/');
            }
            prefix.push_str(part);
        }
        self.rebuild();
        if self.position(path).is_none() && self.filter.is_empty() {
            self.shown.insert(parent_of(path).to_string());
            self.rebuild();
        }
        if let Some(i) = self.position(path) {
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

    fn many(n: usize) -> Vec<String> {
        (0..n)
            .map(|i| format!("src/f{i:02}.rs"))
            .chain(["Cargo.toml".to_string()])
            .collect()
    }

    #[test]
    fn a_long_directory_folds_past_ten() {
        let mut t = Tree::new(&many(30));
        // src, ten files, the fold row, Cargo.toml.
        assert_eq!(t.len(), 13);
        assert_eq!(
            t.entry(11),
            Some(Entry::More {
                depth: 1,
                hidden: 20
            })
        );
        assert!(t.row(11).is_none(), "a fold row is no node");
        t.cursor = 11;
        assert!(t.activate().is_none());
        assert_eq!(t.len(), 33, "shown whole, with its fewer row");
        assert_eq!(t.entry(31), Some(Entry::Fewer { depth: 1 }));
        t.cursor = 31;
        t.activate();
        assert_eq!(t.len(), 13);
        // Left on the fold row folds the directory, as on one of its files.
        t.cursor = 11;
        t.collapse();
        assert_eq!(t.current().unwrap().path, "src");
        assert!(t.is_collapsed("src"));
    }

    #[test]
    fn kept_and_selected_files_do_not_fold() {
        let mut t = Tree::new(&many(30));
        t.set_keep(["src/f25.rs".to_string()].into());
        assert_eq!(t.len(), 14);
        assert_eq!(t.row(11).unwrap().path, "src/f25.rs");
        t.select("src/f28.rs");
        assert_eq!(t.current().unwrap().path, "src/f28.rs");
        assert_eq!(
            t.len(),
            33,
            "selecting a folded file shows its directory whole"
        );
    }

    #[test]
    fn a_short_directory_and_a_filter_fold_nothing() {
        // Eleven files: a fold row hiding one would be worse than the file.
        assert_eq!(Tree::new(&many(11)).len(), 13);
        let mut t = Tree::new(&many(30));
        t.filter = "f2".into();
        t.rebuild();
        assert_eq!(t.len(), 11, "src and f20 to f29");
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
