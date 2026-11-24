# TODO

## One frame, four focuses (PLAN.md phase 7)

- Terminal UI: Docs rendered rather than shown as source; CSV shown as a table; a forge's
  comments on the lines of a pull request's diff, not only in its list.
- Browser UI: Docs rendered rather than shown as source; CSV shown as a table; a forge's
  comments on the lines of a range's diff.

## Navigation (SPECS.md, Navigation)

- The search offering pull requests, branches and worktrees as ranges to open, so `/` answers
  what `o` does too.
- Browser UI: a search result in the open diff marks the query but does not scroll to its line.
- Terminal UI: a list of the trail, to go back to any point of it rather than one step at a time.
- The search and the jump menu in a diff and in omnidiff's view, not only in the file viewer.
- Language-server backend for symbol navigation, for repositories that have one running.

## Other

- Agent: turn a review the agent writes into REVIEW.md comments automatically when it answers
  in prose instead.
- Agent: picking the agent's mode.
- Author name override in the config file (today it is always git's user.name).
- Comment replies, if a readable Markdown form for them exists.

## From the code health review (2026-09-16)

All of it is done except one, deliberately:

- **No unit tests for the page's own helpers.** `renderSegments`, `searchRangesFor` and the
  wrapping are pure functions worth testing, but the page is one inline script on purpose:
  it has no module boundaries to import across, and no build step to add them. Testing them
  would mean either splitting the script, which the content security policy and the
  single-file deployment both argue against, or bringing a JavaScript toolchain into a
  repository that deliberately has none. They are exercised end to end instead, by the
  headless browser and by the integration test.

## From the security review (2026-09-16)

All of it is done except one, deliberately:

- **Not resolving agent paths with `openat`.** `resolve_in` checks a path and then opens it
  by name, so a component swapped for a symlink in between would be followed. Closing that
  window properly means walking every component with `O_NOFOLLOW` against a directory
  handle, which is sixty lines of raw syscalls inside the security boundary itself. The only
  agent it would protect is one with no way to run commands, since any agent that can create
  a symlink mid-race can also write outside the repository directly; every static case is
  already refused. The bug risk of the fix is larger than the window it closes, so it stays
  here rather than in the code.
