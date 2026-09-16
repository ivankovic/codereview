# Plan

codereview is a code exploration and review tool for codebases that change quickly, typically
because a large share of the code is written with AI assistance. A reviewer needs three things:
to read the current state of the tree and leave comments, to see how each file got to where it
is, and to walk the repository's history as a whole. The tool serves those three uses through a
terminal UI (Ratatui) and a browser UI that share one core.

Comments and notes live in two Markdown files at the repository root, `REVIEW.md` and
`NOTES.md`, in a form that reads fine in any editor or on a forge, and that an AI agent can read
and act on without this tool. `REVIEW.md` is the actionable list, pending and completed; it is
the same format `nvim-review` writes, so the two tools can be used on the same file. `NOTES.md`
holds observations that are not tasks: what a module is for, what to watch out for, decisions
made while reviewing.

Diffs come from the `codediff` crate: syntax-aware, tree-sitter based, reporting insertions,
deletions, updates and moves rather than whichever lines happened to align. Files codediff has no
grammar for (Makefiles, for one) fall back to its plain-text line diff.

## Decisions

- **One crate, feature-gated front ends.** `tui` is on by default; `web` is opt-in. Same layout
  as codediff, for the same reasons: one `cargo install` gives the working binary, and a library
  consumer can take only the core.
- **Git through the `git` binary**, not libgit2 or gitoxide. codediff's own review module made
  the same choice: nothing here needs more than `status`, `ls-files`, `log`, `show` and
  `diff-tree`, and shelling out costs no build-time dependency and matches whatever git the user
  has configured (worktrees, sparse checkouts, credential helpers).
- **Languages.** Rust, Python, TypeScript, HTML and Makefile first. codediff covers the first
  four structurally; Makefile gets the line diff plus syntect highlighting. Everything else
  codediff and syntect know works too, since nothing is special-cased per language.
- **Stale comments are expected, not an error.** Every comment records the text of the line it
  was left on. When a file changes, a comment whose line no longer matches is re-anchored by
  searching for that text; if it is found once, the comment follows it, otherwise it is shown as
  stale. `codereview reanchor` rewrites `REVIEW.md` with the new line numbers.
- **The browser UI is a local server**, bound to loopback with a per-session token, serving a
  single embedded page. No build step, no node toolchain. Same approach as `codediff-web`.
- **AGPL-3.0-or-later**, like codediff.

## Phases

Phases 1 to 3 are built (2026-09-10), then colour schemes, symbol navigation and the ACP
agent (2026-09-11), Claude Code driven directly, the agent tab on the page, and several
repositories in one process (2026-09-16). Phase 4 is open; TODO.md tracks it.

1. **Core** (`src/repo.rs`, `src/review.rs`, `src/notes.rs`, `src/anchor.rs`, `src/diff.rs`,
   `src/highlight.rs`): git access, the two Markdown files with round-trip parsing, comment
   re-anchoring, the codediff wrapper, and UI-independent syntax highlighting. Unit tests for
   the parsers against a scratch git repository. CLI subcommands `list`, `add`, `reanchor` so
   the core is usable from scripts and by agents before any UI exists.
2. **TUI**: file tree and viewer with comment gutter; add, edit, complete and delete comments;
   file notes; per-file history with diffs; repository log with per-commit file lists and
   diffs; working-tree and staged diffs; review panel listing every comment with jump-to.
3. **Web UI**: the same views over a JSON API, in one embedded page.
4. **Later**: blame-to-commit jumps; comment
   threads (replies) if the Markdown format can carry them without hurting readability;
   multi-file review sessions ("review this commit range") with progress tracking.

## Layout

```
src/main.rs        CLI (clap): tui (default), web, list, add, note, toggle, reanchor, def,
                   refs, symbols, agent
src/lib.rs
src/session.rs     one repository for the UIs: files, comments, notes, config, symbol index
src/repo.rs        git: root, status, file list, log, file log, blob at revision, commit files
src/review.rs      REVIEW.md: Comment model, parse, write
src/notes.rs       NOTES.md: Note model, parse, write
src/anchor.rs      re-anchoring comments after edits
src/diff.rs        codediff wrapper, per-line paint model shared by both UIs
src/highlight.rs   syntect highlighting into UI-independent segments
src/theme.rs       colour schemes shared by both front ends
src/config.rs      the config file
src/symbols.rs     tree-sitter symbol index: definitions, occurrences, search
src/acp.rs         Agent Client Protocol client
src/claude.rs      Claude Code driven through its own streaming protocol
src/agent.rs       the agent as the front ends see it, either backend
src/fakes.rs       in-process stand-ins for both backends, tests only
src/tui.rs, src/tui/  Ratatui front end (feature `tui`); workspace.rs holds one app per repository
src/web.rs, src/web/  axum server and the embedded page (feature `web`)
```
