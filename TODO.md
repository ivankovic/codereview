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

## From the security review (2026-09-16)

Fixed already: two panics reachable from a request that poisoned a repository's lock, a
symlink escape from the working tree, a dangling-symlink escape and `.git` writes from the
agent, truncated permission prompts, secrets inherited by the agent, newline injection into
REVIEW.md, unbounded log pages and search results, an unvalidated layout, and prototype
pollution in the page. What is left:

- Run the agent hub's work under `spawn_blocking` as `with_session` does: today
  `/api/agent/*` writes to a child process's pipe on a runtime thread, so an agent that
  stops reading stalls a worker while holding the hub lock.
- Put a concurrency limit on `/api`. Every request for a repository queues on one mutex, so
  a few slow ones (a large diff, a wide search) make the rest wait with no bound on how many
  blocking threads pile up.
- Cache the line text in the symbol index: `references` re-reads every matching file from
  disk on every request, and `search` lowercases every name again each time.
- Kill the agent's process group, not just the child: an `npx` wrapper leaves its `node`
  grandchild running, holding the pipes.
- Resolve agent paths by opening rather than by name: `resolve_in` checks a path and then
  reopens it, so a component swapped in between is followed. `openat` with `O_NOFOLLOW`
  relative to a directory handle would close the window.
- Bound the transcript the web hub keeps, in bytes as well as entries.
- Snap the page's byte cuts to character boundaries in `renderSegments`, so a diff span
  inside a multi-byte character renders it rather than a replacement character.
- Say in the config file documentation that `claude_args` is appended after the permission
  flags, so a `--permission-mode` there decides what the agent asks about.
- Pin the CI actions by commit, not by tag.
- Set an explicit body limit rather than relying on axum's default.

