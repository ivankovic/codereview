/*  This file is part of the OmniDiff code diffing tool.
 *
 *  Copyright (C) 2026 Marko Ivankovic
 *
 *  This program is free software: you can redistribute it and/or modify
 *  it under the terms of the GNU Affero General Public License as published
 *  by the Free Software Foundation, either version 3 of the License, or
 *  (at your option) any later version.
 *
 *  This program is distributed in the hope that it will be useful,
 *  but WITHOUT ANY WARRANTY; without even the implied warranty of
 *  MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the
 *  GNU Affero General Public License for more details.
 *
 *  You should have received a copy of the GNU Affero General Public License
 *  along with this program. If not, see <https://www.gnu.org/licenses/>.
 */

//! Renders every fixture's `human_mapping.json` (see `src/bin/human_solver/`) as a static,
//! read-only HTML page with before/after trees and code panels, driven by
//! `assets/mapping_site/viewer.js`. Published to GitHub Pages by `.github/workflows/pages.yml`.
//!
//! This is for humans to review the ground truth itself; it never runs omnidiff's own diff. The
//! one thing a reviewer can record is "I looked at this and had nothing to file": a per-fixture
//! mark that `assets/mapping_site/reviewed.js` keeps in the browser's own storage (the site has
//! no server), tied to the page's [`fixture_revision`] so a later remapping shows up as
//! unreviewed again. The index page and every fixture page can also open a random fixture that
//! has no current mark.

use std::collections::HashMap;
use std::fs;
use std::hash::Hasher;
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Parser;
use tree_sitter::Node;

use omnidiff::code::{Code, Language};
use omnidiff::diff::NodeCache;
use omnidiff::diff::text::{RangeMatch, TextDiff, TextOperation};
use omnidiff::diff::text_range::TextRange;
use omnidiff::test::helper;
#[cfg(test)]
use omnidiff::test::helper::human_mapping::rebuild_caches;
use omnidiff::test::helper::human_mapping::{
    self, Caches, GroupPairing, HumanMapping, HumanOperation, HumanTextVerdict, MarkKind,
    NodeStatus, is_identical_after, is_identical_before, is_moved_after, is_moved_before,
    match_operation_after, match_operation_before, rebuild_caches_for_mapping, status_after,
    status_before, unmarked_node_count,
};

/// `owner/repo` for the "file an issue" and "view source" links.
const REPO: &str = "ivankovic/omnidiff";

#[derive(Parser)]
struct Args {
    /// Directory to write the generated site into. Wiped and recreated on every run.
    #[arg(long, default_value = "site")]
    out: PathBuf,
}

fn main() -> Result<()> {
    let args = Args::parse();

    if args.out.exists() {
        fs::remove_dir_all(&args.out)
            .with_context(|| format!("removing existing {:?}", args.out))?;
    }
    let fixtures_dir = args.out.join("fixtures");
    let assets_dir = args.out.join("assets");
    fs::create_dir_all(&fixtures_dir)?;
    fs::create_dir_all(&assets_dir)?;

    // Embedded at compile time so the generator is one self-contained binary.
    fs::write(
        assets_dir.join("style.css"),
        include_str!("../../assets/mapping_site/style.css"),
    )?;
    fs::write(
        assets_dir.join("viewer.js"),
        include_str!("../../assets/mapping_site/viewer.js"),
    )?;
    fs::write(
        assets_dir.join("index.js"),
        include_str!("../../assets/mapping_site/index.js"),
    )?;
    fs::write(
        assets_dir.join("reviewed.js"),
        include_str!("../../assets/mapping_site/reviewed.js"),
    )?;

    let pairs = helper::handmade_test_code_pairs()?;
    let mut names: Vec<&String> = pairs.keys().collect();
    names.sort();

    // A fixture's origin lives in `sample.csv`, and its `owner-repo` slug needs the clone list to
    // become a URL (see `helper::repository_urls`).
    let provenance = helper::sample_provenance()?;
    let repository_urls = helper::repository_urls()?;

    let mut index_entries: Vec<IndexEntry> = Vec::new();
    let mut skipped = 0usize;
    let mut warnings: Vec<String> = Vec::new();

    for name in names {
        let (before, after) = &pairs[name];
        // A fixture with no grammar (`bazel-not-actually-supported-by-treesitter`) has no trees to
        // draw; skip it with a warning rather than failing the whole site.
        if before.ast.is_none() || after.ast.is_none() {
            warnings.push(format!(
                "{name}: no AST on one side (no grammar for its language), not rendered"
            ));
            skipped += 1;
            continue;
        }
        let mapping = match human_mapping::load(name) {
            Ok(mapping) => mapping,
            // An unsolved fixture has no human_mapping.json; not an error.
            Err(_) => {
                skipped += 1;
                continue;
            }
        };

        let note = helper::read_note(name);
        let upstream = provenance.get(name).and_then(|sample| {
            helper::upstream_commit_url(sample, &repository_urls).map(|commit_url| Upstream {
                commit_url,
                repository: sample.repository.clone(),
                commit: sample.commit.clone(),
                path: sample.path.clone(),
            })
        });

        let page = render_fixture_page(
            name,
            before,
            after,
            &mapping,
            note.as_deref(),
            upstream.as_ref(),
            &mut warnings,
        )?;
        fs::write(fixtures_dir.join(format!("{name}.html")), page.html)
            .with_context(|| format!("writing page for '{name}'"))?;

        let language = before.metadata.language.unwrap_or_default();
        // Line-level rather than node-level: it is the only granularity Unix `diff` can be scored
        // at, so the omnidiff and diff columns mean the same thing.
        let mismatches = human_mapping::line_mismatches_for_mapping(&mapping, before, after)
            .with_context(|| format!("computing line mismatches for '{name}'"))?;
        index_entries.push(IndexEntry {
            name: name.clone(),
            language,
            omnidiff_mismatches: mismatches.omnidiff,
            unix_diff_mismatches: mismatches.unix_diff,
            total_lines: mismatches.total_lines,
            // Names, not a count: only "nobody painted this" has no names, which differs from a
            // painting with nothing to paint (see `HumanMapping::text_mappings`).
            paintings: mapping
                .text_mappings
                .iter()
                .map(|named| named.name.clone())
                .collect(),
            note,
            unmarked_nodes: page.unmarked_nodes,
            revision: page.revision,
        });
    }

    fs::write(
        args.out.join("index.html"),
        render_index_page(&index_entries),
    )?;
    // Every fixture page's "random unreviewed fixture" button needs the whole list; see
    // `render_fixtures_script`.
    fs::write(
        assets_dir.join("fixtures.js"),
        render_fixtures_script(&index_entries),
    )?;

    let painted = index_entries
        .iter()
        .filter(|entry| !entry.paintings.is_empty())
        .count();
    let incomplete = index_entries
        .iter()
        .filter(|entry| entry.unmarked_nodes > 0)
        .count();
    println!(
        "Generated {} fixture page(s) into {:?} ({painted} with a painting, {incomplete} with \
         unmarked nodes, {skipped} skipped: no human_mapping.json, or no grammar)",
        index_entries.len(),
        args.out,
    );
    for warning in &warnings {
        eprintln!("warning: {warning}");
    }
    Ok(())
}

/// Where a fixture's two files were sampled from. The commit is the *after* side's; the before
/// side is the same file in its single parent. See `helper::SampleProvenance`.
struct Upstream {
    /// From `helper::upstream_commit_url`, so nothing here knows how a forge spells a commit path.
    commit_url: String,
    /// `owner-repo`, the slug `sample.csv` records; the link text, since a bare hash names no
    /// project.
    repository: String,
    commit: String,
    path: String,
}

/// One rendered fixture page, plus the one number the index wants that only rendering computes.
struct FixturePage {
    html: String,
    /// Nodes the human mapping says nothing about. Counted here because rendering already built
    /// the `Caches` it needs.
    unmarked_nodes: usize,
    /// See [`fixture_revision`]; baked into the page and repeated in the index's row for it.
    revision: String,
}

/// A fingerprint of everything a fixture's page shows: the mapping (paintings included, they are
/// in the same struct) and both source files. `reviewed.js` stores it with a reader's "I have
/// reviewed this" mark, so a fixture that is remapped, repainted or resampled after being reviewed
/// shows up as needing another look rather than silently keeping its mark.
///
/// Hashes the mapping as serialized, not the file's bytes, so reformatting `human_mapping.json`
/// without changing a decision keeps every mark. MetroHash because it is already a dependency and
/// nothing here needs to resist an adversary; 16 hex digits, one `u64`.
fn fixture_revision(mapping: &HumanMapping, before: &Code, after: &Code) -> Result<String> {
    let mut hasher = metrohash::MetroHash64::new();
    let mapping_json = serde_json::to_string(mapping).context("serializing the mapping")?;
    // Length-prefixed so the three parts cannot slide into one another.
    for part in [mapping_json.as_str(), &before.contents, &after.contents] {
        hasher.write_usize(part.len());
        hasher.write(part.as_bytes());
    }
    Ok(format!("{:016x}", hasher.finish()))
}

fn render_fixture_page(
    name: &str,
    before: &Code,
    after: &Code,
    mapping: &human_mapping::HumanMapping,
    note: Option<&str>,
    upstream: Option<&Upstream>,
    // Problems worth reporting but not failing over, such as an unreadable painting.
    warnings: &mut Vec<String>,
) -> Result<FixturePage> {
    let before_root = before
        .ast
        .as_ref()
        .context("Before code has no AST")?
        .root_node();
    let after_root = after
        .ast
        .as_ref()
        .context("After code has no AST")?
        .root_node();

    let caches = rebuild_caches_for_mapping(mapping, before_root, after_root);
    let groups = resolve_groups(mapping, before_root, after_root);
    let unmarked_nodes = unmarked_node_count(before_root, &caches, status_before)
        + unmarked_node_count(after_root, &caches, status_after);
    let revision = fixture_revision(mapping, before, after)?;

    let before_quiet_sizes =
        fully_quiet_subtree_sizes(before_root, &caches, status_before, is_identical_before);
    let after_quiet_sizes =
        fully_quiet_subtree_sizes(after_root, &caches, status_after, is_identical_after);

    // Node paths are not baked into the markup: `viewer.js` derives the clicked node's path
    // client-side, since a `data-path` on every node inflates the page substantially.
    let before_html = render_node(
        before_root,
        before.contents.as_bytes(),
        'b',
        &caches,
        &groups,
        &before_quiet_sizes,
        true,
    );
    let after_html = render_node(
        after_root,
        after.contents.as_bytes(),
        'a',
        &caches,
        &groups,
        &after_quiet_sizes,
        true,
    );

    // The code view goes through the same `ASTDiff` -> `TextDiff` path omnidiff's own output uses,
    // so it cannot drift from how the TUI would render the human's answer.
    let human_diff = human_mapping::as_ast_diff_for_mapping(mapping, before, after)
        .with_context(|| format!("building a synthetic ASTDiff for '{name}'"))?;
    let node_cache = NodeCache::build(before, after);
    let text_diff = TextDiff::from(before, after, &human_diff, &node_cache);
    let before_ranges = text_diff.all(0);
    let after_ranges = text_diff.all(1);
    let row_counts = [
        before.contents.split('\n').count(),
        after.contents.split('\n').count(),
    ];

    // The tree mapping's rendering, then one per painting. They are alternative accounts of the
    // same edit, so they are switchable panels, never merged.
    let mut renderings: Vec<(String, String, [PanelRanges; 2])> = vec![(
        "tree".to_string(),
        "From the node mapping".to_string(),
        [
            PanelRanges::from_tree(
                "b",
                "a",
                before_ranges.clone(),
                &after_ranges,
                row_counts[0],
                &groups,
                0,
            ),
            PanelRanges::from_tree(
                "a",
                "b",
                after_ranges.clone(),
                &before_ranges,
                row_counts[1],
                &groups,
                1,
            ),
        ],
    )];
    for (index, named) in mapping.text_mappings.iter().enumerate() {
        match painting_panels(named, &before.contents, &after.contents, index, row_counts) {
            Ok(panels) => renderings.push((format!("p{index}"), named.name.clone(), panels)),
            // One unreadable painting costs its own panel, not the page.
            Err(error) => warnings.push(format!("'{name}' painting: {error:#}")),
        }
    }

    // Folding is decided once per side across every rendering; see `code_visible_rows`.
    let mut anchors: [Vec<usize>; 2] = [Vec::new(), Vec::new()];
    for (_, _, panels) in &renderings {
        for side in 0..2 {
            anchors[side].extend(panels[side].anchor_rows());
        }
    }
    let visible = [
        code_visible_rows(&anchors[0], row_counts[0]),
        code_visible_rows(&anchors[1], row_counts[1]),
    ];

    let mut code_sections = String::new();
    let mut rendering_buttons = String::new();
    for (key, label, panels) in &renderings {
        let selected = key == "tree";
        // `viewer.js` switches by handle but remembers by name: `p0` differs between fixtures,
        // "Minimal" does not.
        let label_attr = escape_html_attr(label);
        rendering_buttons.push_str(&format!(
            r#"<button type="button" data-painting="{key}" data-painting-name="{label_attr}" aria-pressed="{selected}">{}</button>"#,
            escape_html_text(label)
        ));
        code_sections.push_str(&format!(
            r#"<div class="panels code-panels{hidden}" data-painting="{key}" data-painting-name="{label_attr}">
<section class="panel code-panel" data-side="before">
<h2>Before</h2>
<div class="code">{}</div>
</section>
<section class="panel code-panel" data-side="after">
<h2>After</h2>
<div class="code">{}</div>
</section>
</div>
"#,
            render_code_panel(&before.contents, &panels[0], &visible[0]),
            render_code_panel(&after.contents, &panels[1], &visible[1]),
            hidden = if selected { "" } else { " hidden" },
        ));
    }

    let painting_switch = if renderings.len() > 1 {
        format!(
            r#"<div class="painting-switch" role="group" aria-label="Code rendering">
<span class="painting-switch-label">Code view:</span>
{rendering_buttons}</div>
<p class="notice painting-notice">A painting is a person's account of this edit <em>as text</em>, recorded independently of the node mapping - a rendering often has several equally defensible answers where the mapping has one. Deletions and insertions carry no position on the opposite side in a painting, so that panel draws no caret for them.</p>"#
        )
    } else {
        String::new()
    };

    let description = match note {
        Some(note) => format!(
            r#"<p class="description">{}</p>"#,
            escape_html_text(note.trim())
        ),
        None => String::new(),
    };

    // An unfinished mapping otherwise looks the same as one whose unmarked nodes are deliberate.
    let unmarked_notice = if unmarked_nodes == 0 {
        String::new()
    } else {
        let total = human_mapping::total_node_count_for(before, after);
        format!(
            r#"<p class="notice">This mapping is unfinished: {unmarked_nodes} of {total} nodes are still unmarked.</p>"#
        )
    };

    // Building an `ASTDiff` collapses each group to one concrete pairing, so the page must say the
    // pairing shown is arbitrary (any-one-to-one) or partial (all-to-all).
    let any_one_to_one = mapping
        .groups
        .iter()
        .filter(|group| group.pairing == GroupPairing::AnyOneToOne)
        .count();
    let all_to_all = mapping.groups.len() - any_one_to_one;
    let mut groups_notice = String::new();
    if any_one_to_one > 0 {
        let plural = if any_one_to_one == 1 { "" } else { "s" };
        groups_notice.push_str(&format!(
            r#"<p class="notice">This mapping has {any_one_to_one} multi-map group{plural}: several pairings are equally correct there. The code view shows one arbitrary valid pairing, not the only one.</p>"#
        ));
    }
    if all_to_all > 0 {
        let plural = if all_to_all == 1 { "" } else { "s" };
        groups_notice.push_str(&format!(
            r#"<p class="notice">This mapping has {all_to_all} all-to-all group{plural}: every node on one side corresponds to every node on the other, and none is deleted or inserted. Each member carries an <span class="group-badge group-all">all N:M</span> badge; selecting one highlights all of its counterparts, in the tree and in the code view alike.</p>"#
        ));
    }

    let language = before.metadata.language.unwrap_or_default();
    // The URL needs the `DIFF_DATASETS` folder the fixture lives under.
    let dataset = helper::diffs_case_dir(name)
        .and_then(|dir| {
            dir.parent()
                .and_then(|p| p.file_name())
                .map(|f| f.to_string_lossy().into_owned())
        })
        .unwrap_or_else(|| "small".to_string());
    // A handmade fixture, or one whose repository does not resolve, has no upstream link.
    let upstream_link = match upstream {
        Some(upstream) => {
            let short: String = upstream.commit.chars().take(7).collect();
            format!(
                r#"<a class="source-link" href="{url}" target="_blank" rel="noopener">Upstream commit {repository}@{short}</a>
<span class="source-path" title="path in the upstream repository">{path}</span>"#,
                url = escape_html_attr(&upstream.commit_url),
                repository = escape_html_text(&upstream.repository),
                short = escape_html_text(&short),
                path = escape_html_text(&upstream.path),
            )
        }
        None => String::new(),
    };

    let source_url = format!(
        "https://github.com/{REPO}/tree/main/src/test/data/diffs/{}/{}",
        dataset,
        escape_html_attr(name)
    );

    let html = format!(
        r##"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>{name_escaped} — human mapping</title>
<link rel="stylesheet" href="../assets/style.css">
</head>
<body data-fixture="{name_attr}" data-repo="{repo}" data-revision="{revision}">
<header class="page-header">
<a class="back-link" href="../index.html">&larr; all fixtures</a>
<h1>{name_escaped}</h1>
<span class="language-badge">{language}</span>
<a class="source-link" href="{source_url}" target="_blank" rel="noopener">View before/after files on GitHub</a>
{upstream_link}
<ul class="legend">
<li><span class="status-unmarked">&#9679;</span> unmarked</li>
<li><span class="status-matched">&#9679;</span> matched</li>
<li><span class="status-matched op-update changed">&#9679;</span> updated</li>
<li><span class="status-matched changed">&#9679;</span> matched, not identical</li>
<li><span class="status-matched op-moved">&#9679;</span> moved, unchanged</li>
<li><span class="status-deleted">&#9679;</span> deleted</li>
<li><span class="status-inserted">&#9679;</span> inserted</li>
<li><span class="group-badge group-any">any 3:2</span> multi-map group: any pairing of the 3 with the 2 is valid, one is shown</li>
<li><span class="group-badge group-all">all 3:1</span> all-to-all group: each of the 3 corresponds to the 1</li>
</ul>
<ul class="legend code-legend">
<li><span class="cd cd-inserted-swatch cd-insert"></span> inserted</li>
<li><span class="cd cd-inserted-swatch cd-delete"></span> deleted</li>
<li><span class="cd cd-inserted-swatch cd-update"></span> updated</li>
<li><span class="cd cd-inserted-swatch cd-move"></span> moved</li>
<li>click a highlight to reveal its counterpart</li>
</ul>
<div class="view-switch" role="group" aria-label="View">
<button type="button" data-view="split" aria-pressed="true">Split</button>
<button type="button" data-view="code" aria-pressed="false">Code</button>
<button type="button" data-view="tree" aria-pressed="false">Tree</button>
</div>
</header>
{description}
{unmarked_notice}
{groups_notice}
{painting_switch}
{code_sections}<div class="panels tree-panels">
<section class="panel" data-side="before">
<h2>Before</h2>
<div class="tree">{before_html}</div>
</section>
<section class="panel" data-side="after">
<h2>After</h2>
<div class="tree">{after_html}</div>
</section>
</div>
<footer class="page-footer">
<a id="file-issue" href="#" aria-disabled="true" target="_blank" rel="noopener">File an issue about the selected node</a>
<button id="toggle-identical" type="button" aria-pressed="false">Hide identical matches</button>
<button id="toggle-reviewed" type="button" aria-pressed="false" title="Remembered in this browser only; forgotten if this fixture's mapping changes">Mark as reviewed</button>
<button id="random-unreviewed" type="button" data-fixtures-dir="">Random unreviewed fixture</button>
<span id="status-line" role="status"></span>
</footer>
{help_overlay}
{search_prompt}
<script src="../assets/fixtures.js"></script>
<script src="../assets/reviewed.js"></script>
<script src="../assets/viewer.js"></script>
</body>
</html>
"##,
        painting_switch = painting_switch,
        code_sections = code_sections,
        description = description,
        unmarked_notice = unmarked_notice,
        upstream_link = upstream_link,
        groups_notice = groups_notice,
        name_escaped = escape_html_text(name),
        name_attr = escape_html_attr(name),
        repo = REPO,
        revision = revision,
        help_overlay = HELP_OVERLAY_HTML,
        search_prompt = SEARCH_PROMPT_HTML,
    );
    Ok(FixturePage {
        html,
        unmarked_nodes,
        revision,
    })
}

const HELP_OVERLAY_HTML: &str = r#"<div id="help-overlay" class="hidden" role="dialog" aria-label="Keybindings">
<h2>Keybindings</h2>
<dl>
<dt>j / k</dt><dd>next / previous visible node (tree panels)</dd>
<dt>h / l</dt><dd>collapse / expand the focused node</dd>
<dt>g / G</dt><dd>jump to first / last visible node</dd>
<dt>Tab</dt><dd>switch focus between Before/After panels</dd>
<dt>/</dt><dd>search: jump to next node whose text contains a given string</dd>
<dt>a</dt><dd>jump the other panel to this node's mapped counterpart</dd>
<dt>i</dt><dd>hide identical matches, showing only inserted/deleted/updated nodes and their ancestors</dd>
<dt>v</dt><dd>cycle the view: split / code only / tree only</dd>
<dt>p</dt><dd>cycle what the code view renders: the node mapping, then each human painting</dd>
<dt>r</dt><dd>mark this fixture as reviewed, or forget the mark (kept in this browser only)</dd>
<dt>n</dt><dd>open a random fixture not yet marked as reviewed</dd>
<dt>?</dt><dd>toggle this help</dd>
</dl>
</div>"#;

const SEARCH_PROMPT_HTML: &str = r#"<div id="search-prompt" class="hidden" role="dialog" aria-label="Search">
<label for="search-input">Search (plain substring, no regex):</label>
<input id="search-input" type="text" autocomplete="off">
</div>"#;

/// A fully-quiet subtree (see `fully_quiet_subtree_sizes`) larger than this is replaced by a
/// placeholder rather than collapsed: a closed `<details>` still serializes its whole subtree, so
/// collapsing alone leaves the biggest fixtures' pages many megabytes. Smaller ones render closed.
const OMIT_THRESHOLD: usize = 20;

/// A multi-map group resolved to this parse's nodes, for what [`Caches`] cannot supply: the
/// group's badge and, for an all-to-all group, the full set of counterparts (the caches hold only
/// the one pair the one-to-one projection picked).
struct ResolvedGroup<'tree> {
    pairing: GroupPairing,
    before: Vec<Node<'tree>>,
    after: Vec<Node<'tree>>,
}

impl ResolvedGroup<'_> {
    /// "N:M", the shape a badge shows.
    fn shape(&self) -> String {
        format!("{}:{}", self.before.len(), self.after.len())
    }
}

/// Every group of `mapping`, in file order, so `Caches::before_group`/`after_group` indices address
/// this list. A member path that no longer resolves is dropped from its group, as
/// `rebuild_caches_for_mapping` does.
fn resolve_groups<'tree>(
    mapping: &HumanMapping,
    before_root: Node<'tree>,
    after_root: Node<'tree>,
) -> Vec<ResolvedGroup<'tree>> {
    let mut before_cache = helper::PathCache::new();
    let mut after_cache = helper::PathCache::new();
    mapping
        .groups
        .iter()
        .map(|group| ResolvedGroup {
            pairing: group.pairing,
            before: group
                .before_paths
                .iter()
                .filter_map(|path| {
                    before_cache
                        .resolve(before_root, &human_mapping::path_refs(path))
                        .ok()
                })
                .collect(),
            after: group
                .after_paths
                .iter()
                .filter_map(|path| {
                    after_cache
                        .resolve(after_root, &human_mapping::path_refs(path))
                        .ok()
                })
                .collect(),
        })
        .collect()
}

/// The text a node covers, in the row/byte-column space the code panels paint in.
fn node_extent(node: Node) -> TextRange {
    let (start, end) = (node.start_position(), node.end_position());
    TextRange::new(start.row, start.column, end.row, end.column)
}

/// Whether the painted range `inner` lies within the union of the node extents `extents`.
///
/// The union, because `TextDiff` merges adjacent ranges of one operation into one range that no
/// single member contains. Rows between the two ends must each be covered, so a range that sweeps
/// in a line belonging to no member is rejected.
///
/// A range ending at the next row's column 0 counts as ending on the row it closes: the text in
/// between is trailing whitespace and newline, which `paint_row_len` leaves unpainted.
fn extents_cover(extents: &[TextRange], inner: &TextRange) -> bool {
    if inner.is_empty() {
        return false;
    }
    let start = (inner.start_row, inner.start_column);
    // A row-boundary end folds back onto the row it closes, past any column a node can end on.
    let last = if inner.end_column == 0 {
        (inner.end_row - 1, usize::MAX)
    } else {
        (inner.end_row, inner.end_column - 1)
    };
    let inside = |(row, column): (usize, usize)| {
        extents.iter().any(|extent| {
            (extent.start_row, extent.start_column) <= (row, column)
                && (row < extent.end_row
                    || (row == extent.end_row
                        && (column < extent.end_column || column == usize::MAX)))
        })
    };
    let row_covered = |row: usize| {
        extents
            .iter()
            .any(|extent| extent.start_row <= row && row <= extent.end_row)
    };
    inside(start) && inside(last) && (start.0 + 1..last.0).all(row_covered)
}

/// Renders `node` and its subtree. `side` is `'b'` or `'a'`: it prefixes DOM ids (node ids of
/// the two independently parsed trees can collide) and picks which half of `caches` to read.
/// `quiet_sizes` is from `fully_quiet_subtree_sizes`. `force_open` exempts `node` itself (not its
/// descendants) from closing and from the placeholder, so a fully quiet root still renders.
fn render_node(
    node: Node,
    src: &[u8],
    side: char,
    caches: &Caches,
    groups: &[ResolvedGroup],
    quiet_sizes: &HashMap<usize, usize>,
    force_open: bool,
) -> String {
    let status = match side {
        'b' => status_before(node, caches),
        _ => status_after(node, caches),
    };
    // Whether a mark is the node's own or inherited only matters to `human_solver`'s editing.
    let (status_class, matched_other_id) = match status {
        NodeStatus::Unmarked => ("unmarked", None),
        NodeStatus::Matched => (
            "matched",
            match side {
                'b' => caches.before_match.get(&node.id()),
                _ => caches.after_match.get(&node.id()),
            },
        ),
        NodeStatus::Marked {
            kind: MarkKind::Deleted,
            ..
        } => ("deleted", None),
        NodeStatus::Marked {
            kind: MarkKind::Inserted,
            ..
        } => ("inserted", None),
    };
    // A non-identical match is a real edit, which "hide identical matches" must keep visible.
    let is_identical = match side {
        'b' => is_identical_before(node, caches),
        _ => is_identical_after(node, caches),
    };
    let changed_class = if status == NodeStatus::Matched && !is_identical {
        " changed"
    } else {
        ""
    };
    // Color only: a moved identical pair is still hidden by "hide identical matches".
    let (operation, moved) = match side {
        'b' => (
            match_operation_before(node, caches),
            is_moved_before(node, caches),
        ),
        _ => (
            match_operation_after(node, caches),
            is_moved_after(node, caches),
        ),
    };
    let operation_class = match operation {
        Some(HumanOperation::Update) => " op-update",
        Some(HumanOperation::Identical) if moved => " op-moved",
        _ => "",
    };

    let other_side = if side == 'b' { 'a' } else { 'b' };
    let id_attr = format!("{side}-{}", node.id());
    let mut match_attr = matched_other_id
        .map(|&other_id| format!(" data-match=\"{other_side}-{other_id}\""))
        .unwrap_or_default();
    let kind_attr = escape_html_attr(node.kind());

    // An all-to-all member's `data-match` names every counterpart, not just the one `caches`
    // paired it with; `viewer.js` highlights them all and aligns to the first.
    let group = match side {
        'b' => caches.before_group.get(&node.id()),
        _ => caches.after_group.get(&node.id()),
    }
    .and_then(|index| groups.get(*index));
    let (group_class, group_badge) = match group {
        Some(group) => {
            let shape = group.shape();
            let (before, after) = (group.before.len(), group.after.len());
            match group.pairing {
                GroupPairing::AllToAll => {
                    let theirs = if side == 'b' {
                        &group.after
                    } else {
                        &group.before
                    };
                    let ids: Vec<String> = theirs
                        .iter()
                        .map(|n| format!("{other_side}-{}", n.id()))
                        .collect();
                    if !ids.is_empty() {
                        match_attr = format!(" data-match=\"{}\"", ids.join(" "));
                    }
                    (
                        " group-all",
                        format!(
                            r#" <span class="group-badge group-all" title="all-to-all group: each of the {before} before nodes corresponds to each of the {after} after nodes">all {shape}</span>"#
                        ),
                    )
                }
                GroupPairing::AnyOneToOne => (
                    " group-any",
                    format!(
                        r#" <span class="group-badge group-any" title="multi-map group: any pairing of the {before} before nodes with the {after} after nodes is valid; the one shown is arbitrary">any {shape}</span>"#
                    ),
                ),
            }
        }
        None => ("", String::new()),
    };

    let quiet_size = quiet_sizes.get(&node.id()).copied();

    if !force_open && quiet_size.is_some_and(|size| size > OMIT_THRESHOLD) {
        let size = quiet_size.unwrap();
        let kind_label = escape_html_text(node.kind());
        // Keeps the real status and data-match: a placeholder can be the root of a large matched
        // block, whose counterpart is still worth linking to.
        return format!(
            r#"<div class="node leaf status-{status_class}{changed_class}{operation_class}{group_class} placeholder" id="{id_attr}"{match_attr} data-kind="{kind_attr}" tabindex="0">{kind_label} (+{size} nodes collapsed){group_badge}</div>"#
        );
    }

    if node.child_count() == 0 {
        let label = escape_html_text(&leaf_label(node, src));
        format!(
            r#"<div class="node leaf status-{status_class}{changed_class}{operation_class}{group_class}" id="{id_attr}"{match_attr} data-kind="{kind_attr}" tabindex="0">{label}{group_badge}</div>"#
        )
    } else {
        let mut children = String::new();
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            children.push_str(&render_node(
                child,
                src,
                side,
                caches,
                groups,
                quiet_sizes,
                false,
            ));
        }
        let kind_label = escape_html_text(node.kind());
        let open_attr = if force_open || quiet_size.is_none() {
            " open"
        } else {
            ""
        };
        format!(
            r#"<details class="node status-{status_class}{changed_class}{operation_class}{group_class}" id="{id_attr}"{match_attr} data-kind="{kind_attr}"{open_attr}><summary tabindex="0">{kind_label}{group_badge}</summary>{children}</details>"#
        )
    }
}

/// Same convention as `human_solver`'s `node_label`: kind plus a truncated, `Debug`-quoted
/// (Rust-escaped) snippet of the node's own text.
fn leaf_label(node: Node, src: &[u8]) -> String {
    let text = node.utf8_text(src).unwrap_or("");
    let truncated: String = text.chars().take(60).collect();
    let ellipsis = if text.chars().count() > 60 { "..." } else { "" };
    format!("{} {:?}{}", node.kind(), truncated, ellipsis)
}

/// Whether `node` is "quiet": unmarked, or matched and identical. Deletions, insertions and
/// non-identical matches are edits a reviewer must see, so they never are.
fn is_quiet(
    node: Node,
    caches: &Caches,
    status: NodeStatus,
    identical_fn: fn(Node, &Caches) -> bool,
) -> bool {
    match status {
        NodeStatus::Marked {
            kind: MarkKind::Deleted | MarkKind::Inserted,
            ..
        } => false,
        NodeStatus::Matched => identical_fn(node, caches),
        NodeStatus::Unmarked => true,
    }
}

/// Maps each node whose entire subtree is quiet (see `is_quiet`) to its subtree's node count,
/// itself included. `render_node` compares the count against `OMIT_THRESHOLD`.
fn fully_quiet_subtree_sizes(
    root: Node,
    caches: &Caches,
    status_fn: fn(Node, &Caches) -> NodeStatus,
    identical_fn: fn(Node, &Caches) -> bool,
) -> HashMap<usize, usize> {
    let mut sizes = HashMap::new();
    mark_fully_quiet(root, caches, status_fn, identical_fn, &mut sizes);
    sizes
}

/// Post-order: returns `Some(subtree size)` if `node`'s own subtree is fully quiet (recording it
/// in `sizes` too), `None` otherwise.
fn mark_fully_quiet(
    node: Node,
    caches: &Caches,
    status_fn: fn(Node, &Caches) -> NodeStatus,
    identical_fn: fn(Node, &Caches) -> bool,
    sizes: &mut HashMap<usize, usize>,
) -> Option<usize> {
    let mut cursor = node.walk();
    let mut all_children_quiet = true;
    let mut subtree_size = 1usize;
    for child in node.children(&mut cursor) {
        match mark_fully_quiet(child, caches, status_fn, identical_fn, sizes) {
            Some(child_size) => subtree_size += child_size,
            None => all_children_quiet = false,
        }
    }

    let quiet = all_children_quiet && is_quiet(node, caches, status_fn(node, caches), identical_fn);
    if quiet {
        sizes.insert(node.id(), subtree_size);
        Some(subtree_size)
    } else {
        None
    }
}

/// Rows of unchanged context kept around every changed row in a code panel; the rest fold, for
/// the same page-size reason as `OMIT_THRESHOLD`.
const CODE_CONTEXT_ROWS: usize = 6;

/// A run of consecutive unchanged, out-of-context rows shorter than this is rendered in full
/// rather than folded: below it the fold placeholder costs about as much markup as the rows it
/// replaces, and reading around a two-line gap is worse than reading through it.
const CODE_FOLD_THRESHOLD: usize = 4;

/// The CSS class painting `operation`, or `None` for the two sentinels that mean "not a change" -
/// `Identical` text is the panel's plain, unhighlighted background, so it gets no span at all.
fn code_operation_class(operation: &TextOperation) -> Option<&'static str> {
    match operation {
        TextOperation::Insert => Some("cd-insert"),
        TextOperation::Delete => Some("cd-delete"),
        TextOperation::Update => Some("cd-update"),
        TextOperation::Move => Some("cd-move"),
        TextOperation::Identical | TextOperation::NotYetSet => None,
    }
}

/// A caret drawn on one side to mark where the *other* side's inserted or deleted text belongs.
///
/// Read off the other side's `destination`: on a pure deletion the after side's own range list
/// has no non-`Identical` entry at all, so this side's ranges cannot supply it.
struct CodeMarker {
    row: usize,
    column: usize,
    operation: TextOperation,
    /// This caret's own `data-range` id.
    id: String,
    /// The `data-range` id of the other side's text that this caret stands in for.
    points_at: String,
}

/// Positional `data-range` ids, as the tree-derived panels use. Paintings share ids instead; see
/// [`painting_panels`].
fn positional_ids(side: &str, count: usize) -> Vec<String> {
    (0..count).map(|index| format!("{side}{index}")).collect()
}

/// Every caret one side should draw, read off `other`'s ranges (see [`CodeMarker`]).
///
/// The clamp to `row_count` (this side's line count) is load-bearing: a delete at end-of-file has
/// its destination one row past the last rendered row, and unclamped its caret would silently
/// vanish (`python-api-change` does this).
fn code_markers(
    other: &[RangeMatch],
    row_count: usize,
    side: &str,
    other_ids: &[String],
) -> Vec<CodeMarker> {
    other
        .iter()
        .enumerate()
        .filter(|(_, range_match)| {
            code_operation_class(&range_match.operation).is_some()
                && !range_match.source.is_empty()
                && range_match.destination.is_empty()
        })
        .map(|(index, range_match)| CodeMarker {
            row: range_match
                .destination
                .start_row
                .min(row_count.saturating_sub(1)),
            column: range_match.destination.start_column,
            operation: range_match.operation.clone(),
            id: format!("{side}m{index}"),
            points_at: other_ids[index].clone(),
        })
        .collect()
}

/// Maps each index in `from`'s range list to the `data-range` id of the thing on the *other* side
/// that it points at, so a clicked span can reveal its counterpart.
///
/// The counterpart is found by exact lookup of `destination` among the other side's `source`
/// extents; the rare range with no exact match renders unlinked rather than guessing by overlap.
/// An insert or delete links to the caret [`code_markers`] draws for it.
fn code_counterparts(
    from: &[RangeMatch],
    to: &[RangeMatch],
    to_side: &str,
    to_ids: &[String],
) -> HashMap<usize, String> {
    let key = |r: &omnidiff::diff::text_range::TextRange| {
        (r.start_row, r.start_column, r.end_row, r.end_column)
    };
    let mut by_source: HashMap<(usize, usize, usize, usize), usize> = HashMap::new();
    for (index, range_match) in to.iter().enumerate() {
        if range_match.source.is_empty() {
            continue;
        }
        by_source.entry(key(&range_match.source)).or_insert(index);
    }

    from.iter()
        .enumerate()
        .filter(|(_, range_match)| code_operation_class(&range_match.operation).is_some())
        .filter_map(|(index, range_match)| {
            if range_match.destination.is_empty() {
                // `code_markers` names the caret after the range it stands in for: this one.
                return Some((index, format!("{to_side}m{index}")));
            }
            by_source
                .get(&key(&range_match.destination))
                .map(|&other| (index, to_ids[other].clone()))
        })
        .collect()
}

/// Gives every range inside an all-to-all group's members one shared id per side, pointing at the
/// other side's shared id, as [`painting_panels`] does for an N:M match: the group asserts the
/// correspondence whole, so clicking one member reveals every counterpart.
///
/// A member's `Identical` range is promoted to `Move`, because verbatim copies are the very edit
/// such a group records and unpainted text would show none of it.
///
/// Only `Identical`, `Move` and `Update` ranges within a member's extent qualify: a descendant
/// deleted or inserted inside a member is not part of what the group asserts.
#[allow(clippy::too_many_arguments)]
fn share_all_to_all_ids(
    ranges: &mut [RangeMatch],
    ids: &mut [String],
    counterparts: &mut HashMap<usize, String>,
    side: &str,
    other_side: &str,
    groups: &[ResolvedGroup],
    side_index: usize,
) {
    for (group_index, group) in groups.iter().enumerate() {
        if group.pairing != GroupPairing::AllToAll {
            continue;
        }
        let members = if side_index == 0 {
            &group.before
        } else {
            &group.after
        };
        let extents: Vec<TextRange> = members.iter().map(|node| node_extent(*node)).collect();
        for (index, range_match) in ranges.iter_mut().enumerate() {
            let linked = matches!(
                range_match.operation,
                TextOperation::Identical | TextOperation::Move | TextOperation::Update
            );
            if !linked || !extents_cover(&extents, &range_match.source) {
                continue;
            }
            if range_match.operation == TextOperation::Identical {
                range_match.operation = TextOperation::Move;
            }
            ids[index] = format!("{side}G{group_index}");
            counterparts.insert(index, format!("{other_side}G{group_index}"));
        }
    }
}

/// One side of one code rendering: the ranges to paint onto that side's source text, the DOM ids
/// they carry, what each points at on the other side, and the carets standing in for text this
/// side doesn't have.
///
/// A page holds one per rendering per side, so each needs its own `side` prefix: `viewer.js`
/// looks ids up document-wide, and a shared prefix would select spans in a hidden panel.
struct PanelRanges {
    /// DOM id prefix for this panel: `b`/`a` for the tree mapping, `b{k}`/`a{k}` for painting *k*.
    side: String,
    ranges: Vec<RangeMatch>,
    /// `data-range` id per range, parallel to `ranges`. Several ranges may share one id: an N:M
    /// match is one decision.
    ids: Vec<String>,
    /// `data-counterpart` per range index, where there is something on the other side to point at.
    counterparts: HashMap<usize, String>,
    markers: Vec<CodeMarker>,
    /// This side's per-row operation, from `line_operations`.
    ops: Vec<TextOperation>,
}

impl PanelRanges {
    /// One side of the tree mapping's rendering. `side_index` is 0 for before, 1 for after.
    fn from_tree(
        side: &str,
        other_side: &str,
        ranges: Vec<RangeMatch>,
        other: &[RangeMatch],
        row_count: usize,
        groups: &[ResolvedGroup],
        side_index: usize,
    ) -> Self {
        let mut ranges = ranges;
        let mut ids = positional_ids(side, ranges.len());
        let other_ids = positional_ids(other_side, other.len());
        let mut counterparts = code_counterparts(&ranges, other, other_side, &other_ids);
        share_all_to_all_ids(
            &mut ranges,
            &mut ids,
            &mut counterparts,
            side,
            other_side,
            groups,
            side_index,
        );
        PanelRanges {
            counterparts,
            markers: code_markers(other, row_count, side, &other_ids),
            ops: omnidiff::diff::text::line_operations(&ranges, row_count),
            side: side.to_string(),
            ids,
            ranges,
        }
    }

    /// Rows worth centering a fold on for *this* rendering: changed rows, plus every caret's row.
    ///
    /// Carets matter: `line_operations` cannot see them, so without them a pure deletion's after
    /// side would fold away entirely.
    fn anchor_rows(&self) -> Vec<usize> {
        let mut anchors: Vec<usize> = self
            .ops
            .iter()
            .enumerate()
            .filter(|(_, op)| **op != TextOperation::Identical && **op != TextOperation::NotYetSet)
            .map(|(row, _)| row)
            .collect();
        anchors.extend(self.markers.iter().map(|marker| marker.row));
        anchors
    }
}

/// Both sides of one named painting, as the panels [`render_code_panel`] already draws.
///
/// Each span becomes a `RangeMatch` with a zero `destination`; links come from the entry's
/// grouping instead. A painting records no caret positions, so these panels draw none.
///
/// Every span of one entry's side shares an id: the entry does not record which span pairs with
/// which (see `HumanTextEntry`), so naming a pairing would invent one.
///
/// `Err` if any entry is malformed or falls outside its file (see `HumanTextEntry::verdict`).
fn painting_panels(
    named: &human_mapping::NamedTextMapping,
    before: &str,
    after: &str,
    painting: usize,
    row_counts: [usize; 2],
) -> Result<[PanelRanges; 2]> {
    let sides = [format!("b{painting}"), format!("a{painting}")];
    let mut ranges: [Vec<RangeMatch>; 2] = [Vec::new(), Vec::new()];
    let mut ids: [Vec<String>; 2] = [Vec::new(), Vec::new()];
    let mut counterparts: [HashMap<usize, String>; 2] = [HashMap::new(), HashMap::new()];

    for (entry_index, entry) in named.mapping.entries.iter().enumerate() {
        let verdict = entry
            .verdict(before, after)
            .with_context(|| format!("entry {entry_index} of the '{}' painting", named.name))?;
        let operation = match verdict {
            HumanTextVerdict::Move => TextOperation::Move,
            HumanTextVerdict::Update => TextOperation::Update,
            HumanTextVerdict::Delete => TextOperation::Delete,
            HumanTextVerdict::Insert => TextOperation::Insert,
        };
        // A `Match` is the only entry with text on both sides, so it is the only one whose spans
        // have anything to point at.
        let linked = matches!(verdict, HumanTextVerdict::Move | HumanTextVerdict::Update);
        let entry_ids = [
            format!("{}e{entry_index}", sides[0]),
            format!("{}e{entry_index}", sides[1]),
        ];
        for (side, spans) in [(0usize, &entry.before), (1usize, &entry.after)] {
            for span in spans {
                let index = ranges[side].len();
                ranges[side].push(RangeMatch {
                    source: span.to_text_range(),
                    destination: omnidiff::diff::text_range::TextRange::zero(),
                    operation: operation.clone(),
                });
                ids[side].push(entry_ids[side].clone());
                if linked {
                    counterparts[side].insert(index, entry_ids[1 - side].clone());
                }
            }
        }
    }

    let [before_ranges, after_ranges] = ranges;
    let [before_ids, after_ids] = ids;
    let [before_counterparts, after_counterparts] = counterparts;
    Ok([
        PanelRanges {
            ops: omnidiff::diff::text::line_operations(&before_ranges, row_counts[0]),
            side: sides[0].clone(),
            ranges: before_ranges,
            ids: before_ids,
            counterparts: before_counterparts,
            markers: Vec::new(),
        },
        PanelRanges {
            ops: omnidiff::diff::text::line_operations(&after_ranges, row_counts[1]),
            side: sides[1].clone(),
            ranges: after_ranges,
            ids: after_ids,
            counterparts: after_counterparts,
            markers: Vec::new(),
        },
    ])
}

/// Which rows of a panel are actually rendered: every anchor, plus `CODE_CONTEXT_ROWS` on each
/// side of it. Everything else is folded away by [`render_code_panel`].
///
/// `anchors` is the union over every rendering of this side, so flipping between renderings
/// keeps rows in place, and a painting panel (which draws no carets) still folds.
///
/// With no anchors at all everything stays visible: a blank panel is worse than a long one.
fn code_visible_rows(anchors: &[usize], row_count: usize) -> Vec<bool> {
    if anchors.is_empty() {
        return vec![true; row_count];
    }

    let mut visible = vec![false; row_count];
    for &row in anchors {
        let start = row.saturating_sub(CODE_CONTEXT_ROWS);
        let end = (row + CODE_CONTEXT_ROWS + 1).min(row_count);
        for slot in visible.iter_mut().take(end).skip(start) {
            *slot = true;
        }
    }
    visible
}

/// Renders one side's source text with a rendering's changes painted onto it, character-precise -
/// the code-shaped counterpart to `render_node`'s tree.
///
/// Walks the source text and opens spans where ranges cover it, rather than concatenating the
/// ranges' text: ranges leave whitespace gaps between themselves, which concatenation would drop.
/// Pinned by `render_code_panel_reproduces_the_source_text_exactly`.
fn render_code_panel(contents: &str, panel: &PanelRanges, visible: &[bool]) -> String {
    let lines: Vec<&str> = contents.split('\n').collect();

    let mut markers_by_row: HashMap<usize, Vec<&CodeMarker>> = HashMap::new();
    for marker in &panel.markers {
        markers_by_row.entry(marker.row).or_default().push(marker);
    }
    let no_markers: Vec<&CodeMarker> = Vec::new();
    let row_html = |row: usize| {
        render_code_row(
            lines[row],
            row,
            &panel.ops[row],
            panel,
            markers_by_row.get(&row).unwrap_or(&no_markers),
        )
    };

    let mut html = String::new();
    let mut row = 0usize;
    while row < lines.len() {
        if visible[row] {
            html.push_str(&row_html(row));
            row += 1;
            continue;
        }
        let start = row;
        while row < lines.len() && !visible[row] {
            row += 1;
        }
        let folded = row - start;
        if folded >= CODE_FOLD_THRESHOLD {
            // A 1-indexed line range, which can be checked against the gutter; a bare count cannot.
            let (first, last) = (start + 1, row);
            html.push_str(&format!(
                r#"<div class="cl fold"><span class="ln">&hellip;</span><span class="lt">lines {first}&ndash;{last} unchanged ({folded} lines)</span></div>"#
            ));
        } else {
            for short_row in start..row {
                html.push_str(&row_html(short_row));
            }
        }
    }
    html
}

/// One row of a code panel: a line-number gutter cell plus the row's text, split into plain
/// stretches, highlighted spans, and any carets that belong on it.
fn render_code_row(
    line: &str,
    row: usize,
    row_op: &TextOperation,
    panel: &PanelRanges,
    markers: &[&CodeMarker],
) -> String {
    let row_len = line.len();
    // A row a range spans wholly is painted only to its last real character, but `row_len` stays
    // untrimmed so the unpainted trailing whitespace is still emitted.
    let paint_row_len = omnidiff::diff::text_range::paint_row_len(line);
    let side = &panel.side;

    // Byte-column spans for this row: painted ranges, plus carets as zero-width spans.
    let mut segments: Vec<(usize, usize, &TextOperation, &String, Option<&String>)> = panel
        .ranges
        .iter()
        .enumerate()
        .filter(|(_, range_match)| {
            code_operation_class(&range_match.operation).is_some() && !range_match.source.is_empty()
        })
        .filter_map(|(index, range_match)| {
            range_match.source.columns_on_row(row, paint_row_len).map(
                |(start, end)| -> (usize, usize, &TextOperation, &String, Option<&String>) {
                    (
                        start,
                        end,
                        &range_match.operation,
                        &panel.ids[index],
                        panel.counterparts.get(&index),
                    )
                },
            )
        })
        .collect();
    segments.extend(markers.iter().map(|marker| {
        let column = omnidiff::diff::text_range::floor_char_boundary(line, marker.column);
        (
            column,
            column,
            &marker.operation,
            &marker.id,
            Some(&marker.points_at),
        )
    }));
    // A caret sorts before a range starting at the same column, as in
    // `widgets::code_viewer::build_range_order`.
    segments.sort_by_key(|(start, end, _, _, _)| (*start, *end));

    let mut text = String::new();
    let mut cursor = 0usize;
    let mut has_marker = false;
    for (start, end, operation, id, counterpart) in segments {
        let start = omnidiff::diff::text_range::floor_char_boundary(line, start).max(cursor);
        let end = omnidiff::diff::text_range::floor_char_boundary(line, end).max(start);
        if start > cursor {
            text.push_str(&escape_html_text(&line[cursor..start]));
        }
        // Unreachable `None`; defaulted rather than unwrapped so one row cannot fail the build.
        let class = code_operation_class(operation).unwrap_or("cd-update");
        let counterpart_attr = counterpart
            .map(|other| format!(" data-counterpart=\"{other}\""))
            .unwrap_or_default();
        if end > start {
            text.push_str(&format!(
                r#"<span class="cd {class}" data-range="{id}"{counterpart_attr} tabindex="0">{}</span>"#,
                escape_html_text(&line[start..end])
            ));
        } else {
            has_marker = true;
            let title = match operation {
                TextOperation::Delete => "deleted here",
                _ => "inserted here",
            };
            text.push_str(&format!(
                r#"<span class="cd cd-gap {class}" data-range="{id}"{counterpart_attr} title="{title}" tabindex="0"></span>"#
            ));
        }
        cursor = end;
    }
    if cursor < row_len {
        text.push_str(&escape_html_text(&line[cursor..]));
    }

    // A row tint keeps changed rows findable while scrolling. A caret-only row's text is
    // unchanged, so it gets a marker class instead of a tint.
    let row_class = match code_operation_class(row_op) {
        Some(class) => format!(" row-{class}"),
        None if has_marker => " row-gap".to_string(),
        None => String::new(),
    };
    format!(
        r#"<div class="cl{row_class}" id="{side}L{row}"><span class="ln">{}</span><span class="lt">{text}</span></div>"#,
        row + 1
    )
}

/// One row of the index page's sortable table.
struct IndexEntry {
    name: String,
    language: Language,
    /// Line-level mismatches against the human mapping - see `human_mapping::LineMismatches`.
    omnidiff_mismatches: usize,
    unix_diff_mismatches: usize,
    total_lines: usize,
    /// The fixture's `description.md`, if it has one.
    note: Option<String>,
    /// Nodes the human mapping still says nothing about; `0` is a finished mapping.
    unmarked_nodes: usize,
    /// Every painting's name, in file order. Empty means unpainted.
    paintings: Vec<String>,
    /// See [`fixture_revision`].
    revision: String,
}

/// `assets/fixtures.js`: every fixture's name and revision as one array on `window`, for the
/// "random unreviewed fixture" button on fixture pages, which have no table to read the list from.
/// A script rather than JSON to `fetch`, so a site opened from a file:// URL works too. It is one
/// cached asset shared by every page, not something inflating each of them.
fn render_fixtures_script(entries: &[IndexEntry]) -> String {
    let items: Vec<String> = entries
        .iter()
        .map(|entry| {
            format!(
                "{{name:{},revision:{}}}",
                serde_json::to_string(&entry.name).expect("a string serializes"),
                serde_json::to_string(&entry.revision).expect("a string serializes"),
            )
        })
        .collect();
    format!(
        "// Generated by generate_mapping_site.rs; read by reviewed.js.\nwindow.OMNIDIFF_FIXTURES = [\n{}\n];\n",
        items.join(",\n")
    )
}

fn render_index_page(entries: &[IndexEntry]) -> String {
    let mut rows = String::new();
    for entry in entries {
        let name_attr = escape_html_attr(&entry.name);
        let name_escaped = escape_html_text(&entry.name);
        rows.push_str(&format!(
            r#"<tr data-name="{name_attr}" data-language="{language}" data-omnidiff="{omnidiff}" data-unix_diff="{unix_diff}" data-total_lines="{total_lines}" data-paintings="{painting_count}" data-unmarked="{unmarked}" data-revision="{revision}" data-reviewed="0">
<td><a href="fixtures/{name_attr}.html">{name_escaped}</a>{note}</td>
<td><span class="language-badge">{language}</span></td>
<td>{omnidiff}</td>
<td>{unix_diff}</td>
<td>{total_lines}</td>
<td class="paintings">{painting_names}</td>
<td>{unmarked_cell}</td>
<td class="reviewed"><input type="checkbox" class="reviewed-mark" aria-label="Reviewed: {name_attr}"></td>
</tr>
"#,
            language = entry.language,
            revision = entry.revision,
            omnidiff = entry.omnidiff_mismatches,
            unix_diff = entry.unix_diff_mismatches,
            total_lines = entry.total_lines,
            painting_count = entry.paintings.len(),
            unmarked = entry.unmarked_nodes,
            unmarked_cell = if entry.unmarked_nodes == 0 {
                "&mdash;".to_string()
            } else {
                entry.unmarked_nodes.to_string()
            },
            // Under the name: a column wide enough for free prose squeezes out the numbers.
            note = match &entry.note {
                Some(note) => format!(
                    r#"<div class="fixture-note">{}</div>"#,
                    escape_html_text(note.trim())
                ),
                None => String::new(),
            },
            painting_names = if entry.paintings.is_empty() {
                // Sorts on `data-paintings`; the cell reads as absence, not a count.
                "&mdash;".to_string()
            } else {
                escape_html_text(&entry.paintings.join(", "))
            },
        ));
    }

    format!(
        r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>omnidiff human mappings</title>
<link rel="stylesheet" href="assets/style.css">
</head>
<body>
<header class="page-header">
<h1>Human-authored ground-truth mappings</h1>
<p class="notice">Looking for the tool rather than its ground truth? <a href="showcase/index.html">Twenty
of these changes in omnidiff's own viewer</a>, each shown as Unix <code>diff</code> marks it and as
omnidiff maps it.</p>
<p>Each page below shows one fixture's before/after AST, annotated with what a human decided
should match, get deleted, or get inserted. Disagree with one? Open the fixture, select the node,
and use the "file an issue" button.</p>
<p>"omnidiff mismatches"/"unix diff mismatches" are line-level disagreements against the human
mapping (see the introductory paper for why line granularity, not AST-node granularity, is the only
fair way to compare omnidiff against a line-only tool like Unix <code>diff</code>) - click a column
header to sort by it.</p>
<p>"Paintings" lists the hand-recorded accounts of each diff <em>as text</em>, kept independently of
the node mapping because a rendering often has several equally correct answers where the mapping has
one. Open a fixture and use the Code view's switch to flip between them and the mapping's own
rendering. A dash means nobody has painted that fixture yet.</p>
<p>"Unmarked nodes" counts what the human mapping still says nothing about - a dash means the
mapping is finished. Sort by it to find the ones that still need work. Where a fixture carries a
description, it appears under its name.</p>
<p>"Reviewed" is yours to tick: it means you looked at that fixture and had nothing to file. It is
kept in this browser only, and a fixture whose mapping changes after you reviewed it drops back to
half-ticked so you know to look again. Sort by it to see what is left.</p>
<div class="review-controls">
<span id="review-progress" role="status"></span>
<button id="random-unreviewed" type="button" data-fixtures-dir="fixtures/">Random unreviewed fixture</button>
</div>
</header>
<table class="fixture-table" id="fixture-table">
<thead>
<tr>
<th data-sort="name" data-type="string" tabindex="0" aria-sort="ascending">Fixture</th>
<th data-sort="language" data-type="string" tabindex="0" aria-sort="none">Language</th>
<th data-sort="omnidiff" data-type="number" tabindex="0" aria-sort="none">omnidiff mismatches</th>
<th data-sort="unix_diff" data-type="number" tabindex="0" aria-sort="none">Unix diff mismatches</th>
<th data-sort="total_lines" data-type="number" tabindex="0" aria-sort="none">Total lines</th>
<th data-sort="paintings" data-type="number" tabindex="0" aria-sort="none">Paintings</th>
<th data-sort="unmarked" data-type="number" tabindex="0" aria-sort="none">Unmarked nodes</th>
<th data-sort="reviewed" data-type="number" tabindex="0" aria-sort="none">Reviewed</th>
</tr>
</thead>
<tbody>
{rows}</tbody>
</table>
<script src="assets/reviewed.js"></script>
<script src="assets/index.js"></script>
</body>
</html>
"#
    )
}

fn escape_html_text(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn escape_html_attr(s: &str) -> String {
    escape_html_text(s).replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use omnidiff::test::helper::human_mapping::{
        HumanMapping, HumanMappingEntry, HumanOperation, HumanTextEntry, HumanTextOperation,
        HumanTextSpan,
    };

    /// Strips tags and undoes `escape_html_text`. Only for this module's own output; not a
    /// general HTML parser.
    fn strip_tags(html: &str) -> String {
        let mut out = String::new();
        let mut in_tag = false;
        for ch in html.chars() {
            match ch {
                '<' => in_tag = true,
                '>' => in_tag = false,
                _ if !in_tag => out.push(ch),
                _ => {}
            }
        }
        // `&amp;` last, or a literal `&amp;lt;` in the source would become `<`.
        out.replace("&lt;", "<")
            .replace("&gt;", ">")
            .replace("&amp;", "&")
    }

    /// Every rendered row of a code panel as `(row index, plain text)`, keyed by the
    /// `id="{side}L{row}"` attribute. Fold placeholders carry no id and are skipped.
    fn rendered_rows(html: &str, side: &str) -> Vec<(usize, String)> {
        let id_prefix = format!("\" id=\"{side}L");
        html.split("<div class=\"cl")
            .filter_map(|chunk| {
                let id_start = chunk.find(&id_prefix)? + id_prefix.len();
                let id_end = id_start + chunk[id_start..].find('"')?;
                let row: usize = chunk[id_start..id_end].parse().ok()?;
                let text_start = chunk.find("<span class=\"lt\">")? + "<span class=\"lt\">".len();
                let text_end = chunk.rfind("</span></div>")?;
                Some((row, strip_tags(&chunk[text_start..text_end])))
            })
            .collect()
    }

    /// The code panel renders the file itself, not a reassembly of range texts, which would
    /// silently drop the whitespace gaps between ranges.
    #[test]
    fn render_code_panel_reproduces_the_source_text_exactly() {
        let mut checked = 0usize;
        let mut painted = 0usize;
        for name in omnidiff::test::helper::UNIT_TEST_FIXTURES {
            let Ok(pair) = helper::handmade_test_code_pair(name) else {
                continue;
            };
            let (before, after) = &*pair;
            let Ok(mapping) = human_mapping::load(name) else {
                continue;
            };
            let Ok(diff) = human_mapping::as_ast_diff_for_mapping(&mapping, before, after) else {
                continue;
            };
            let node_cache = NodeCache::build(before, after);
            let text_diff = TextDiff::from(before, after, &diff, &node_cache);
            let before_ranges = text_diff.all(0);
            let after_ranges = text_diff.all(1);
            let row_counts = [
                before.contents.split('\n').count(),
                after.contents.split('\n').count(),
            ];

            // Paintings too: their hand-recorded spans need this guarantee most.
            let mut renderings = vec![[
                PanelRanges::from_tree(
                    "b",
                    "a",
                    before_ranges.clone(),
                    &after_ranges,
                    row_counts[0],
                    &[],
                    0,
                ),
                PanelRanges::from_tree(
                    "a",
                    "b",
                    after_ranges.clone(),
                    &before_ranges,
                    row_counts[1],
                    &[],
                    1,
                ),
            ]];
            for (index, named) in mapping.text_mappings.iter().enumerate() {
                let panels =
                    painting_panels(named, &before.contents, &after.contents, index, row_counts)
                        .unwrap_or_else(|error| {
                            panic!("'{name}' painting '{}': {error:#}", named.name)
                        });
                renderings.push(panels);
                painted += 1;
            }

            let source_lines = [
                before.contents.split('\n').collect::<Vec<&str>>(),
                after.contents.split('\n').collect::<Vec<&str>>(),
            ];
            for panels in &renderings {
                for (side, contents) in [(0usize, &before.contents), (1usize, &after.contents)] {
                    let panel = &panels[side];
                    // Nothing folded, so every row of the file has to come back.
                    let html = render_code_panel(contents, panel, &vec![true; row_counts[side]]);
                    let rows = rendered_rows(&html, &panel.side);
                    assert!(
                        !rows.is_empty(),
                        "'{name}' panel '{}' rendered no rows at all",
                        panel.side
                    );
                    for (row, text) in rows {
                        assert_eq!(
                            text, source_lines[side][row],
                            "'{name}' panel '{}' row {row} does not reproduce its source line",
                            panel.side
                        );
                    }
                }
            }
            checked += 1;
        }
        assert!(
            checked > 0,
            "no fixture in UNIT_TEST_FIXTURES had a loadable human mapping - \
             this test would silently pass while checking nothing"
        );
        // A floor, not `> 0`: one painted fixture would leave the painted-panel property barely
        // measured.
        assert!(
            painted >= 10,
            "only {painted} painting(s) in UNIT_TEST_FIXTURES reached this check - the painted \
             half of the test is barely measuring anything"
        );
    }

    #[test]
    fn render_code_row_paints_only_the_changed_columns() {
        let ranges = vec![
            RangeMatch {
                source: omnidiff::diff::text_range::TextRange::new(0, 4, 0, 7),
                destination: omnidiff::diff::text_range::TextRange::new(0, 4, 0, 7),
                operation: TextOperation::Update,
            },
            RangeMatch {
                source: omnidiff::diff::text_range::TextRange::new(0, 0, 0, 4),
                destination: omnidiff::diff::text_range::TextRange::new(0, 0, 0, 4),
                operation: TextOperation::Identical,
            },
        ];
        let panel = PanelRanges::from_tree("b", "a", ranges, &[], 1, &[], 0);
        let html = render_code_row("let foo = 1;", 0, &TextOperation::Update, &panel, &[]);

        assert!(
            html.contains(r#"<span class="cd cd-update" data-range="b0" tabindex="0">foo</span>"#),
            "expected exactly the update columns to be wrapped, got: {html}"
        );
        assert_eq!(strip_tags(&html), "1let foo = 1;");
    }

    /// A range spanning a row it does not end on is painted only to the row's last real
    /// character; the trailing whitespace is still emitted, outside the span.
    #[test]
    fn render_code_row_does_not_wrap_a_middle_rows_trailing_whitespace() {
        let ranges = vec![RangeMatch {
            source: omnidiff::diff::text_range::TextRange::new(0, 0, 1, 3),
            destination: omnidiff::diff::text_range::TextRange::new(0, 0, 1, 3),
            operation: TextOperation::Move,
        }];
        let panel = PanelRanges::from_tree("b", "a", ranges, &[], 1, &[], 0);
        let html = render_code_row("foo   ", 0, &TextOperation::Move, &panel, &[]);

        assert!(
            html.contains(r#"<span class="cd cd-move" data-range="b0" tabindex="0">foo</span>"#),
            "expected exactly 'foo' wrapped, not the trailing spaces: {html}"
        );
        assert_eq!(strip_tags(&html), "1foo   ");
    }

    /// On a pure deletion the after side has no range of its own; the caret is read off the
    /// before side's `destination`.
    #[test]
    fn render_code_row_draws_a_caret_for_the_other_sides_deletion() {
        use omnidiff::diff::text_range::TextRange;

        let before = vec![RangeMatch {
            source: TextRange::new(7, 0, 9, 0),
            destination: TextRange::new(0, 4, 0, 4),
            operation: TextOperation::Delete,
        }];
        let panel = PanelRanges::from_tree("a", "b", Vec::new(), &before, 20, &[], 1);
        assert_eq!(
            panel.markers.len(),
            1,
            "the delete should produce exactly one caret"
        );

        let html = render_code_row(
            "let foo = 1;",
            0,
            &TextOperation::Identical,
            &panel,
            &panel.markers.iter().collect::<Vec<_>>(),
        );

        assert!(
            html.contains(
                r#"<span class="cd cd-gap cd-delete" data-range="am0" data-counterpart="b0" title="deleted here" tabindex="0"></span>"#
            ),
            "expected an empty caret span pointing back at the deleted text, got: {html}"
        );
        // The deleted text links back to the caret.
        assert_eq!(
            code_counterparts(&before, &[], "a", &[])
                .get(&0)
                .map(String::as_str),
            Some("am0")
        );
        // A caret does not change its row's text, so the row gets the marker class, not a tint.
        assert!(html.contains(r#"class="cl row-gap""#), "got: {html}");
        assert_eq!(strip_tags(&html), "1let foo = 1;");
    }

    #[test]
    fn code_markers_clamps_an_end_of_file_caret_onto_the_last_row() {
        use omnidiff::diff::text_range::TextRange;

        let before = vec![RangeMatch {
            source: TextRange::new(3, 0, 4, 0),
            destination: TextRange::new(18, 0, 18, 0),
            operation: TextOperation::Delete,
        }];
        let markers = code_markers(&before, 18, "a", &positional_ids("b", 1));

        assert_eq!(markers.len(), 1);
        assert_eq!(markers[0].row, 17);
    }

    #[test]
    fn render_code_panel_folds_a_long_unchanged_run_but_renders_a_short_one() {
        let contents = "l0\nl1\nl2\nl3\nl4\nl5\nl6\nl7\nl8\nl9";
        let panel = PanelRanges::from_tree("a", "b", Vec::new(), &[], 10, &[], 1);
        let mut visible = vec![false; 10];
        for row in [0, 3, 9] {
            visible[row] = true;
        }

        let html = render_code_panel(contents, &panel, &visible);

        let rows: Vec<usize> = rendered_rows(&html, "a")
            .into_iter()
            .map(|(row, _)| row)
            .collect();
        assert_eq!(rows, vec![0, 1, 2, 3, 9]);
        assert!(
            html.contains("lines 5&ndash;9 unchanged (5 lines)"),
            "got: {html}"
        );
    }

    #[test]
    fn code_visible_rows_keeps_context_around_a_change_and_drops_the_rest() {
        let visible = code_visible_rows(&[15], 30);

        assert!(visible[15], "the changed row itself must be visible");
        assert!(visible[15 - CODE_CONTEXT_ROWS], "context above");
        assert!(visible[15 + CODE_CONTEXT_ROWS], "context below");
        assert!(
            !visible[15 - CODE_CONTEXT_ROWS - 1],
            "beyond the context above"
        );
        assert!(
            !visible[15 + CODE_CONTEXT_ROWS + 1],
            "beyond the context below"
        );
        assert!(!visible[0] && !visible[29]);
    }

    /// On a pure deletion the after side has no changed row, only a caret; anchoring on
    /// `line_operations` alone would fold the whole panel away.
    #[test]
    fn anchor_rows_anchors_on_a_caret_with_no_changed_row() {
        use omnidiff::diff::text_range::TextRange;

        let panel = PanelRanges::from_tree(
            "a",
            "b",
            Vec::new(),
            &[RangeMatch {
                source: TextRange::new(3, 0, 5, 0),
                destination: TextRange::new(15, 0, 15, 0),
                operation: TextOperation::Delete,
            }],
            30,
            &[],
            1,
        );
        assert_eq!(
            panel.anchor_rows(),
            vec![15],
            "the caret's row is the panel's only anchor - it has no changed row of its own"
        );

        let visible = code_visible_rows(&panel.anchor_rows(), 30);

        assert!(visible[15], "the caret's own row must be visible");
        assert!(visible[15 - CODE_CONTEXT_ROWS] && visible[15 + CODE_CONTEXT_ROWS]);
        assert!(!visible[0] && !visible[29]);
    }

    #[test]
    fn code_visible_rows_shows_everything_when_nothing_anchors() {
        assert!(code_visible_rows(&[], 30).iter().all(|v| *v));
    }

    /// A painting draws no carets, so on a pure deletion its after panel anchors nowhere and would
    /// unfold the whole file unless it shares the tree panel's anchors.
    #[test]
    fn a_paintings_anchors_are_unioned_with_the_tree_panels_own() {
        use omnidiff::diff::text_range::TextRange;

        let tree = PanelRanges::from_tree(
            "a",
            "b",
            Vec::new(),
            &[RangeMatch {
                source: TextRange::new(3, 0, 5, 0),
                destination: TextRange::new(15, 0, 15, 0),
                operation: TextOperation::Delete,
            }],
            30,
            &[],
            1,
        );
        let painting = PanelRanges {
            side: "a0".to_string(),
            ranges: Vec::new(),
            ids: Vec::new(),
            counterparts: HashMap::new(),
            markers: Vec::new(),
            ops: vec![TextOperation::Identical; 30],
        };
        assert!(
            painting.anchor_rows().is_empty(),
            "a painting with nothing on this side anchors nowhere by itself"
        );

        let mut anchors = tree.anchor_rows();
        anchors.extend(painting.anchor_rows());
        let visible = code_visible_rows(&anchors, 30);

        assert!(visible[15], "the union keeps the tree panel's caret row");
        assert!(
            !visible[0] && !visible[29],
            "and it does not fall through to showing the whole file"
        );
    }

    #[test]
    fn code_counterparts_links_real_text_directly_and_a_deletion_to_its_caret() {
        use omnidiff::diff::text_range::TextRange;

        let before = vec![
            RangeMatch {
                source: TextRange::new(0, 0, 0, 3),
                destination: TextRange::new(5, 0, 5, 3),
                operation: TextOperation::Update,
            },
            // Delete: links to the caret `code_markers` draws.
            RangeMatch {
                source: TextRange::new(1, 0, 1, 3),
                destination: TextRange::new(9, 0, 9, 0),
                operation: TextOperation::Delete,
            },
        ];
        let after = vec![RangeMatch {
            source: TextRange::new(5, 0, 5, 3),
            destination: TextRange::new(0, 0, 0, 3),
            operation: TextOperation::Update,
        }];

        let after_ids = positional_ids("a", after.len());
        let links = code_counterparts(&before, &after, "a", &after_ids);

        assert_eq!(links.get(&0).map(String::as_str), Some("a0"));
        assert_eq!(links.get(&1).map(String::as_str), Some("am1"));
    }

    fn painting(entries: Vec<HumanTextEntry>) -> human_mapping::NamedTextMapping {
        human_mapping::NamedTextMapping {
            name: "Minimal".to_string(),
            mapping: human_mapping::HumanTextMapping { entries },
        }
    }

    fn span(
        start_row: usize,
        start_column: usize,
        end_row: usize,
        end_column: usize,
    ) -> HumanTextSpan {
        HumanTextSpan {
            start_row,
            start_column,
            end_row,
            end_column,
        }
    }

    /// An N:M match records no span-to-span pairing (see `HumanTextEntry`), so no pairing is
    /// invented: every span on a side shares one id and points at the other side's.
    #[test]
    fn painting_panels_give_every_span_of_one_match_a_single_shared_id() {
        // Three `foo` before, two after, all identical text.
        let before = "foo\nfoo\nfoo\n";
        let after = "foo\nfoo\n";
        let named = painting(vec![HumanTextEntry {
            operation: HumanTextOperation::Match,
            before: vec![span(0, 0, 0, 3), span(1, 0, 1, 3), span(2, 0, 2, 3)],
            after: vec![span(0, 0, 0, 3), span(1, 0, 1, 3)],
        }]);

        let [before_panel, after_panel] =
            painting_panels(&named, before, after, 0, [4, 3]).unwrap();

        assert_eq!(before_panel.ids, vec!["b0e0".to_string(); 3]);
        assert_eq!(after_panel.ids, vec!["a0e0".to_string(); 2]);
        for index in 0..3 {
            assert_eq!(
                before_panel.counterparts.get(&index).map(String::as_str),
                Some("a0e0"),
                "every before span points at the whole after side of the entry"
            );
        }
        for index in 0..2 {
            assert_eq!(
                after_panel.counterparts.get(&index).map(String::as_str),
                Some("b0e0")
            );
        }
        // Identical text on both sides is derived as a move, not an update.
        assert!(
            before_panel
                .ranges
                .iter()
                .all(|range| range.operation == TextOperation::Move)
        );
    }

    /// A painted `Delete` records where text was, not where its absence sits on the after side,
    /// so there is no caret to draw and nothing to link to.
    #[test]
    fn painting_panels_draw_no_caret_for_a_one_sided_entry() {
        let before = "keep\ngone\n";
        let after = "keep\n";
        let named = painting(vec![HumanTextEntry {
            operation: HumanTextOperation::Delete,
            before: vec![span(1, 0, 1, 4)],
            after: vec![],
        }]);

        let [before_panel, after_panel] =
            painting_panels(&named, before, after, 1, [3, 2]).unwrap();

        assert_eq!(before_panel.ranges.len(), 1);
        assert_eq!(before_panel.ranges[0].operation, TextOperation::Delete);
        assert!(
            before_panel.counterparts.is_empty(),
            "a delete has nothing on the other side to point at"
        );
        assert!(after_panel.ranges.is_empty() && after_panel.markers.is_empty());
        assert!(
            before_panel.markers.is_empty(),
            "painted panels never carry carets - only the tree projection computes those positions"
        );
    }

    /// `viewer.js` looks `data-range` ids up document-wide, so a shared prefix would let a click
    /// select spans in a hidden rendering.
    #[test]
    fn each_rendering_gets_its_own_dom_id_prefix() {
        let source = "foo\n";
        let tree = PanelRanges::from_tree("b", "a", Vec::new(), &[], 2, &[], 0);
        let named = painting(vec![HumanTextEntry {
            operation: HumanTextOperation::Delete,
            before: vec![span(0, 0, 0, 3)],
            after: vec![],
        }]);
        let [first, _] = painting_panels(&named, source, source, 0, [2, 2]).unwrap();
        let [second, _] = painting_panels(&named, source, source, 1, [2, 2]).unwrap();

        let sides = [tree.side.clone(), first.side.clone(), second.side.clone()];
        assert_eq!(sides, ["b".to_string(), "b0".to_string(), "b1".to_string()]);
        assert!(
            first.ids.iter().all(|id| id.starts_with("b0")),
            "got: {:?}",
            first.ids
        );
        assert!(second.ids.iter().all(|id| id.starts_with("b1")));
    }

    #[test]
    fn an_unreadable_painting_is_reported_and_skipped_rather_than_failing_the_page() {
        let source = "fn f() {}\n";
        let before = Code::from_string(source, &Language::Rust);
        let after = Code::from_string(source, &Language::Rust);
        let mapping = HumanMapping {
            entries: vec![],
            text_mappings: vec![painting(vec![HumanTextEntry {
                operation: HumanTextOperation::Delete,
                before: vec![span(99, 0, 99, 4)],
                after: vec![],
            }])],
            ..Default::default()
        };

        let mut warnings = Vec::new();
        let html = render_fixture_page(
            "rust-add-if",
            &before,
            &after,
            &mapping,
            None,
            None,
            &mut warnings,
        )
        .expect("a bad painting must not fail the page")
        .html;

        assert_eq!(warnings.len(), 1, "got: {warnings:?}");
        assert!(
            warnings[0].contains("rust-add-if") && warnings[0].contains("Minimal"),
            "the warning has to name the fixture and the painting: {warnings:?}"
        );
        assert!(
            !html.contains("painting-switch"),
            "with the only painting skipped there is one rendering left, and one rendering needs \
             no chooser"
        );
    }

    #[test]
    fn a_painted_fixture_page_stacks_one_code_panel_per_painting() {
        let source = "fn f() {}\n";
        let before = Code::from_string(source, &Language::Rust);
        let after = Code::from_string(source, &Language::Rust);
        let mut minimal = painting(vec![]);
        minimal.name = "Minimal".to_string();
        let mut full = painting(vec![]);
        full.name = "Full".to_string();
        let mapping = HumanMapping {
            entries: vec![],
            text_mappings: vec![minimal, full],
            ..Default::default()
        };

        let mut warnings = Vec::new();
        let html = render_fixture_page(
            "rust-add-if",
            &before,
            &after,
            &mapping,
            None,
            None,
            &mut warnings,
        )
        .expect("should render")
        .html;

        assert!(warnings.is_empty(), "got: {warnings:?}");
        for key in ["tree", "p0", "p1"] {
            assert!(
                html.contains(&format!(
                    r#"<div class="panels code-panels" data-painting="{key}""#
                )) || html.contains(&format!(
                    r#"<div class="panels code-panels hidden" data-painting="{key}""#
                )),
                "expected a code panel for '{key}': {html}"
            );
        }
        assert!(html.contains(r#"data-painting-name="Minimal""#));
        assert!(html.contains(r#"data-painting-name="Full""#));
        // Counts only `data-painting` buttons: the view switch has its own pressed button.
        assert!(html.contains(r#"data-painting="tree" data-painting-name="From the node mapping" aria-pressed="true""#), "got: {html}");
        for (key, name) in [("p0", "Minimal"), ("p1", "Full")] {
            assert!(
                html.contains(&format!(
                    r#"data-painting="{key}" data-painting-name="{name}" aria-pressed="false""#
                )),
                "expected an unpressed button for '{name}': {html}"
            );
        }
        assert_eq!(
            html.matches("code-panels hidden").count(),
            2,
            "the two paintings start hidden: {html}"
        );
    }

    #[test]
    fn escape_html_text_escapes_the_three_html_metacharacters_but_not_quotes() {
        assert_eq!(
            escape_html_text("a < b && c > d \"quoted\""),
            "a &lt; b &amp;&amp; c &gt; d \"quoted\""
        );
    }

    #[test]
    fn escape_html_attr_also_escapes_double_quotes() {
        assert_eq!(
            escape_html_attr("say \"hi\" <b>"),
            "say &quot;hi&quot; &lt;b&gt;"
        );
    }

    #[test]
    fn render_fixture_page_links_to_the_fixtures_directory_in_this_repo() {
        let source = "fn f() {}\n";
        let before = Code::from_string(source, &Language::Rust);
        let after = Code::from_string(source, &Language::Rust);
        let mapping = HumanMapping {
            entries: vec![],
            ..Default::default()
        };

        let html = render_fixture_page(
            "rust-add-if",
            &before,
            &after,
            &mapping,
            None,
            None,
            &mut Vec::new(),
        )
        .expect("should render")
        .html;

        assert!(
            html.contains(
                r#"href="https://github.com/ivankovic/omnidiff/tree/main/src/test/data/diffs/handmade/rust-add-if""#
            ),
            "expected a link straight to this fixture's own before/after files: {html}"
        );
    }

    #[test]
    fn render_fixture_page_links_to_the_upstream_commit_when_there_is_one() {
        let source = "fn f() {}\n";
        let before = Code::from_string(source, &Language::Rust);
        let after = Code::from_string(source, &Language::Rust);
        let mapping = HumanMapping::default();
        let upstream = Upstream {
            commit_url:
                "https://github.com/awslabs/aws-c-common/commit/fbb21230e117d2afe49d03ebe8605270dacb4ab3"
                    .to_string(),
            repository: "awslabs-aws-c-common".to_string(),
            commit: "fbb21230e117d2afe49d03ebe8605270dacb4ab3".to_string(),
            path: "include/aws/common/file.h".to_string(),
        };

        let html = render_fixture_page(
            "rust-add-if",
            &before,
            &after,
            &mapping,
            Some("Only insert, nothing else."),
            Some(&upstream),
            &mut Vec::new(),
        )
        .expect("should render")
        .html;

        assert!(
            html.contains(
                r#"href="https://github.com/awslabs/aws-c-common/commit/fbb21230e117d2afe49d03ebe8605270dacb4ab3""#
            ),
            "expected a link to the upstream commit: {html}"
        );
        assert!(
            html.contains(">Upstream commit awslabs-aws-c-common@fbb2123</a>"),
            "got: {html}"
        );
        assert!(
            html.contains(">include/aws/common/file.h</span>"),
            "got: {html}"
        );
        assert!(
            html.contains(r#"<p class="description">Only insert, nothing else.</p>"#),
            "the fixture's own note belongs on its page: {html}"
        );
    }

    #[test]
    fn render_fixture_page_has_no_upstream_link_without_provenance() {
        let source = "fn f() {}\n";
        let before = Code::from_string(source, &Language::Rust);
        let after = Code::from_string(source, &Language::Rust);

        let html = render_fixture_page(
            "rust-add-if",
            &before,
            &after,
            &HumanMapping::default(),
            None,
            None,
            &mut Vec::new(),
        )
        .expect("should render")
        .html;

        assert!(!html.contains("Upstream commit"), "got: {html}");
        assert!(!html.contains("class=\"description\""), "got: {html}");
    }

    /// One end of a cross-language pin: `assets/mapping_site/viewer.test.js` asserts that
    /// `viewer.js`'s `nodePath` produces this same string from the emitted HTML. The path goes into
    /// the "file an issue" body, where a wrong one fails silently.
    #[test]
    fn path_for_node_agrees_with_viewer_js_on_a_shared_example() {
        let source = "fn f() {\n    let a = 1;\n    let b = 2;\n}\n";
        let tree = parse_rust(source);

        // The *second* `let_declaration`, to exercise same-kind sibling counting.
        let node = helper::node_for_path(
            tree.root_node(),
            &[
                "function_item:1",
                "block:1",
                "let_declaration:2",
                "integer_literal:1",
            ],
        )
        .expect("the example path should resolve");

        assert_eq!(
            helper::path_for_node(node).join("/"),
            "function_item:1/block:1/let_declaration:2/integer_literal:1",
            "if this string changes, change it in viewer.test.js too - they pin each other"
        );
    }

    fn parse_rust(source: &str) -> tree_sitter::Tree {
        let language =
            omnidiff::code::language::to_treesitter(&omnidiff::code::Language::Rust).unwrap();
        let mut parser = tree_sitter::Parser::new();
        parser.set_language(&language).unwrap();
        parser.parse(source, None).unwrap()
    }

    #[test]
    fn render_node_emits_a_leaf_div_and_a_nonleaf_details_with_nested_children() {
        let source = "fn f() {}\n";
        let tree = parse_rust(source);
        let root = tree.root_node();
        let caches = Caches::default();

        let html = render_node(
            root,
            source.as_bytes(),
            'b',
            &caches,
            &[],
            &HashMap::new(),
            true,
        );

        assert!(
            html.starts_with(r#"<details class="node status-unmarked" id="b-"#),
            "root (non-leaf) should render as a <details>: {html}"
        );
        assert!(
            html.contains(r#"<div class="node leaf status-unmarked""#),
            "a leaf token (e.g. the 'fn' keyword) should render as a <div>: {html}"
        );
        assert!(html.contains(">fn \"fn\"<"), "leaf label missing: {html}");
        // Tags must balance on every recursion path.
        for tag in ["details", "summary", "div"] {
            let opens = html.matches(&format!("<{tag}")).count();
            let closes = html.matches(&format!("</{tag}>")).count();
            assert_eq!(
                opens, closes,
                "{tag}: {opens} opens vs {closes} closes in {html}"
            );
        }
    }

    #[test]
    fn render_node_marks_matched_nodes_with_a_data_match_pointing_at_the_other_side() {
        let source = "fn f() {}\n";
        let before_tree = parse_rust(source);
        let after_tree = parse_rust(source);
        let before_root = before_tree.root_node();
        let after_root = after_tree.root_node();

        let mapping = HumanMapping {
            entries: vec![HumanMappingEntry {
                operation: HumanOperation::Identical,
                before_path: Some(vec![]),
                after_path: Some(vec![]),
            }],
            ..Default::default()
        };
        let caches = rebuild_caches(&mapping.entries, before_root, after_root);

        let before_html = render_node(
            before_root,
            source.as_bytes(),
            'b',
            &caches,
            &[],
            &HashMap::new(),
            true,
        );

        let expected_id = format!("id=\"b-{}\"", before_root.id());
        let expected_match = format!("data-match=\"a-{}\"", after_root.id());
        assert!(
            before_html.contains(&expected_id),
            "expected {expected_id} in {before_html}"
        );
        assert!(
            before_html.contains("status-matched"),
            "matched root should get the matched status class: {before_html}"
        );
        assert!(
            before_html.contains(&expected_match),
            "expected {expected_match} in {before_html}"
        );
        assert!(
            !before_html.contains("changed"),
            "an Identical match must not get the changed class: {before_html}"
        );
    }

    #[test]
    fn render_node_marks_a_non_identical_matched_node_as_changed_with_its_own_operation_class() {
        let source = "fn f() {}\n";
        let before_tree = parse_rust(source);
        let after_tree = parse_rust(source);
        let before_root = before_tree.root_node();
        let after_root = after_tree.root_node();

        // (operation, an operation-specific class it must carry (empty if none), one it must not)
        for (operation, expected_class, unexpected_class) in [
            (HumanOperation::Update, "op-update", "op-moved"),
            (HumanOperation::MatchButNotIdentical, "", "op-update"),
        ] {
            let mapping = HumanMapping {
                entries: vec![HumanMappingEntry {
                    operation,
                    before_path: Some(vec![]),
                    after_path: Some(vec![]),
                }],
                ..Default::default()
            };
            let caches = rebuild_caches(&mapping.entries, before_root, after_root);

            let before_html = render_node(
                before_root,
                source.as_bytes(),
                'b',
                &caches,
                &[],
                &HashMap::new(),
                true,
            );

            assert!(
                before_html.contains("status-matched changed"),
                "{operation:?} should render matched *and* changed: {before_html}"
            );
            if !expected_class.is_empty() {
                assert!(
                    before_html.contains(expected_class),
                    "{operation:?} should get the {expected_class} class: {before_html}"
                );
            }
            assert!(
                !before_html.contains(unexpected_class),
                "{operation:?} should not get the {unexpected_class} class: {before_html}"
            );
        }
    }

    #[test]
    fn render_node_marks_an_identical_but_relocated_match_as_moved() {
        let source_a = "fn f() {\n    a();\n    b();\n}\n";
        let source_b = "fn f() {\n    b();\n    a();\n}\n";
        let before_tree = parse_rust(source_a);
        let after_tree = parse_rust(source_b);
        let before_root = before_tree.root_node();
        let after_root = after_tree.root_node();

        // `a();` is expression_statement:1 before, expression_statement:2 after (swapped with
        // `b();`) - same content, different position.
        let mapping = HumanMapping {
            entries: vec![HumanMappingEntry {
                operation: HumanOperation::Identical,
                before_path: Some(vec![
                    "function_item:1".to_string(),
                    "block:1".to_string(),
                    "expression_statement:1".to_string(),
                ]),
                after_path: Some(vec![
                    "function_item:1".to_string(),
                    "block:1".to_string(),
                    "expression_statement:2".to_string(),
                ]),
            }],
            ..Default::default()
        };
        let caches = rebuild_caches(&mapping.entries, before_root, after_root);

        let mut cursor = before_root.walk();
        let function_item = before_root.children(&mut cursor).next().unwrap();
        let mut c2 = function_item.walk();
        let block = function_item
            .children(&mut c2)
            .find(|n| n.kind() == "block")
            .unwrap();
        let mut c3 = block.walk();
        let call_statement = block
            .children(&mut c3)
            .find(|n| n.kind() == "expression_statement")
            .unwrap();

        let before_html = render_node(
            call_statement,
            source_a.as_bytes(),
            'b',
            &caches,
            &[],
            &HashMap::new(),
            true,
        );

        assert!(
            before_html.contains("op-moved"),
            "an Identical match at a different path should get the op-moved class: {before_html}"
        );
        assert!(
            !before_html.contains("changed"),
            "a moved-but-identical match is still content-identical, not changed: {before_html}"
        );
    }

    /// Every `expression_statement` reading `foo();`, in source order. Filtered by text because a
    /// statement-level `if` is an `expression_statement` too.
    fn foo_statements<'t>(root: Node<'t>, src: &[u8]) -> Vec<Node<'t>> {
        let mut found = Vec::new();
        let mut stack = vec![root];
        while let Some(node) = stack.pop() {
            if node.kind() == "expression_statement"
                && node.utf8_text(src).unwrap_or("").starts_with("foo")
            {
                found.push(node);
            }
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                stack.push(child);
            }
        }
        found.sort_by_key(|n| n.start_byte());
        found
    }

    /// One `foo();` before, two after, grouped all-to-all (the shape of a duplicated statement).
    fn duplicated_statement_mapping(
        before_root: Node,
        before_source: &str,
        after_root: Node,
        after_source: &str,
    ) -> HumanMapping {
        HumanMapping {
            groups: vec![human_mapping::MultiMapGroup {
                before_paths: foo_statements(before_root, before_source.as_bytes())
                    .iter()
                    .map(|n| helper::path_for_node(*n))
                    .collect(),
                after_paths: foo_statements(after_root, after_source.as_bytes())
                    .iter()
                    .map(|n| helper::path_for_node(*n))
                    .collect(),
                operation: HumanOperation::Identical,
                with_children: true,
                pairing: GroupPairing::AllToAll,
            }],
            ..Default::default()
        }
    }

    #[test]
    fn render_node_names_every_counterpart_of_an_all_to_all_member_and_badges_it() {
        let before_source = "fn main() {\n    foo();\n}\n";
        let after_source = "fn main() {\n    foo();\n    foo();\n}\n";
        let before_tree = parse_rust(before_source);
        let after_tree = parse_rust(after_source);
        let before_root = before_tree.root_node();
        let after_root = after_tree.root_node();
        let mapping =
            duplicated_statement_mapping(before_root, before_source, after_root, after_source);
        let caches = rebuild_caches_for_mapping(&mapping, before_root, after_root);
        let groups = resolve_groups(&mapping, before_root, after_root);
        assert_eq!((groups[0].before.len(), groups[0].after.len()), (1, 2));

        let before_html = render_node(
            before_root,
            before_source.as_bytes(),
            'b',
            &caches,
            &groups,
            &HashMap::new(),
            true,
        );
        let after_statements = foo_statements(after_root, after_source.as_bytes());
        // Both copies, not just the one the projection paired the original with.
        let expected_match = format!(
            r#"data-match="a-{} a-{}""#,
            after_statements[0].id(),
            after_statements[1].id()
        );
        assert!(
            before_html.contains(&expected_match),
            "expected {expected_match} in {before_html}"
        );
        assert!(
            before_html.contains(r#" group-all" id="b-"#),
            "the member carries the group class: {before_html}"
        );
        assert!(
            before_html.contains(r#"<span class="group-badge group-all" title="all-to-all group: each of the 1 before nodes corresponds to each of the 2 after nodes">all 1:2</span>"#),
            "{before_html}"
        );

        let after_html = render_node(
            after_root,
            after_source.as_bytes(),
            'a',
            &caches,
            &groups,
            &HashMap::new(),
            true,
        );
        let original = foo_statements(before_root, before_source.as_bytes())[0];
        // Each copy is matched (neither is the "leftover"), and both point back at the original.
        assert_eq!(
            after_html
                .matches(&format!(r#"data-match="b-{}""#, original.id()))
                .count(),
            2,
            "{after_html}"
        );
        assert!(!after_html.contains("status-inserted"), "{after_html}");
    }

    #[test]
    fn render_node_badges_an_any_one_to_one_member_without_changing_its_match() {
        let before_source = "fn main() {\n    foo();\n}\n";
        let after_source = "fn main() {\n    foo();\n    foo();\n}\n";
        let before_tree = parse_rust(before_source);
        let after_tree = parse_rust(after_source);
        let before_root = before_tree.root_node();
        let after_root = after_tree.root_node();
        let mut mapping =
            duplicated_statement_mapping(before_root, before_source, after_root, after_source);
        mapping.groups[0].pairing = GroupPairing::AnyOneToOne;
        let caches = rebuild_caches_for_mapping(&mapping, before_root, after_root);
        let groups = resolve_groups(&mapping, before_root, after_root);

        let before_html = render_node(
            before_root,
            before_source.as_bytes(),
            'b',
            &caches,
            &groups,
            &HashMap::new(),
            true,
        );
        assert!(before_html.contains(">any 1:2</span>"), "{before_html}");
        assert!(before_html.contains("group-any"), "{before_html}");
        // Still exactly one counterpart: the projection's pick, as before.
        let matches = before_html.matches("data-match=\"a-").count();
        assert_eq!(matches, 1, "{before_html}");
    }

    #[test]
    fn extents_cover_reads_a_range_against_the_union_of_a_groups_members() {
        // Statements on rows 3 and 4 (columns 8..14), and one on row 7.
        let members = [
            TextRange::new(3, 8, 3, 14),
            TextRange::new(4, 8, 4, 14),
            TextRange::new(7, 8, 7, 14),
        ];
        let covered = |range: TextRange| extents_cover(&members, &range);
        assert!(covered(TextRange::new(3, 8, 3, 14)), "one member exactly");
        assert!(covered(TextRange::new(3, 9, 3, 12)), "inside one member");
        assert!(
            covered(TextRange::new(3, 8, 4, 0)),
            "to the end of the member's line"
        );
        assert!(
            covered(TextRange::new(3, 8, 5, 0)),
            "two adjacent members merged"
        );
        assert!(
            !covered(TextRange::new(3, 7, 3, 14)),
            "starts before a member"
        );
        assert!(
            !covered(TextRange::new(3, 8, 6, 0)),
            "runs onto a line no member owns"
        );
        assert!(
            !covered(TextRange::new(4, 8, 8, 0)),
            "sweeps a stranger's row between members"
        );
        assert!(
            !covered(TextRange::new(5, 0, 6, 0)),
            "a row between members"
        );
        assert!(!covered(TextRange::new(3, 8, 3, 8)), "empty");
    }

    #[test]
    fn share_all_to_all_ids_gives_a_groups_moved_and_updated_spans_one_id_per_side() {
        let before_source = "fn main() {\n    foo();\n}\n";
        let after_source = "fn main() {\n    foo();\n    foo();\n}\n";
        let before_tree = parse_rust(before_source);
        let after_tree = parse_rust(after_source);
        let before_root = before_tree.root_node();
        let after_root = after_tree.root_node();
        let mapping =
            duplicated_statement_mapping(before_root, before_source, after_root, after_source);
        let groups = resolve_groups(&mapping, before_root, after_root);
        let statements = foo_statements(after_root, after_source.as_bytes());
        let extent = |n: Node| node_extent(n);

        let range = |source: TextRange, operation: TextOperation| RangeMatch {
            source,
            destination: TextRange::zero(),
            operation,
        };
        let mut ranges = vec![
            // The second copy, verbatim and in place: identical text, which is promoted to a
            // move so the copy is painted at all.
            range(extent(statements[1]), TextOperation::Identical),
            // A leaf inside the first copy, updated: inside a member.
            range(TextRange::new(1, 4, 1, 7), TextOperation::Update),
            // An insertion inside a member is not part of the correspondence.
            range(TextRange::new(1, 7, 1, 9), TextOperation::Insert),
            // A move outside every member.
            range(TextRange::new(0, 0, 0, 2), TextOperation::Move),
            // Identical text outside every member stays unpainted.
            range(TextRange::new(0, 3, 0, 7), TextOperation::Identical),
        ];
        let mut ids = positional_ids("a", ranges.len());
        let mut counterparts = HashMap::new();
        share_all_to_all_ids(
            &mut ranges,
            &mut ids,
            &mut counterparts,
            "a",
            "b",
            &groups,
            1,
        );

        assert_eq!(ids, vec!["aG0", "aG0", "a2", "a3", "a4"]);
        assert_eq!(ranges[0].operation, TextOperation::Move);
        assert_eq!(ranges[1].operation, TextOperation::Update);
        assert_eq!(ranges[4].operation, TextOperation::Identical);
        assert_eq!(counterparts.get(&0).map(String::as_str), Some("bG0"));
        assert_eq!(counterparts.get(&1).map(String::as_str), Some("bG0"));
        assert!(!counterparts.contains_key(&2));
        assert!(!counterparts.contains_key(&3));
        assert!(!counterparts.contains_key(&4));
    }

    #[test]
    fn a_page_with_an_all_to_all_group_links_the_copies_to_the_original_in_the_code_view() {
        // `TextDiff` calls both copies identical; the page still paints and links them.
        let before_source = "fn main() {\n    foo();\n}\n";
        let after_source = "fn main() {\n    foo();\n    foo();\n}\n";
        let before = Code::from_string(before_source, &Language::Rust);
        let after = Code::from_string(after_source, &Language::Rust);
        let before_root = before.ast.as_ref().unwrap().root_node();
        let after_root = after.ast.as_ref().unwrap().root_node();
        let mapping =
            duplicated_statement_mapping(before_root, before_source, after_root, after_source);

        let mut warnings = Vec::new();
        let page = render_fixture_page(
            "rust-duplicated",
            &before,
            &after,
            &mapping,
            None,
            None,
            &mut warnings,
        )
        .unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");
        assert!(page.html.contains("1 all-to-all group:"), "{}", page.html);
        assert!(page.html.contains(">all 1:2</span>"), "{}", page.html);
        // Group ids, not positional ones, so any span reveals the whole other side.
        assert_eq!(
            page.html
                .matches(r#"class="cd cd-move" data-range="aG0" data-counterpart="bG0""#)
                .count(),
            2,
            "{}",
            page.html
        );
        assert_eq!(
            page.html
                .matches(r#"class="cd cd-move" data-range="bG0" data-counterpart="aG0""#)
                .count(),
            1,
            "{}",
            page.html
        );
    }

    #[test]
    fn render_node_marks_a_deleted_node_without_a_data_match() {
        let source = "fn f() {}\n";
        let before_tree = parse_rust(source);
        let after_tree = parse_rust(source);
        let before_root = before_tree.root_node();
        let after_root = after_tree.root_node();

        let mapping = HumanMapping {
            entries: vec![HumanMappingEntry {
                operation: HumanOperation::DeleteWithChildren,
                before_path: Some(vec![]),
                after_path: None,
            }],
            ..Default::default()
        };
        let caches = rebuild_caches(&mapping.entries, before_root, after_root);

        let before_html = render_node(
            before_root,
            source.as_bytes(),
            'b',
            &caches,
            &[],
            &HashMap::new(),
            true,
        );

        assert!(
            before_html.contains("status-deleted"),
            "expected the deleted status class: {before_html}"
        );
        assert!(
            !before_html.contains("data-match"),
            "a deleted node has no counterpart to point at: {before_html}"
        );
    }

    #[test]
    fn fully_quiet_subtree_sizes_treats_matched_as_quiet_but_deleted_as_not() {
        let source = "fn main() {\n    a();\n    b();\n    c();\n}\n";
        let tree = parse_rust(source);
        let root = tree.root_node();
        let mut cursor = root.walk();
        let function_item = root.children(&mut cursor).next().unwrap();
        let block = {
            let mut c = function_item.walk();
            function_item
                .children(&mut c)
                .find(|n| n.kind() == "block")
                .unwrap()
        };
        let mut stmt_cursor = block.walk();
        let statements: Vec<Node> = block
            .children(&mut stmt_cursor)
            .filter(|n| n.kind() == "expression_statement")
            .collect();
        let stmt_a = statements[0];
        let stmt_c = statements[2];

        let mut caches = Caches::default();
        // `a();`'s call_expression is explicitly Matched - still quiet, nothing being edited.
        let mut c = stmt_a.walk();
        let call_expr_a = stmt_a.children(&mut c).next().unwrap();
        caches.before_match.insert(call_expr_a.id(), usize::MAX);
        // `c();`'s call_expression is explicitly Deleted - genuinely not quiet.
        let mut c2 = stmt_c.walk();
        let call_expr_c = stmt_c.children(&mut c2).next().unwrap();
        caches.before_removed.insert(call_expr_c.id(), false);

        let quiet = fully_quiet_subtree_sizes(root, &caches, status_before, is_identical_before);

        assert!(
            quiet.contains_key(&stmt_a.id()),
            "a(); has only a Matched descendant, which is quiet - not a live edit"
        );
        assert!(
            !quiet.contains_key(&stmt_c.id()),
            "c(); has a Deleted descendant, so it must not count as quiet"
        );
        assert!(
            !quiet.contains_key(&root.id()),
            "the root has a Deleted descendant somewhere, so it isn't quiet either"
        );
        assert!(
            quiet.contains_key(&statements[1].id()),
            "b(); has no marks anywhere in it (fully Unmarked), so it should still be quiet"
        );
    }

    #[test]
    fn fully_quiet_subtree_sizes_excludes_a_non_identical_match_even_though_its_matched() {
        // As the test above, but `a();`'s call_expression is a non-identical match: a real edit
        // must never be swallowed into a placeholder.
        let source = "fn main() {\n    a();\n    b();\n}\n";
        let tree = parse_rust(source);
        let root = tree.root_node();
        let mut cursor = root.walk();
        let function_item = root.children(&mut cursor).next().unwrap();
        let block = {
            let mut c = function_item.walk();
            function_item
                .children(&mut c)
                .find(|n| n.kind() == "block")
                .unwrap()
        };
        let mut stmt_cursor = block.walk();
        let statements: Vec<Node> = block
            .children(&mut stmt_cursor)
            .filter(|n| n.kind() == "expression_statement")
            .collect();
        let stmt_a = statements[0];

        let mut caches = Caches::default();
        let mut c = stmt_a.walk();
        let call_expr_a = stmt_a.children(&mut c).next().unwrap();
        caches.before_match.insert(call_expr_a.id(), usize::MAX);
        caches
            .before_operation
            .insert(call_expr_a.id(), HumanOperation::Update);

        let quiet = fully_quiet_subtree_sizes(root, &caches, status_before, is_identical_before);

        assert!(
            !quiet.contains_key(&call_expr_a.id()),
            "a non-identical match is a real edit, not quiet"
        );
        assert!(
            !quiet.contains_key(&stmt_a.id()),
            "a(); contains a non-identical match, so it isn't quiet either"
        );
        assert!(
            !quiet.contains_key(&root.id()),
            "the root contains a non-identical match somewhere, so it isn't quiet either"
        );
        assert!(
            quiet.contains_key(&statements[1].id()),
            "b(); has no marks anywhere in it (fully Unmarked), so it should still be quiet"
        );
    }

    #[test]
    fn render_node_keeps_the_root_open_even_when_the_whole_tree_is_fully_quiet() {
        let source = "fn main() {\n    a();\n    b();\n}\n";
        let tree = parse_rust(source);
        let root = tree.root_node();
        let caches = Caches::default(); // nothing marked anywhere
        let quiet_sizes =
            fully_quiet_subtree_sizes(root, &caches, status_before, is_identical_before);

        let html = render_node(
            root,
            source.as_bytes(),
            'b',
            &caches,
            &[],
            &quiet_sizes,
            true, // force_open: the root itself must stay open/rendered despite being fully unmarked
        );

        assert!(
            html.starts_with(r#"<details class="node status-unmarked" id="b-"#) && {
                let root_tag_end = html.find('>').unwrap();
                html[..root_tag_end].contains(" open")
            },
            "the root must stay open despite being fully unmarked (force_open): {html}"
        );
    }

    #[test]
    fn render_node_omits_a_large_fully_unmarked_subtree_behind_a_placeholder() {
        let source = "fn main() {\n    a();\n    b();\n}\n";
        let tree = parse_rust(source);
        let root = tree.root_node();
        let mut cursor = root.walk();
        let function_item = root.children(&mut cursor).next().unwrap();
        let caches = Caches::default(); // nothing marked anywhere
        let quiet_sizes =
            fully_quiet_subtree_sizes(root, &caches, status_before, is_identical_before);
        // The whole function body is one fully-unmarked subtree well past OMIT_THRESHOLD (20).
        let function_item_size = *quiet_sizes.get(&function_item.id()).unwrap();
        assert!(
            function_item_size > OMIT_THRESHOLD,
            "fixture assumption broken: function_item is only {function_item_size} nodes"
        );

        let html = render_node(
            root,
            source.as_bytes(),
            'b',
            &caches,
            &[],
            &quiet_sizes,
            true,
        );

        assert!(
            html.contains(&format!("+{function_item_size} nodes collapsed")),
            "expected an omission placeholder naming the subtree size: {html}"
        );
        assert!(
            !html.contains("fn \"fn\""),
            "an omitted subtree's leaf content must not be in the DOM at all: {html}"
        );
        assert_eq!(
            html.matches("<details").count(),
            1,
            "only the root's own <details> should remain -- function_item was omitted, not \
             nested: {html}"
        );
    }

    #[test]
    fn render_node_omits_a_large_fully_matched_subtree_but_keeps_its_own_data_match() {
        // An exhaustively matched subtree (e.g. generated code) is as quiet as an unannotated one.
        let source = "fn main() {\n    a();\n    b();\n}\n";
        let before_tree = parse_rust(source);
        let after_tree = parse_rust(source);
        let before_root = before_tree.root_node();
        let after_root = after_tree.root_node();
        let mut cursor = before_root.walk();
        let function_item = before_root.children(&mut cursor).next().unwrap();

        // Match every node one-for-one, as in `c-cpython-autogenerated-code`.
        fn match_everything(b: Node, a: Node, caches: &mut Caches) {
            caches.before_match.insert(b.id(), a.id());
            caches.after_match.insert(a.id(), b.id());
            let mut bc = b.walk();
            let mut ac = a.walk();
            for (bchild, achild) in b.children(&mut bc).zip(a.children(&mut ac)) {
                match_everything(bchild, achild, caches);
            }
        }
        let mut caches = Caches::default();
        match_everything(before_root, after_root, &mut caches);

        let quiet_sizes =
            fully_quiet_subtree_sizes(before_root, &caches, status_before, is_identical_before);
        let function_item_size = *quiet_sizes.get(&function_item.id()).unwrap();
        assert!(
            function_item_size > OMIT_THRESHOLD,
            "fixture assumption broken: function_item is only {function_item_size} nodes"
        );

        let html = render_node(
            before_root,
            source.as_bytes(),
            'b',
            &caches,
            &[],
            &quiet_sizes,
            true,
        );

        let expected_placeholder_prefix = format!(
            r#"<div class="node leaf status-matched placeholder" id="b-{}""#,
            function_item.id()
        );
        assert!(
            html.contains(&expected_placeholder_prefix),
            "expected a matched-status placeholder for function_item: {html}"
        );

        let after_function_item = {
            let mut c = after_root.walk();
            after_root.children(&mut c).next().unwrap()
        };
        assert!(
            html.contains(&format!("data-match=\"a-{}\"", after_function_item.id())),
            "a compressed matched subtree must still link to its counterpart: {html}"
        );
        assert!(
            html.contains(&format!("+{function_item_size} nodes collapsed")),
            "expected an omission placeholder naming the subtree size: {html}"
        );
        assert!(
            !html.contains("fn \"fn\""),
            "an omitted matched subtree's leaf content must not be in the DOM either: {html}"
        );
    }

    #[test]
    fn render_node_closes_but_still_fully_renders_a_small_fully_quiet_subtree() {
        // `parameters` is fully quiet and under OMIT_THRESHOLD; `function_item` is not quiet.
        let source = "fn main() {\n    a();\n}\n";
        let before_tree = parse_rust(source);
        let after_tree = parse_rust(source);
        let before_root = before_tree.root_node();
        let after_root = after_tree.root_node();
        let mut c = before_root.walk();
        let function_item = before_root.children(&mut c).next().unwrap();
        let mut c2 = function_item.walk();
        let block = function_item
            .children(&mut c2)
            .find(|n| n.kind() == "block")
            .unwrap();

        let mapping = HumanMapping {
            entries: vec![HumanMappingEntry {
                operation: HumanOperation::MatchButNotIdentical,
                before_path: Some(omnidiff::test::helper::path_for_node(block)),
                after_path: Some(omnidiff::test::helper::path_for_node(block)),
            }],
            ..Default::default()
        };
        let caches = rebuild_caches(&mapping.entries, before_root, after_root);
        let quiet_sizes =
            fully_quiet_subtree_sizes(before_root, &caches, status_before, is_identical_before);

        let html = render_node(
            before_root,
            source.as_bytes(),
            'b',
            &caches,
            &[],
            &quiet_sizes,
            true,
        );

        assert!(
            html.contains(r#"data-kind="parameters""#),
            "parameters should still be present, not omitted: {html}"
        );
        let tag_start = html.find(r#"data-kind="parameters""#).unwrap();
        let details_start = html[..tag_start].rfind("<details").unwrap();
        let details_tag_end = html[details_start..].find('>').unwrap() + details_start;
        assert!(
            !html[details_start..details_tag_end].contains(" open"),
            "a small fully-unmarked subtree should start collapsed: {}",
            &html[details_start..details_tag_end]
        );
        assert!(
            html.contains(r#"data-kind="(""#) && html.contains(r#"data-kind=")""#),
            "parameters' own children must still be fully rendered, just closed: {html}"
        );
    }

    #[test]
    fn render_index_page_links_to_each_fixtures_page_and_shows_its_language() {
        let entries = vec![
            IndexEntry {
                name: "rust-add-if".to_string(),
                language: Language::Rust,
                omnidiff_mismatches: 0,
                unix_diff_mismatches: 3,
                total_lines: 40,
                paintings: vec!["Minimal".to_string(), "Full".to_string()],
                note: Some("Requires a N:M match for perfect solution".to_string()),
                unmarked_nodes: 0,
                revision: "0123456789abcdef".to_string(),
            },
            IndexEntry {
                name: "c-linux-small-bugfix".to_string(),
                language: Language::C,
                omnidiff_mismatches: 1,
                unix_diff_mismatches: 5,
                total_lines: 12,
                paintings: Vec::new(),
                note: None,
                unmarked_nodes: 7,
                revision: "0123456789abcdef".to_string(),
            },
        ];

        let html = render_index_page(&entries);

        assert!(html.contains(r#"href="fixtures/rust-add-if.html""#));
        assert!(html.contains(">rust-add-if<"));
        assert!(html.contains(">Rust<"));
        assert!(html.contains(r#"href="fixtures/c-linux-small-bugfix.html""#));
        assert!(html.contains(">C<"));
        assert!(
            html.contains(">Minimal, Full</td>"),
            "expected a painted fixture to list its paintings by name: {html}"
        );
        assert!(
            html.contains(">&mdash;</td>"),
            "expected an unpainted fixture to read as absent: {html}"
        );
        assert!(
            html.contains(
                r#"<div class="fixture-note">Requires a N:M match for perfect solution</div>"#
            ),
            "expected the fixture's description under its name: {html}"
        );
        assert!(
            html.contains(r#"data-unmarked="7""#) && html.contains(">7</td>"),
            "expected the unfinished mapping's unmarked-node count: {html}"
        );
    }

    #[test]
    fn render_index_page_puts_each_mismatch_count_in_its_own_sortable_column() {
        let entries = vec![IndexEntry {
            name: "rust-add-if".to_string(),
            language: Language::Rust,
            omnidiff_mismatches: 2,
            unix_diff_mismatches: 9,
            total_lines: 40,
            paintings: vec!["Only one solution".to_string()],
            note: None,
            unmarked_nodes: 4,
            revision: "0123456789abcdef".to_string(),
        }];

        let html = render_index_page(&entries);

        assert!(
            html.contains(r#"data-omnidiff="2""#),
            "expected the row to carry omnidiff's mismatch count as a data attribute for the \
             sort script to read: {html}"
        );
        assert!(
            html.contains(r#"data-unix_diff="9""#),
            "expected the row to carry Unix diff's mismatch count as a data attribute: {html}"
        );
        assert!(
            html.contains(r#"data-total_lines="40""#),
            "expected the row to carry the total line count as a data attribute: {html}"
        );
        assert!(
            html.contains(">2</td>"),
            "omnidiff's count should render as a cell: {html}"
        );
        assert!(
            html.contains(">9</td>"),
            "Unix diff's count should render as a cell: {html}"
        );
        assert!(
            html.contains(r#"data-paintings="1""#),
            "expected the row to carry how many paintings the fixture has: {html}"
        );
        assert!(
            html.contains(r#"data-unmarked="4""#),
            "expected the row to carry how much of the mapping is still unmarked: {html}"
        );
        for sort_key in [
            "name",
            "language",
            "omnidiff",
            "unix_diff",
            "total_lines",
            "paintings",
            "unmarked",
        ] {
            assert!(
                html.contains(&format!(r#"data-sort="{sort_key}""#)),
                "expected a sortable column header for {sort_key}: {html}"
            );
        }
        assert!(html.contains(r#"src="assets/index.js""#));
    }

    #[test]
    fn render_index_page_escapes_fixture_names() {
        let entries = vec![IndexEntry {
            name: "a&b".to_string(),
            language: Language::Unknown,
            omnidiff_mismatches: 0,
            unix_diff_mismatches: 0,
            total_lines: 0,
            paintings: Vec::new(),
            note: None,
            unmarked_nodes: 0,
            revision: "0123456789abcdef".to_string(),
        }];
        let html = render_index_page(&entries);
        assert!(html.contains("a&amp;b"));
        assert!(!html.contains("a&b<"));
    }

    #[test]
    fn render_index_page_has_a_reviewed_column_carrying_each_rows_revision() {
        let entries = vec![IndexEntry {
            name: "rust-add-if".to_string(),
            language: Language::Rust,
            omnidiff_mismatches: 0,
            unix_diff_mismatches: 0,
            total_lines: 0,
            paintings: Vec::new(),
            note: None,
            unmarked_nodes: 0,
            revision: "00ff00ff00ff00ff".to_string(),
        }];
        let html = render_index_page(&entries);

        assert!(
            html.contains(r#"data-revision="00ff00ff00ff00ff" data-reviewed="0">"#),
            "the row must carry the revision reviewed.js compares marks against: {html}"
        );
        assert!(html.contains(r#"<th data-sort="reviewed" data-type="number""#));
        assert!(html.contains(
            r#"<input type="checkbox" class="reviewed-mark" aria-label="Reviewed: rust-add-if">"#
        ));
        assert!(html.contains(
            r#"<button id="random-unreviewed" type="button" data-fixtures-dir="fixtures/">"#
        ));
        assert!(html.contains(r#"<script src="assets/reviewed.js"></script>"#));
    }

    #[test]
    fn render_fixtures_script_lists_every_fixture_with_its_revision() {
        let entry = |name: &str, revision: &str| IndexEntry {
            name: name.to_string(),
            language: Language::Rust,
            omnidiff_mismatches: 0,
            unix_diff_mismatches: 0,
            total_lines: 0,
            paintings: Vec::new(),
            note: None,
            unmarked_nodes: 0,
            revision: revision.to_string(),
        };
        let script = render_fixtures_script(&[
            entry("rust-add-if", "0000000000000001"),
            // A quote in a name is not something the corpus has, but the script must stay valid JS
            // if it ever does.
            entry("odd\"name", "0000000000000002"),
        ]);

        assert!(script.starts_with("// Generated by generate_mapping_site.rs"));
        assert!(script.contains("window.OMNIDIFF_FIXTURES = ["));
        assert!(script.contains(r#"{name:"rust-add-if",revision:"0000000000000001"}"#));
        assert!(script.contains(r#"{name:"odd\"name",revision:"0000000000000002"}"#));
    }

    #[test]
    fn fixture_revision_follows_the_mapping_and_the_sources_but_not_json_formatting() {
        let before = Code::from_string("fn f() {}\n", &Language::Rust);
        let after = Code::from_string("fn g() {}\n", &Language::Rust);
        let empty = HumanMapping::default();
        let base = fixture_revision(&empty, &before, &after).expect("hashes");

        assert_eq!(base.len(), 16, "one u64 as hex: {base}");
        assert_eq!(
            base,
            fixture_revision(&empty, &before, &after).expect("hashes"),
            "deterministic"
        );

        // Reformatting the file changes nothing: the hash is over the mapping as a value, so it
        // is what `load` would see, not the bytes on disk.
        let reparsed: HumanMapping =
            serde_json::from_str(&serde_json::to_string_pretty(&empty).expect("serializes"))
                .expect("parses");
        assert_eq!(
            base,
            fixture_revision(&reparsed, &before, &after).expect("hashes")
        );

        let mut painted = HumanMapping::default();
        painted.text_mappings.push(painting(Vec::new()));
        assert_ne!(
            base,
            fixture_revision(&painted, &before, &after).expect("hashes"),
            "a painting"
        );

        let other_after = Code::from_string("fn h() {}\n", &Language::Rust);
        assert_ne!(
            base,
            fixture_revision(&empty, &before, &other_after).expect("hashes"),
            "a source"
        );
        assert_ne!(
            base,
            fixture_revision(&empty, &after, &before).expect("hashes"),
            "swapping the sides"
        );
    }

    #[test]
    fn render_fixture_page_bakes_its_revision_and_the_review_controls() {
        let before = Code::from_string("fn f() {}\n", &Language::Rust);
        let after = Code::from_string("fn f() {}\n", &Language::Rust);
        let mapping = HumanMapping::default();

        let page = render_fixture_page(
            "rust-add-if",
            &before,
            &after,
            &mapping,
            None,
            None,
            &mut Vec::new(),
        )
        .expect("should render");

        assert_eq!(
            page.revision,
            fixture_revision(&mapping, &before, &after).expect("hashes")
        );
        assert!(
            page.html.contains(&format!(
                r#"<body data-fixture="rust-add-if" data-repo="ivankovic/omnidiff" data-revision="{}">"#,
                page.revision
            )),
            "the page must carry the same revision the index row does: {}",
            page.html
        );
        assert!(
            page.html
                .contains(r#"<button id="toggle-reviewed" type="button" aria-pressed="false""#)
        );
        assert!(
            page.html
                .contains(r#"<button id="random-unreviewed" type="button" data-fixtures-dir="">"#)
        );
        // fixtures.js before reviewed.js: the latter reads the list the former defines.
        let fixtures_at = page
            .html
            .find(r#"<script src="../assets/fixtures.js"></script>"#)
            .expect("fixtures.js");
        let reviewed_at = page
            .html
            .find(r#"<script src="../assets/reviewed.js"></script>"#)
            .expect("reviewed.js");
        assert!(fixtures_at < reviewed_at);
        assert!(
            page.html
                .contains("<dt>r</dt><dd>mark this fixture as reviewed")
        );
        assert!(
            page.html
                .contains("<dt>n</dt><dd>open a random fixture not yet marked as reviewed")
        );
    }
}
