# TODO

- Jump from a blame line to that commit's diff.
- Language-server backend for symbol navigation, for repositories that have one running.
- Agent: turn a review the agent writes into REVIEW.md comments automatically when it answers
  in prose instead.
- Agent: sessions that survive a restart (`session/load`), and picking the agent's mode.
- Author name override in the config file (today it is always git's user.name).
- Comment replies, if a readable Markdown form for them exists.
- Review sessions over a commit range with progress tracking.

## From the code health review (2026-09-16)

- TUI: extract the agent panel (state, events, keys, drawing) out of `App` and `ui.rs`; one
  list-navigation helper for the six copies; rewrite `handle_log_key` without the
  pop-and-push dance.
- TUI: `r` should keep the tree's folds and filter; Esc on the filter prompt should restore
  the previous filter; a status message should not be hidden while an agent runs.
- Backends: move the shared plumbing (start-up diagnostics, timed wait, poll, send) onto
  `Transport`; split `claude::handle_message` per message type; detect Claude Code's
  cancellation from `aborted: true` rather than from the message text; resolve the `auto`
  kind in one place instead of three.
- Web: run hub work under `spawn_blocking`, close the double-start race with a starting
  flag, take a permission only once the answer was sent, map internal errors to 500; build
  transcript entries on the server so the page stops repeating the TUI's state machine.
- Page: give a diff opened from the explorer its own view, so the keys and the search work;
  factor out the repeated templates and query builders; report polling failures.
- Core: normalise comment text on construction so REVIEW.md round-trips (newlines, a
  trailing quote); bucket diff ranges by row (`spans_for_line` is lines times ranges);
  intern identifier names in the symbol index; run git with `LC_ALL=C` so `show` can tell a
  missing file from an error in any locale; validate `layout` and an unknown theme name
  instead of falling back silently; derive `Session.timestamps` and `theme` from `config`.
- Dead code: `NotesFile::rename_target`, `ReviewFile::comments_mut` and `sections`,
  `highlight::theme_names` and `theme_background`, `Theme::names`, `DiffView::before_line`
  and `focus_after`, `PromptKind::ConfirmDelete`, the four `hint` stubs in `ui.rs`.
- Tests: `main.rs` has none; `apply_agent_event`, `DiffView`, log paging, `Config` file I/O,
  `diff::anchors` with duplicate lines, a `fake-claude` round trip through the web API, a
  request with a wrong token; a few unit tests for the pure helpers in the page.

