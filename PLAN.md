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

Diffs come from the `omnidiff` crate: syntax-aware, tree-sitter based, reporting insertions,
deletions, updates and moves rather than whichever lines happened to align. Files omnidiff has no
grammar for (Makefiles, for one) fall back to its plain-text line diff.

## Decisions

- **One crate, feature-gated front ends.** `tui` is on by default; `web` is opt-in. Same layout
  as omnidiff, for the same reasons: one `cargo install` gives the working binary, and a library
  consumer can take only the core.
- **Git through the `git` binary**, not libgit2 or gitoxide. omnidiff's own review module made
  the same choice: nothing here needs more than `status`, `ls-files`, `log`, `show` and
  `diff-tree`, and shelling out costs no build-time dependency and matches whatever git the user
  has configured (worktrees, sparse checkouts, credential helpers).
- **Languages.** Rust, Python, TypeScript, HTML and Makefile first. omnidiff covers the first
  four structurally; Makefile gets the line diff plus syntect highlighting. Everything else
  omnidiff and syntect know works too, since nothing is special-cased per language.
- **Stale comments are expected, not an error.** Every comment records the text of the line it
  was left on. When a file changes, a comment whose line no longer matches is re-anchored by
  searching for that text; if it is found once, the comment follows it, otherwise it is shown as
  stale. `codereview reanchor` rewrites `REVIEW.md` with the new line numbers.
- **The browser UI is a local server**, bound to loopback with a per-session token, serving a
  single embedded page. No build step, no node toolchain. Same approach as `omnidiff-web`.
- **The server keeps no state of its own.** What is worth remembering goes in `REVIEW.md` and
  `NOTES.md`, where any editor can read it; what is merely where you are — the open file, the
  tab, the scroll, a review half worked through — belongs to the browser that is looking, in
  its address and its own storage. This is why there is no database, no per-user store, and no
  progress tracking on the server: two people reviewing the same repository share the files and
  nothing else, and a restart costs nobody anything they had not already written down. An agent
  transcript is the one thing the server builds and holds, because both front ends have to draw
  the same one, and it is deliberately not kept past a restart. Signing in is the price of that
  rule and is paid knowingly: a session lives in memory, so a restart ends every one of them.
  Keeping them would mean either credentials on disk or a cookie the server can no longer take
  back, and neither is worth a re-typed password after a deploy.
- **A run is a process; its result is a file.** The tool can run the checks a project already
  has — tests, coverage, a linter, `make check` — and show what came back. Nothing about a run
  is kept in the server: while it runs it is live progress, the same deliberate exception the
  agent transcript is, and what outlives it is whatever the command itself wrote, a
  `junit.xml` or an `lcov.info` or an exit code. The page reads those when asked. So two
  browsers see the same numbers because they are looking at the same files, a restart loses
  nothing that was not already on disk, and the rule above survives a feature that would
  otherwise have needed a database.
- **AGPL-3.0-or-later**, like omnidiff.

## Phases

Phases 1 to 3 are built (2026-09-10), then colour schemes, symbol navigation and the ACP
agent (2026-09-11), Claude Code driven directly, the agent tab on the page, and several
repositories in one process (2026-09-16), committing from the page, the status view and
phase 5 (2026-09-17). Phase 4 is open; TODO.md tracks it.

1. **Core** (`src/repo.rs`, `src/review.rs`, `src/notes.rs`, `src/anchor.rs`, `src/diff.rs`,
   `src/highlight.rs`): git access, the two Markdown files with round-trip parsing, comment
   re-anchoring, the omnidiff wrapper, and UI-independent syntax highlighting. Unit tests for
   the parsers against a scratch git repository. CLI subcommands `list`, `add`, `reanchor` so
   the core is usable from scripts and by agents before any UI exists.
2. **TUI**: file tree and viewer with comment gutter; add, edit, complete and delete comments;
   file notes; per-file history with diffs; repository log with per-commit file lists and
   diffs; working-tree and staged diffs; review panel listing every comment with jump-to.
3. **Web UI**: the same views over a JSON API, in one embedded page.
4. **Later**: blame-to-commit jumps; comment threads (replies) if the Markdown format can
   carry them without hurting readability.
5. **Analysis** (`src/analysis.rs`, used by both front ends): the checks a project already has,
   run from the tool and read back as figures worth a glance. A `[[analysis]]` entry in the
   configuration names one — a label, a command, and optionally the artefact it leaves behind
   and how to read it. Three steps, in this order, because the first two need no new state and
   the third is where the question gets hard:
   - **On demand.** A button per entry, in the status view and beside a diff. The command runs
     from the repository root, its output streams to the page the way an agent turn does, and
     what comes back is an exit code, a duration and the tail of what it printed. That much is
     useful for anything: `make check`, a linter, a type checker, a build.
   - **Numbers.** Where an entry names an artefact, it is parsed for what belongs on a
     dashboard: `junit.xml` for tests run, failed and skipped, `lcov.info` for line coverage.
     Read on request and never stored, and shown with the artefact's own modification time, so
     a stale number says how stale it is instead of pretending.
   - **On its own.** A tick per repository computes the same fingerprint the page polls; when
     it settles — a few seconds of quiet, so that saving a file in an editor is not a build —
     the entries marked for it run. One run per repository at a time, a global limit well
     below the number of repositories served, and a floor on how often any one may be built.
     Being run without anyone asking is opt-in per repository: a command that runs on its own,
     in a repository that was cloned rather than written, is code execution from a clone. The
     commands come from the user's own configuration file for that reason, and a per-repository
     file would have to be trusted first, the way git trusts a directory's ownership.

   Both of the questions this left open answered themselves in the building. There is no
   record of a run on disk, because there did not need to be one: the artefact is the record,
   and a run whose figures outlive it has already written them down. And the tick runs whether
   or not a browser is connected, since a check that waits for someone to look is not proactive
   at all; what keeps a laptop quiet is that nothing is watched until a check names it.

## Layout

```
src/main.rs        CLI (clap): tui (default), web, list, add, note, toggle, reanchor, def,
                   refs, symbols, agent
src/lib.rs
src/session.rs     one repository for the UIs: files, comments, notes, config, symbol index
src/repo.rs        git: root, status, file list, log, file log, blob at revision, commit files
src/review.rs      REVIEW.md: Comment model, parse, write
src/notes.rs       NOTES.md: Note model, parse, write
src/overview.rs    what a repository looks like when you open it: its own words, where to
                   start, and how it is doing
src/anchor.rs      re-anchoring comments after edits
src/diff.rs        omnidiff wrapper, per-line paint model shared by both UIs
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
