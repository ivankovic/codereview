//! The codediff wrapper, and the paint model both front ends draw from.
//!
//! codediff reports a diff as ranges (row/column spans, columns in bytes) on each side, each
//! tagged with an operation. This module turns that into two things a renderer wants: one
//! operation per line, for gutters and alignment, and per-line byte spans, for painting the
//! changed characters within a line.

use std::path::Path;

use codediff::code::language::language_for_path_and_content;
use codediff::code::{Code, Language};
use codediff::diff::text::{
    RangeMatch, TextDiff, TextOperation, line_operations, plain_text_line_diff,
};
use codediff::diff::{Diff, NodeCache};
use serde::{Deserialize, Serialize};

/// Stack for the thread the diff runs on. Deep trees (generated files, long chained
/// expressions) recurse deeper than a default thread stack allows; codediff's own front ends
/// use the same figure.
const DIFF_STACK_SIZE: usize = 256 * 1024 * 1024;

/// Beyond this many lines on either side, skip the structural diff. Plain-text is instant.
const STRUCTURAL_MAX_LINES: usize = 20_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Op {
    #[default]
    None,
    Insert,
    Delete,
    Update,
    Move,
}

impl From<&TextOperation> for Op {
    fn from(op: &TextOperation) -> Self {
        match op {
            TextOperation::Insert => Op::Insert,
            TextOperation::Delete => Op::Delete,
            TextOperation::Update => Op::Update,
            TextOperation::Move => Op::Move,
            TextOperation::Identical | TextOperation::NotYetSet => Op::None,
        }
    }
}

impl Op {
    pub fn is_change(self) -> bool {
        self != Op::None
    }
}

/// A byte span within one line and what happened to it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Span {
    pub start: usize,
    pub end: usize,
    pub op: Op,
}

/// One side of a diff, ready to draw.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Side {
    pub text: String,
    /// One entry per line of `text`.
    pub line_ops: Vec<Op>,
    /// Per line, the spans that differ. Lines with nothing changed have an empty list.
    pub spans: Vec<Vec<Span>>,
}

impl Side {
    pub fn line_count(&self) -> usize {
        self.line_ops.len()
    }

    pub fn lines(&self) -> Vec<&str> {
        self.text.lines().collect()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileDiff {
    pub before: Side,
    pub after: Side,
    pub language: String,
    /// True when codediff's tree diff ran; false for the plain-text line diff fallback.
    pub structural: bool,
    /// Whether the tree diff left an unusually large unmatched residual, which usually means
    /// the file was rewritten rather than edited.
    pub large_residual: bool,
}

/// Row `i` of a side-by-side view: which line of each side it shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AlignedRow {
    pub before: Option<usize>,
    pub after: Option<usize>,
}

impl FileDiff {
    /// Diffs `before` against `after`, choosing the language from `path` and content.
    pub fn compute(path: &Path, before: &str, after: &str) -> Self {
        let path = path.to_path_buf();
        let before = before.to_string();
        let after = after.to_string();
        std::thread::Builder::new()
            .stack_size(DIFF_STACK_SIZE)
            .spawn(move || compute_inner(&path, &before, &after))
            .expect("spawn diff thread")
            .join()
            .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
    }

    /// Pairs lines for a two-column view. Unchanged lines with the same text anchor the two
    /// sides; between anchors, changed lines pair up in order and whatever is left over sits
    /// alone on its side.
    pub fn aligned_rows(&self) -> Vec<AlignedRow> {
        let before: Vec<&str> = self.before.text.lines().collect();
        let after: Vec<&str> = self.after.text.lines().collect();
        let anchors = anchors(&before, &self.before.line_ops, &after, &self.after.line_ops);
        let mut rows = Vec::with_capacity(before.len().max(after.len()));
        let (mut i, mut j) = (0, 0);
        for (ai, aj) in anchors
            .into_iter()
            .chain(std::iter::once((before.len(), after.len())))
        {
            // Pair up the changed lines between the previous anchor and this one.
            while i < ai && j < aj {
                rows.push(AlignedRow {
                    before: Some(i),
                    after: Some(j),
                });
                i += 1;
                j += 1;
            }
            while i < ai {
                rows.push(AlignedRow {
                    before: Some(i),
                    after: None,
                });
                i += 1;
            }
            while j < aj {
                rows.push(AlignedRow {
                    before: None,
                    after: Some(j),
                });
                j += 1;
            }
            if ai < before.len() && aj < after.len() {
                rows.push(AlignedRow {
                    before: Some(ai),
                    after: Some(aj),
                });
                i = ai + 1;
                j = aj + 1;
            }
        }
        rows
    }

    /// Counts of changed lines on each side, for a status line.
    pub fn counts(&self) -> (usize, usize) {
        let count = |ops: &[Op]| ops.iter().filter(|o| o.is_change()).count();
        (count(&self.before.line_ops), count(&self.after.line_ops))
    }
}

/// Pairs of (before row, after row) that hold the same unchanged text, strictly increasing on
/// both sides. Lines unique on both sides are matched first (they are the reliable anchors);
/// the gaps between them are then filled with a plain in-order walk over equal lines.
fn anchors(before: &[&str], bops: &[Op], after: &[&str], aops: &[Op]) -> Vec<(usize, usize)> {
    use std::collections::HashMap;
    let unchanged = |ops: &[Op], i: usize| ops.get(i).is_none_or(|o| !o.is_change());
    let mut before_index: HashMap<&str, Vec<usize>> = HashMap::new();
    for (i, line) in before.iter().enumerate() {
        if unchanged(bops, i) && !line.trim().is_empty() {
            before_index.entry(line).or_default().push(i);
        }
    }
    let mut after_index: HashMap<&str, Vec<usize>> = HashMap::new();
    for (j, line) in after.iter().enumerate() {
        if unchanged(aops, j) && !line.trim().is_empty() {
            after_index.entry(line).or_default().push(j);
        }
    }
    // Unique on both sides, then the longest chain that is increasing on both.
    let mut unique: Vec<(usize, usize)> = before_index
        .iter()
        .filter(|(_, is)| is.len() == 1)
        .filter_map(|(text, is)| {
            let js = after_index.get(text)?;
            (js.len() == 1).then_some((is[0], js[0]))
        })
        .collect();
    unique.sort_unstable();
    let chain = longest_increasing(&unique);
    // Fill the gaps between chained anchors with an in-order walk over equal lines.
    let mut out = Vec::new();
    let (mut i, mut j) = (0, 0);
    for (ai, aj) in chain
        .into_iter()
        .chain(std::iter::once((before.len(), after.len())))
    {
        while i < ai && j < aj {
            if unchanged(bops, i) && unchanged(aops, j) && before[i] == after[j] {
                out.push((i, j));
                i += 1;
                j += 1;
            } else if !unchanged(bops, i) || (unchanged(aops, j) && !unchanged(bops, i)) {
                i += 1;
            } else if !unchanged(aops, j) {
                j += 1;
            } else {
                // Both unchanged but different text: advance the side that is further behind.
                if ai - i >= aj - j {
                    i += 1;
                } else {
                    j += 1;
                }
            }
        }
        if ai < before.len() && aj < after.len() {
            out.push((ai, aj));
        }
        i = ai + 1;
        j = aj + 1;
    }
    out
}

/// The longest subsequence of `pairs` (sorted by the first element) whose second elements
/// strictly increase.
fn longest_increasing(pairs: &[(usize, usize)]) -> Vec<(usize, usize)> {
    if pairs.is_empty() {
        return Vec::new();
    }
    let mut tails: Vec<usize> = Vec::new(); // index into pairs of the tail of each length
    let mut prev: Vec<Option<usize>> = vec![None; pairs.len()];
    for (k, &(_, j)) in pairs.iter().enumerate() {
        let pos = tails.partition_point(|&t| pairs[t].1 < j);
        if pos > 0 {
            prev[k] = Some(tails[pos - 1]);
        }
        if pos == tails.len() {
            tails.push(k);
        } else {
            tails[pos] = k;
        }
    }
    let mut out = Vec::new();
    let mut cur = tails.last().copied();
    while let Some(k) = cur {
        out.push(pairs[k]);
        cur = prev[k];
    }
    out.reverse();
    out
}

fn compute_inner(path: &Path, before: &str, after: &str) -> FileDiff {
    let too_big = before.lines().count() > STRUCTURAL_MAX_LINES
        || after.lines().count() > STRUCTURAL_MAX_LINES;
    let language =
        language_for_path_and_content(path, if after.is_empty() { before } else { after })
            .unwrap_or(Language::Unknown);
    if language == Language::Unknown || too_big {
        return plain(before, after, language, false);
    }
    let before_code = Code::from_string(before, &language);
    let after_code = Code::from_string(after, &language);
    if before_code.ast.is_none() || after_code.ast.is_none() {
        return plain(before, after, language, false);
    }
    let pending = Diff::pending(&before_code, &after_code);
    let large_residual = pending.large_residual();
    let diff = pending.finish();
    let Some(ast) = diff.ast.as_ref() else {
        return plain(before, after, language, large_residual);
    };
    let cache = NodeCache::build(&before_code, &after_code);
    let text = TextDiff::from(&before_code, &after_code, ast, &cache);
    FileDiff {
        before: side(before, &text.all(0)),
        after: side(after, &text.all(1)),
        language: language.to_string(),
        structural: true,
        large_residual,
    }
}

fn plain(before: &str, after: &str, language: Language, large_residual: bool) -> FileDiff {
    let (b, a) = plain_text_line_diff(before, after);
    FileDiff {
        before: side(before, &b),
        after: side(after, &a),
        language: language.to_string(),
        structural: false,
        large_residual,
    }
}

fn side(text: &str, ranges: &[RangeMatch]) -> Side {
    let lines: Vec<&str> = text.lines().collect();
    let line_ops: Vec<Op> = line_operations(ranges, lines.len())
        .into_iter()
        .map(|op| Op::from(&op))
        .collect();
    // Which ranges touch which row, worked out once. Asking every range about every line
    // is lines times ranges, which on a large structural diff is most of the work.
    let mut by_row: Vec<Vec<&RangeMatch>> = vec![Vec::new(); lines.len()];
    for range in ranges.iter().filter(|r| Op::from(&r.operation).is_change()) {
        let last = range.source.end_row.min(lines.len().saturating_sub(1));
        for row in by_row
            .iter_mut()
            .take(last + 1)
            .skip(range.source.start_row)
        {
            row.push(range);
        }
    }
    let spans = lines
        .iter()
        .zip(&by_row)
        .enumerate()
        .map(|(row, (line, here))| spans_for_line(here, row, line.len()))
        .collect();
    Side {
        text: text.to_string(),
        line_ops,
        spans,
    }
}

/// The changed byte spans of row `row`, clipped to the line, sorted, non-overlapping.
/// `ranges` are only those that touch this row.
fn spans_for_line(ranges: &[&RangeMatch], row: usize, len: usize) -> Vec<Span> {
    let mut spans: Vec<Span> = ranges
        .iter()
        .map(|r| {
            let start = if r.source.start_row == row {
                r.source.start_column.min(len)
            } else {
                0
            };
            let end = if r.source.end_row == row {
                r.source.end_column.min(len)
            } else {
                len
            };
            Span {
                start,
                end,
                op: Op::from(&r.operation),
            }
        })
        .filter(|s| s.end > s.start || len == 0)
        .collect();
    spans.sort_by_key(|s| (s.start, s.end));
    // Later ranges take precedence where they overlap (codediff lists the finer ones last).
    let mut merged: Vec<Span> = Vec::new();
    for s in spans {
        if let Some(last) = merged.last_mut() {
            if s.start < last.end {
                if s.start > last.start {
                    let tail = Span {
                        start: s.end,
                        end: last.end,
                        op: last.op,
                    };
                    last.end = s.start;
                    merged.push(s);
                    if tail.end > tail.start {
                        merged.push(tail);
                    }
                } else {
                    last.op = s.op;
                    last.end = last.end.max(s.end);
                }
                continue;
            }
        }
        merged.push(s);
    }
    merged
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rust_insertion_is_structural() {
        let before = "fn main() {\n    let a = 1;\n}\n";
        let after = "fn main() {\n    let a = 1;\n    let b = 2;\n}\n";
        let d = FileDiff::compute(Path::new("x.rs"), before, after);
        assert!(d.structural);
        assert_eq!(d.after.line_ops[2], Op::Insert);
        assert_eq!(d.before.line_ops[1], Op::None);
        let rows = d.aligned_rows();
        assert_eq!(rows.len(), 4);
        assert_eq!(
            rows[2],
            AlignedRow {
                before: None,
                after: Some(2)
            }
        );
    }

    #[test]
    fn alignment_anchors_on_unchanged_lines() {
        let before = "fn main() {\n    let a = 1;\n}\n";
        let after = "fn total() {}\n\nfn main() {\n    let a = 2;\n}\n";
        let d = FileDiff::compute(Path::new("x.rs"), before, after);
        let rows = d.aligned_rows();
        // The two `fn main() {` lines and the two `}` lines sit on the same rows.
        assert!(rows.contains(&AlignedRow {
            before: Some(0),
            after: Some(2)
        }));
        assert!(rows.contains(&AlignedRow {
            before: Some(1),
            after: Some(3)
        }));
        assert!(rows.contains(&AlignedRow {
            before: Some(2),
            after: Some(4)
        }));
        assert_eq!(
            rows[0],
            AlignedRow {
                before: None,
                after: Some(0)
            }
        );
        assert_eq!(rows.len(), 5);
    }

    #[test]
    fn longest_chain() {
        let pairs = [(0, 5), (1, 1), (2, 2), (3, 9), (4, 3)];
        assert_eq!(longest_increasing(&pairs), vec![(1, 1), (2, 2), (4, 3)]);
        assert!(longest_increasing(&[]).is_empty());
    }

    #[test]
    fn makefile_falls_back_to_lines() {
        let d = FileDiff::compute(Path::new("Makefile"), "a:\n\techo a\n", "a:\n\techo b\n");
        assert!(!d.structural);
        assert!(d.after.line_ops[1].is_change());
        assert!(!d.after.line_ops[0].is_change());
    }

    #[test]
    fn spans_clip_and_merge() {
        use codediff::diff::text_range::TextRange;
        let r = |sr, sc, er, ec, op| RangeMatch {
            source: TextRange::new(sr, sc, er, ec),
            destination: TextRange::new(0, 0, 0, 0),
            operation: op,
        };
        let ranges = [
            r(0, 2, 0, 8, TextOperation::Update),
            r(0, 4, 0, 6, TextOperation::Insert),
            r(0, 0, 2, 0, TextOperation::Identical),
        ];
        // `side` hands on only the ranges that change something and touch the row.
        let touching: Vec<&RangeMatch> = ranges
            .iter()
            .filter(|r| Op::from(&r.operation).is_change())
            .collect();
        let spans = spans_for_line(&touching, 0, 10);
        assert_eq!(
            spans,
            vec![
                Span {
                    start: 2,
                    end: 4,
                    op: Op::Update
                },
                Span {
                    start: 4,
                    end: 6,
                    op: Op::Insert
                },
                Span {
                    start: 6,
                    end: 8,
                    op: Op::Update
                },
            ]
        );
    }
}
