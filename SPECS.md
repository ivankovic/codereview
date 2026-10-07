# Specification

What codereview does, as built. PLAN.md has the reasoning and the roadmap; this file has the
behaviour. No code here.

## Repository access

Everything git-related runs the `git` binary from the repository root, found with
`rev-parse --show-toplevel` from the directory given (`-C`) or the current one. Used commands:
`ls-files` (tracked plus untracked, ignored excluded), `status --porcelain=v1 -z`, `log` with a
custom record format (`--follow` when limited to one file), `diff-tree` for a commit's files,
`diff --name-status` between two revisions, `show <rev>:<path>` for file content at a revision
(`:<path>` for the index), `blame --porcelain`, `config user.name` for the author, and, for the
one thing the tool writes to the repository itself, `add -A` and `commit -m`. git is run with no
terminal, so anything that would ask a question — a passphrase for a signature, a credential —
fails rather than waiting for an answer that cannot come.

A repository without commits works: the log is empty, every file counts as added, and diffs
compare against nothing.

## REVIEW.md

Sections are level-one headings. `Pending` and `Completed` are created when missing, `Pending`
first. A comment is one bullet:

`- [author] (timestamp) In <path> on line <N>: <text> - "<anchor>" (at <commit>)` or
`on line <A>-<B>` for a range. Author, timestamp, anchor and commit are optional, and so is the
whole `on line` clause: without it the comment is about the path as a whole, which is how a file
or a directory is commented on. The anchor is the trimmed text of the first commented line, cut
to 80 characters with an ellipsis; a comment on a path has none. The commit, abbreviated as git
abbreviates it, names the commit whose version of the file the line numbers count in: the
commit of a diff whose after side the comment was left on, or HEAD for a comment on the working
tree when the file is committed and unchanged there; otherwise there is none. Only seven to
forty hex digits in `(at …)` ending the line are read as one. It goes last because nvim-review
finds a comment by `on line N:` and reads everything after the first `: ` as text. The parser
accepts what
nvim-review writes and vice versa; nvim-review keeps the lines it does not itself write. A
path is read up to the first `: `, and the `on line` clause may only be left out on a bullet
at the left margin, so a nested prose bullet stays prose but a top-level one reading
`- In some cases: ...` is read as a comment on the path `some cases`. Lines that are not
comments, and headings the tool does not know, are kept as they are; the placeholder
`No pending comments` is removed when a comment is added under it. The file is rewritten
whole on every change, in the same order it was read.

## NOTES.md

Level-two headings name a file (repository-relative) or `General`. Bullets under a heading are
notes; a note continues on indented lines. `General` is kept first; new file sections go last.
Other content is preserved.

## OVERVIEW.md

What the tool last found out about the repository, next to the other two files, readable and
read back: `As of commit <hash> on <branch>, <YYYY-MM-DD HH:MM>.` under the title, then `Facts`
(`- N files, N tests in N files, N marked to skip`, and a line per check, `- <name>: passed in
<N> s, <when>`), `Start here` and `What the rest of it leans on` and `Named by no test` (a
bullet per path, `- <path> — <why or names>`), and `Tests` (a `###` heading per file with a
bullet per test as a phrase, `(skipped)` after one marked so). `About` holds whatever a person or
the agent writes, kept as it is; so is any section under a heading the tool does not know, kept
whole and last. The tool's sections are rewritten whole. Nothing in it changes with the clock,
and the tests carry no line numbers, so a rewrite means the repository changed.

It is read when a repository is opened. Until the symbol index is built, what it says stands in
for what the index would say — the leaned-on files and the tests, as of its commit and however
many commits behind — and the page says so and asks again every two seconds. It is written once
the repository has held still for ten seconds since it last changed, the index is built, and
nothing was written at that fingerprint yet; the browser server does this every five seconds for
the repositories a request has named in the last two minutes, the terminal UI for the one on
screen, and `codereview overview` on demand. Writing it does not count as the repository
changing. The web server, which knows what its checks did, writes them into `Facts`; the terminal
UI and the command, which do not, keep what the file says. The About section and the kept
sections are taken from the file as it is on disk at the time, not as it was read at open.

## Caching

The session keeps what it worked out until what it came from changes: the overview's figures
by the repository's fingerprint (one `git status` to check, which the page already pays every
two seconds), and what is read off the symbol index — the ranking, the tests, the untested
files — by an index generation that goes up when the index is built, replaced, or changed by a
refresh, which says whether any file did. The week's activity is the count of commits and their
authors, not the lines they changed: that needed a diff of every commit, seconds on a
repository whose commits carry a corpus.

The symbol index is built on a thread of its own when a repository is opened: the browser
server builds one repository's after another's from start-up and hands each to its session;
the terminal UI builds the open repository's and takes it in between key presses. A request or
a key that needs the index before then does not build a second one, but answers from
`OVERVIEW.md` (the browser) or waits (the terminal). Only a session nobody is building for
builds the index on the caller's thread, as before.

## Anchoring

A comment that names a commit is followed. The file at that commit is diffed with omnidiff
against the text the comment is placed in — the working tree, or the after side of a diff — and
the comment goes where its first line went: exact when that is the line number it had, edited
or not; moved when it is elsewhere, having moved, or had lines added or removed above it; stale
when the line was deleted, shown just below the nearest line above it that is still there. The
line is followed through omnidiff's AST diff, used as a library, which matches every node of the
old syntax tree, tokens included, to a node of the new one or to nothing: a line goes where the
outermost node starting on it that survived went, so it follows the function, statement or
brace it holds through moves and edits as the trees see them. A line on which nothing starts —
blank, or inside a comment or string spanning lines — keeps its distance from the start of the
innermost node around it, within where that node went; between top-level items, a blank line
keeps its distance from the nearest placed line above. Only the AST diff runs, not the text
rendering a diff view needs. Without trees — no grammar, over 20,000 lines, or a file that does
not parse — omnidiff's plain line diff maps the lines it leaves unchanged, one for one in order.
The map is kept per path and commit for as long as the text it was made for does not change, at
most 256 of them. A comment whose commit or path at that commit is not in
the repository any more is placed by its anchor instead.

Any other comment is placed by its anchor. If the recorded line still holds the anchor text, it
is exact. Otherwise the nearest line holding that text wins, and the comment is moved. If no
line holds it, the comment is stale and stays at its recorded line. Ranges keep their length. A
comment without an anchor, and one without a line range, is always exact. `reanchor` rewrites
the line numbers of moved comments; one that named a commit then counts in the working tree, so
it names HEAD if the file is committed as it is and nothing otherwise, and takes its line's
current text as its anchor. Both front ends say which commit a moved or stale comment was
followed from.

## Diffs

Two versions of a file are diffed with omnidiff when the language is one it parses and both
sides are under 20,000 lines; otherwise with omnidiff's plain-text line diff. The result, per
side, is one operation per line (none, insert, delete, update, move) and, per line, the byte
spans that changed. Binary content diffs as empty.

Targets: working tree against the index or HEAD (whichever the file's status says), index
against HEAD, a commit against its first parent (the empty tree for a root commit), and any
revision against another or against the working tree.

Side-by-side alignment anchors on unchanged lines with identical text (unique lines first,
then an in-order walk), pairs changed lines between anchors in order, and leaves the rest
alone on their side. Unified layout shows, per hunk, every before line and then every after
line.

## Symbols

The index covers every listed file under 2 MB that is not binary, parsed with the tree-sitter
grammar omnidiff maps to its language; files without a grammar get a pattern fallback
(Makefile targets and variables, plus words). Files are parsed in parallel on first use and
re-parsed on refresh when their size or modification time changed; files gone from the listing
or from disk are dropped.

A definition is a node whose kind ends in `_definition`, `_declaration`, `_declarator`,
`_item`, `_spec`, `_specifier` or `_signature`, has a `name` field (or a `type` field for a
Rust `impl`), is not a parameter or argument, and whose name is one line. Its kind label is the
prefix, normalised (`function`, `method`, `class`, `struct`, `impl`, `variable`, ...). Each
definition records its line, byte column, end line, the enclosing definition's name, and the
source line. In HTML the `id` attribute values are the definitions. An occurrence is any leaf
node whose kind contains `identifier`.

Queries: definitions by exact name, occurrences by exact name (definitions included), the
definitions of one file in order, and a case-insensitive substring search over names ranked
exact, prefix, then substring. The identifier under a position is the smallest identifier
node at that point (one column back is tried for a click just past a name); without a grammar
or on a non-identifier node it is the word there.

## Several repositories

Every path on the command line names a repository (a directory inside one counts), a file, which
opens its repository at that file, or a directory that is not inside a repository, in which case
every child directory with a `.git` entry (hidden names left out, sorted by name) is opened and a
line on stderr says how many; a directory that is neither is an error. The same repository named
twice is opened once, in first-seen order, and nothing named means the current directory (or
`-C`), resolved the same way. The TUI keeps one full app per repository (tree, viewer, screens,
agent) and shows one at a time; with more than one open, a strip on the top line names them with
their number, a `?` while that repository's agent waits for permission, a spinner while it works,
and the pending comment count. `gt` and `gT` cycle, `W` opens a picker (j/k, Enter, 1-9). Agents
in the other repositories keep running and are polled every tick. A theme or diff layout saved in
one app is adopted by the others without another write of the config file, so no app later saves
a stale copy. The web server holds one session and one agent per repository; every `/api` call
takes `?repo=N` (0-based, command-line order, default 0), `/api/repos` lists them with name,
root, branch and pending count, and the page's header dropdown (shown only with several)
switches, resetting the per-repository page state and rebuilding the agent transcript from that
repository's events.

## Agent

Two backends behind one set of events (text by role, tool calls merged by id, plans,
permission requests, turn end with stop reason, errors, stderr, exit, plus three kinds of
progress: log lines, a status phrase, and token usage). `[agent] kind` picks one: `claude`,
`acp`, or `auto` (Claude Code when its binary is on PATH, else ACP).

**Progress.** Both backends log the command they started (with its pid and directory) and
how long the handshake took. Claude Code also logs the `init` message (session, version,
model, permission mode, tool count, each MCP server with its status), rate-limit events,
unknown message types, and at the end of a turn a summary (wall time, API time, model calls,
tokens in and out, cost so far). Its status phrase follows the wire: prompt sent, waiting for
the model (`system status requesting`), streaming from the model with the call number
(`message_start`), thinking or preparing a tool (`content_block_start`), running a tool
(`tool_use`), tool finished, interrupt sent. Usage comes per model call from `message_start`
(input, cache creation and cache read tokens) and `message_delta` (output tokens), and the
session's total cost from `result`. Stderr lines are forwarded as log lines too. The panels
show the phrase with the turn's elapsed time, a note when the backend has been silent for
five seconds or more, the turn's tool-call count, accumulated tokens and cost; log lines sit
in the transcript, dimmed, and can be hidden (`l` in the TUI, the "log" box on the page).

**Claude Code** runs `claude -p --verbose --input-format stream-json --output-format
stream-json --include-partial-messages --permission-prompts host --permission-prompt-tool
stdio` plus `claude_args`, in the repository root. Start-up sends the SDK's `initialize`
control request and waits up to two minutes for its response. A prompt is one user message
with a text block (context appended as fenced blocks). Streamed `text_delta` and
`thinking_delta` events become agent and thought text; `tool_use` blocks in assistant
messages become tool calls (title from the tool's main argument, kind from its name, the
file path as location); `tool_result` blocks become completed or failed updates with the
result text; a `permission_denied` system message marks the tool failed; `result` ends the
turn (`success` as `end_turn`, an error result as an error); the user message Claude Code
emits on interruption ends the turn as cancelled. A `can_use_tool` control request becomes a
permission request with Allow, Always allow (present when Claude Code suggested rules; the
answer applies them) and Deny; the answer is a control response with `allow` and the
original input, plus the suggested `updatedPermissions` for Always, or `deny` with a message.
Esc sends an `interrupt` control request. No permission mode is imposed: Claude Code's own
settings decide what needs asking, and `claude_args` can pass `--permission-mode`.

**ACP**, protocol version 1, runs the configured command with stdio pipes, sends
`initialize` (file read and write capabilities on, terminal off), tries the agent's first
authentication method once if `session/new` fails with an authentication error, and opens one
session with the repository as `cwd`. Prompts carry the text and, when the agent accepts
embedded context, the selected lines as text resources. `fs/read_text_file` and
`fs/write_text_file` are answered directly, with `line` and `limit` honoured, after the path
is normalised and confirmed to lie inside the repository; anything else is refused.
`session/request_permission` is surfaced with its options; `session/cancel` is sent for Esc.
Node-based ACP commands are checked for Node 20 or newer before starting. A failed start
reports the agent's last stderr lines.

`claude_args` is appended after the permission flags, so a `--permission-mode` there decides
what is asked about. An agent runs in a process group of its own, which is signalled as a
whole when it is stopped, and starts without `CODEREVIEW_TOKEN`, `CODEREVIEW_PASSWORD_HASH`
or `CODEREVIEW_CONFIG` in its environment. A line of agent output longer than eight megabytes
is dropped rather than grown.

Presets: address one comment (then move it to Completed), address every pending comment,
review a diff into REVIEW.md, and a question about a line range.

## Colour schemes and configuration

A scheme names a syntect theme plus every colour the front ends paint: page and panel
backgrounds, cursor, selection, search, a line and a span background and a marker colour for
each of the four diff operations, and one colour per comment state. Eleven are built in
(Terminal, Dark, Light, Catppuccin Latte, Catppuccin Mocha, Solarized Light, Solarized Dark,
Gruvbox Light, Gruvbox Dark, Nord, Dracula). Terminal is the default: it uses only palette
indices and the terminal's default colours, shows the cursor line in reverse video, and marks
changed characters by colour and bold instead of a background. The browser UI does not offer
Terminal; when it is configured the page follows the browser's light or dark preference with
the Light or Dark scheme and highlights with Dark's syntax theme.

The config file is `$CODEREVIEW_CONFIG`, else `$XDG_CONFIG_HOME/codereview/config.toml`, else
`~/.config/codereview/config.toml`: `theme`, `layout` (`auto`, `side-by-side`, `unified`),
`timestamps`, `[agent]` with `kind`, `claude_command`, `claude_args`, `command` and
`args`, `[sidebar]` with `files_auto_hide` and `outline_auto_hide`, and any number of
`[[analysis]]` checks. Missing keys take defaults; a malformed file is an error. The terminal UI
saves on Enter in the picker (`t`; `j`/`k` preview live, `Esc` restores), on every layout change
(`v`) and on every change in the options panel (`,`), and hands what it saved to the other
repositories open beside it; the browser UI saves from its header dropdown. Both read the file
when they start.

## Analysis

The checks a project already has, run from the tool. Each `[[analysis]]` entry in the config
file has a `name`, a `run` command, optionally the artefact it leaves behind (`reads`, relative
to the repository root) with how to read it (`kind`: `junit` or `lcov`), `watch`, the
repository roots where it may also run without being asked, and `repos`, the repository roots it
belongs to — empty, the default, for every repository.

A repository with nothing configured still says how to test it, and its checks are offered as
found rather than configured: its Makefile's `test` and `check` targets, then `cargo nextest
run` (when `cargo-nextest` is on `PATH`) or `cargo test` for a `Cargo.toml`, with `--workspace`
for a workspace; `npm`, `pnpm`, `yarn` or `bun` `test` for a `package.json` with a real test
script, by its lockfile; `pytest` for a pytest configuration, or a `tests` directory beside a
`pyproject.toml` or `setup.py`, through `uv run` or `poetry run` when their lockfile is there;
and `go test ./...` for a `go.mod`. The commands come from that table and never from the
repository. One found is not offered when a configured check for the repository runs the same
command or has the same name. Running a found check saves it to the config file first, scoped to
that repository, or adds the repository to an entry already saved for another one; the file is
read again before any save, so that sessions sharing it do not undo each other's changes.

A run is a process; its result is a file. The command runs through `sh -c` from the repository
root with its error output folded into its ordinary output and no terminal at all, so anything
that would stop to ask fails instead of waiting. What a run reports is an exit code, how long
it took, and the last 64 KiB of what it printed, whole lines only; all of it is held in memory
for as long as the server runs and no longer. What survives is the artefact: `junit.xml` read
for tests, failures and errors together, and skipped — from the wrapping `<testsuites>` when
there is one, otherwise the suites added up — and `lcov.info` read for `LF`/`LH` as a
percentage of lines covered. Without an artefact, a finished run's output is read for the
counts a test runner prints at the end — `cargo test`'s `test result:` lines added up, nextest's
`Summary`, pytest's closing line, jest's and vitest's `Tests` — as tests run, failed and skipped.
Figures are read on request and carry the artefact's own modification time, so a figure with
no run behind it, left by the same command typed in a terminal, is shown and dated rather than
hidden. An artefact path that is absolute or climbs
out of the repository is ignored. Each result also says how far the code has moved since it:
the commits reachable from HEAD made after the run started, or after the artefact was written,
and the files in the working tree written after it, leaving out `REVIEW.md`, `NOTES.md` and the
artefact itself. Commit times are whole seconds, and a commit in the same second as the run
counts as having come before it.

Starting a check that is already running is a conflict; naming one that is not configured is a
bad request. A repository named in a check's `watch` is also watched: every five seconds its
fingerprint is taken, and when it has held the same value for ten seconds the checks that named
it run — but only if the repository has changed since that check last ran in it, so that the
two minutes between runs is a floor and not a schedule. One check runs at a time across the
whole server, however many repositories it serves. Nothing is watched unless a check names it:
a command that starts itself in a repository that was cloned rather than written is code
execution from a clone, so it is chosen one repository at a time, in the user's own config file
and never from anything inside a repository.

## End-to-end recordings

An end-to-end test that drives the software the way a person does leaves a recording of itself
in a directory of its own: the recording, `video.webm` from a browser or `terminal.cast`
(asciicast v2, with the keys sent as input events) from a terminal, and `test.json`. That file
names the test as written and the file and line it is defined on, says whether it passed, when
it started and how long it took, which commit was under test and whether the tree had changes
on top of it, and which file is the recording; then it lists the steps, in order, each with the
milliseconds since the recording started, the line of the test that made it, whether it is an
action or a check, and its text, and for a check whether it passed and, if not, what was seen
instead. It is written when a test fails as well.

A recording is made at a person's pace: the browser holds each tap, keystroke and page load for
600 ms and marks each tap with a dot where the finger landed, gone before the test goes on; the
terminal holds 450 ms before each key and types text a character at a time.

Recordings are looked for under `target/e2e/`, `test-results/` and `e2e-results/`, at most
four directories down and two thousand of them, without following links. A recording named in a
`test.json` is used only when it is a plain file beside it with a name the tool can play: `.webm`,
`.mp4` or `.ogv` as a video, `.cast` as a terminal recording. The browser server serves exactly
those files and nothing else, with byte ranges for seeking. A terminal recording is played once
through a terminal emulator into frames, output less than 16 ms apart making one frame, each
frame the rows that changed as runs of text with their colours (a palette index or red, green and
blue) and bold, italic, underline and inverse; both front ends draw those. A recording over 32 MB,
or of a terminal over 500 by 200, is not played.

The browser test harness names itself as the runner of the page's tests in each `test.json`, and
remuxes each video once more after recording, which writes its length and where its frames are,
so that a player can show the one and seek in the other.

Two more things may sit beside a recording. `docs` in `test.json` names the documentation the
test demonstrates, each as `path#heading`, the heading as written or as a link to it is spelled.
`coverage.lcov` is the code the run executed, in lcov: its `SF` paths, absolute under the
repository or relative to it, and the `DA` lines with a count above nothing; paths outside the
repository are left out.

This repository's own end-to-end tests leave theirs under `target/e2e/terminal/` and
`target/e2e/browser/`, a directory per test named after it in lower case with dashes. The
terminal tests start the binary in a 110 by 32 pseudo-terminal in a scratch repository with a
home and a configuration file of their own, and each replaces only its own directory, since they
run at the same time. The browser tests run one after another in a container, each on a fresh
page at 360 by 640, and the whole browser directory is replaced after every run, so a test that
is gone takes its recording with it.

Run under `cargo llvm-cov` (`make e2e-coverage`), each terminal test's program writes its
coverage profile to a directory of its own and is asked to leave with Esc and Ctrl-C, since a
killed program writes none; what it drew while leaving is cut from the recording. The browser
tests share one server, which in a build instrumented for coverage, and only there, answers a
call that writes what it has run to a file named for the test and starts counting afresh; the
page's script is counted by Chromium for every run, coverage build or not. The harnesses turn
the profiles into each test's `coverage.lcov` with the toolchain's `llvm-profdata` and
`llvm-cov`.

## What a repository opens on

Both front ends open a repository on its end-to-end tests. A test is one when it is in a file
that holds nothing but tests — under a `tests`, `test`, `e2e`, `end_to_end`, `__tests__`, `spec`,
`specs`, `integration` or `functional` directory, or named `*.test.*`, `*.spec.*`, `*.e2e.*`,
`*_test.go`, `*_test.py` or `test_*` — and either its path says end to end (`e2e`, `end_to_end`,
`browser`, `playwright`, `cypress`, `selenium`, `puppeteer`, `webdriver`, `headless`), or the file
drives the software from outside: it names a browser driver (puppeteer, Playwright, Selenium,
WebDriver, Cypress, chromedp, fantoccini, thirtyfour, headless_chrome), a pseudo-terminal
(portable_pty, expectrl, rexpect, pexpect, node-pty) or the built program (`CARGO_BIN_EXE_`,
assert_cmd), counting the files a Rust test pulls in with `#[path = "..."]`. A test a recording
names is one in any case. Code with its unit tests beside it is never one, whatever it is called
or mentions. Each says why it counts, in a few words. The specification's levels follow the same
rules, so its End to end level and the opening list the same tests; a test whose name or path is
about security stays under Security there, and carries a security mark in the opening.

A recording is matched to its test by file and by name, as written or as its phrase. A test that a
recording names as its runner (`run_by`) is left out: it hands another file's tests to a runner
and is not a behaviour of its own. Recordings whose test is not there any more are listed after
the tests, as recordings of tests no longer here. For each recording the opening says how far the
code has moved since it was made: the commits on HEAD made after it started, and the files the
working tree has changed since then outside the recordings' own directories, worked out from one
status and one log for all of them. Writing, replacing or removing a `test.json` moves the
repository's fingerprint, though git ignores those directories, so a page that is open redraws
when a test run finishes.

A test's own view is where a reviewer goes into the code from. Beside its recording are its
steps, each with the line of the test that took it, read from the test's file. Then the code its
run executed, a file at a time, with how many lines ran and how many of those no other
end-to-end test ran, ordered by the second and then the first, at most sixty; the test's own
files are not code it ran, and nor is anything the repository does not list. Without
`coverage.lcov` the view says the run left none. Then its documentation: the sections it names,
or, when it names none, at most three guessed from the words of its sentence — quoted text and
anything spelled like a path left out — against the headings and bodies of every Markdown file
except REVIEW.md, NOTES.md, OVERVIEW.md and a changelog: a section must share at least two words
with it, a word counts for more the fewer sections use it, three times as much in a heading, and
less in a long section than a short one. A named section that is not there is not replaced by a
guess. Last, at most twenty smaller tests that name a file the run executed or live in one,
each file counting for more the higher it ranks in what the test ran.

A repository with no end-to-end test opens on that finding: nothing drives it the way a person
does, so nothing shows that it works. What one would look like is said for the kind of software it
appears to be — a web page (HTML or a web framework in its manifests), a terminal program (a
terminal UI library), or a command (an entry point) — and the agent is offered the job of writing
the first ones, with a request that spells out the recording format above so that each new test
leaves a recording to watch. The request is the same in both front ends.

## What a stretch of history did

A stretch — the last seven days, the last commit, uncommitted work, or one commit from the log —
is seen through the end-to-end tests, in three parts, in the order a reviewer needs them.

What happened to the tests comes first: removed, skipped and weakened, as in the specification.

Then the changed code no end-to-end test ran. The lines the stretch added or changed are read from
one `git diff -U0` against its base, with every line of an untracked file when it ends at the
working tree; lines only removed leave nothing. Of those, only code counts: not files of nothing
but tests, not the test code inside a file of code, and only files the tool reads as code or some
test's coverage names. In a file coverage knows, and only when that coverage is newer than the
file, a line counts if any test's coverage lists it at all, run or not, which leaves out blank
lines, comments and markup; anywhere else, any line with something on it counts. A line no test's
last run executed is one no end-to-end test ran. The files with such lines are listed, most
first, at most a hundred, with how many of their changed lines no test ran; each end-to-end test
that ran some of the changed lines says how many, and the tests whose runs left no coverage are
counted, since what they ran is not. How the lines a test ran line up with the lines changed is
git's alignment of the hunks, so a brace may count with the line beside it.

Last, each test before and after. The before is the tests run at the stretch's base commit, in a
worktree of it kept under the git directory (`codereview/before/<commit>`), by the repository's
end-to-end command: a configured check, or a found one, whose name or command says end to end —
a Makefile's `e2e` target is found like its `test` and `check`. It is never run unasked: the page
offers a button that says what it will run and where, the terminal UI a key, and the command line
`--record`. At most three such worktrees are kept, the oldest removed when another is made. Once
there are recordings at the base, each test says whether it passed then and now, how many of its
steps are on one side only — steps are matched by kind and text, as the longest common run — and,
for two terminal recordings, at how many matched steps the screen's text differs. A test's own
view shows both recordings side by side and the steps in pairs; choosing a pair moves both.

## Highlighting

syntect on the oniguruma regex engine with two-face's syntax set, chosen by extension, then
file name, then first line. Either front end shows the first 20,000 lines of a file and says
how many there are; past that a file is a fixture or a dump, and drawing it whole is what
freezes a phone. Files over 20,000 lines are not highlighted either. The page draws a file's
rows in batches, the first at once and the rest a frame at a time, so a long file shows its
head while the rest is built. A diff's two sides are kept by content, the last sixteen,
since the syntax-aware engine costs a couple of hundred milliseconds on a large file and the
same diff is asked for on every open of a modified file. The output is a list
of coloured segments per line, which both front ends cut at diff-span and search-match
boundaries. The last sixteen files highlighted are kept, by path, theme and content, since a
review opens the same file again and again and a couple of thousand lines of Rust are a
quarter of a second to highlight. On the wire a file's lines are packed: the distinct styles
once, in a table, and per line `[style, text]` pairs with runs of one style joined, a sixth
of the size of one object per segment; the page unpacks them on arrival.

## Terminal UI

Screens form a stack; `q` or `Esc` pops one, and quits from the explorer. The explorer has a side
panel — the file tree (folding, filter, status letters, pending-comment counts for files and
directories; past the tenth file of a directory the rest fold behind `… N more`, except the
changed, the commented on and the one under the cursor, and nothing folds under a filter),
or, after `O`, the outline of the open file — and the file viewer (syntax
highlighting, line numbers, change markers against HEAD or the index, comment rows under the
lines they refer to, comments on the whole file above its first line, optional blame column,
search with smart case, visual line selection). A picture — PNG, JPEG, GIF, WebP, BMP or ICO —
is drawn in the viewer, fitted to it, where the terminal speaks a graphics protocol (kitty,
sixel or iTerm2); where it does not, the viewer says it is a picture and how big. The terminal
is asked which protocol it speaks when the first picture is opened, not before, with a second to
answer; one whose `TERM` is unset or `dumb` is not asked. An SVG is text and shows as its
source. The outline is
what the open file defines, parsed on its own so it does not wait for the symbol index, fields
left out, each definition indented under the ones whose lines hold it; its cursor follows the
file's while the file has the focus, `Enter` jumps to a definition as `Ctrl-o` can come back
from, and `Space` shows it without leaving the outline. Either panel can be set to hide: it is
drawn while it has the focus and left out while the file has it, the file taking its width,
and `Tab`, or `q` from the file, brings it back; with no file open it always shows. `,` opens
the options panel — each panel's auto-hide, the diff layout, the colour scheme — where `Space`
changes a setting, saves it at once, and the setting under the cursor is explained. `c` on the tree
comments on the selected file or directory as a whole; `c` in the viewer comments on the line
or the selection. Other screens: repository log and per-file history (commit list, details, files
of the selected commit, paging in 200s), working tree and staged change lists, a diff view
(auto, side by side or unified, hunk navigation, next and previous file of the same commit,
comments on after-side lines), the review list (jump to, complete, edit, delete, re-anchor),
the notes list, a locations list (definitions, occurrences, symbols; Enter jumps, and a jump
stack brings the cursor back), and the agent panel (transcript with roles, tool calls and
plans; a permission bar answered by number or `y`/`n`; the running turn cancelled with Esc;
the transcript kept while other screens are shown). The viewer has a column cursor moved by
character and word, drawn in reverse video, from which the identifier for navigation is
taken. A single-line prompt at the bottom takes comment and note text, search queries, line
numbers, the tree filter, symbol searches and agent prompts. The agent starts on another
thread so the interface never waits for it. `o` suspends the UI and runs `$VISUAL` or
`$EDITOR` at the current line. The mouse wheel scrolls.

A repository opens on its end-to-end tests, pushed over the explorer, unless a file was named on
the command line; this goes for a repository that appears later in a directory of checkouts too.
`q` leaves them for the code and `E` brings them back from any screen; the tests that name the
open file, which `E` used to show, are on `U`. Until the symbol index is built the tests are
those `OVERVIEW.md` lists, and with no such file the screen says it is reading them; either way
it reads them again once the index is in, and again on `r`, staying on the same test. The list
has each test file as a heading with its tests under it, each marked passed, failed or without
a recording, and a security mark where it has one; recordings whose test is gone follow under a
heading of their own. The title counts the end-to-end tests against all the tests, the failed
and those without a recording. Beside the list is the selected test: its label, file and line,
why it is end to end, how its last run went and when, how long it took, how many commits and
changed files the tree has moved on since (or that it matches the tree), the commit it was
recorded at and whether that had uncommitted changes, and up to three failed checks with the
first line of what was seen instead. A terminal recording is played into frames when its test
is first selected, shown paused at its first frame in a box titled with the position, the
length and whether it plays, and cropped to the box with a note when it is bigger; `z` gives it
the whole screen. Under it are the test's steps with their times, the one the position is in
highlighted. `Space` or `p` plays and pauses, from the start again once it has ended; `]` and
`[` go to the next and previous step; `0` goes back to the start; while it plays the screen is
drawn every 40 ms. A video cannot be drawn, so it is named, and `o` hands it to the system's
opener (`xdg-open`, or `open` on macOS). `s` opens the test's source at its line. A
repository with no end-to-end tests opens on that finding instead: what one would look like for
the kind of software it seems to be, how many tests of other kinds it has, and `a`, which sends
the agent the request for the first ones, each leaving a recording.

`Enter` opens a test on a screen of its own, over the list: where it is and why it counts, how
its last run went and what any failed check saw, its recording with the same keys as the list,
and below it one list in four parts. Its steps, each with its moment and its mark and, under it,
the line of the test that took it, the step playback is at marked; `Enter` on one goes to its
moment and `s` to its line. The code it ran, a file a row with how many lines it ran and how many
of those no other end-to-end test ran, those first; or, without coverage, that its run left none
and, where the Makefile has an `e2e-coverage` target, that it records it. `Enter` opens the file
at the first line it ran, with every line it ran marked by a bar in the gutter and the status
line naming the test on a marked line; the mark lasts while that file stays open and through a
refresh, and a file opened any other way has none. The documentation it names, or the closest
sections as a guess with the words they share, each opening at its heading. The smaller tests
that name a file it ran or live in one, with their level and the files they share, each opening
at its source. A screen opened while the symbols are still being read says so and fills in when
they are. `q` goes back to the list. The terminal end-to-end tests name the section they
demonstrate (`documents`), and under `cargo llvm-cov` each writes the coverage of its own run.

Both screens mark a stretch of history, the last 7 days to begin with; `w` moves on to the last
commit, then the uncommitted work, then back, and the list's title names it. Above the tests the
list says what the stretch did: under `In <stretch>`, the tests it removed, skipped or made to
check less, each opening on the diff of its file across the stretch, or that it did none of
that; then `No end-to-end test ran N of M changed lines` and a row per file of such lines, the
first eight with the rest folded under `… N more files` until `Enter` unfolds them, each opening
on the file at the first of them with every one marked by a bar of the colour of a moved
line and the status line saying it was changed in the stretch and run by no end-to-end test, or
that it changed no code, or that every changed line ran; a warning when some end-to-end tests
left no coverage, whose runs are then not counted; and the stretch's base commit, when it has
one: recorded, with how many tests, or not, with `b` to record it, or that no command records
the tests. Beside the list, a row of the stretch explains itself. `b` makes the worktree of the
base and runs the end-to-end command there in the background; the row says it is recording, for
how long and the last line it printed, and when it ends the status line says how it went and
both screens read the stretch again. Each test in the list carries `Δn` for the changed lines it
ran, its result before and now when they differ (`✗→✓`), and `screen changed`, or `steps
changed`, when its run differs from the one at the base. On a test's own screen, once the base is
recorded, its recording at the base is played beside the one now (one above the other when both
do not fit across), and its steps are paired: both times, both marks, the step, and `only now`,
`only before` or `screen changed`; `Enter` on a pair takes both recordings to it, and `Space`,
`]`, `[` and `0` move both.

## Browser UI

One embedded page served by an axum server on a loopback address (by default) with a random port.

Signing in works one of two ways. With `CODEREVIEW_PASSWORD_HASH` set, to an Argon2 hash from
`codereview hash-password`, the page is a sign-in form and the password is checked against it;
wrong answers are counted and sign-in then stops answering for a doubling delay, capped at a
quarter of an hour, which a success clears. Otherwise a token opens a session: `CODEREVIEW_TOKEN`
when set, which keeps one URL across restarts and must be at least sixteen characters, otherwise a
fresh random one for the run. Setting both is an error, and a non-loopback address without a
password is refused. Either way the browser ends up with a session: a cookie holding an
identifier, and a second secret embedded in the page which every API request must send as a bearer
header, never the cookie, so a page on another site cannot act as the signed-in browser. A session
lasts a week, at most thirty-two are kept, and signing out ends one. Secrets are compared without
stopping at the first wrong byte. Every response forbids storing, referrers and framing, and
restricts the page to its own origin. `--public-url` names what a reverse proxy publishes: it is
the URL printed at start-up, and an HTTPS one marks the cookie `Secure`. Everything the tool can
do it can do for anyone who signs in: there is no read-only mode, and no way to serve the
repository without the agent, the comments or the commit. The server stops on SIGTERM as well as
on Ctrl-C. The API mirrors the session: state, file with highlighting and change markers, a
picture's bytes with its media type (nothing that is not a picture), blame,
log, commit files, changes, a repository's overview, what it leans on and its tests, diff with
highlighting on both sides and anchored comments, comments and notes (list, add, toggle, edit,
delete), commit, the checks configured and found and starting one, refresh and re-anchor, the theme
list and config, a version, symbols (file symbols, definitions, occurrences, search, identifier at a
position), and the agent (start, poll for events since a sequence number, prompt with text or a
preset, answer a permission, cancel, stop, clear). The version is a fingerprint of everything a
front end draws:
the status of every path, the branch, and the size and modification time of both Markdown files
and of one named file, hashed into a number that means nothing on its own. It costs one `git
status`, and it moves whenever anything on screen would look different, whoever made the change; a
status alone would not, since a file written twice is modified either way. The server writes the
transcript itself, the same way the terminal does, and answers a poll with the entries that
changed since the version the page names, each with the position it belongs at: a tool call is one
entry, filled in as it runs, so entries are not append-only. It also sends what the turn is doing
and what it has cost, already worded, so the page only draws. The page has an overview the
terminal UI has not, and otherwise the same: explorer with tree and file, changes, log and
history, diff, review, notes, and the agent tab; clicking a name in code offers its definition and
usages in a side drawer. A picture in the working tree is shown in the file view, fitted to it
on a checkerboard, with its size in pixels and bytes under it; an SVG has its source below it.
The page fetches the bytes with the API token and shows them from memory, which the content
security policy allows as `blob:` images; the endpoint serves an SVG under a policy that runs
nothing. A comment left in a commit's diff sends that commit with it. A comment on a whole file
comes from the Comment button in the file
header and is shown above the first line; one on a directory from the `+` on its row in the tree.
A range is chosen with shift-click, or with the comment form's Extend range button followed by the
line number at the other end, which keeps the text typed so far. Editing a comment or a note, and
commenting on a file or a directory, opens a text sheet of the page's own rather than the
browser's prompt. The file header's Zen button, and `z`, leave the file alone on the screen: the
header, the tabs, the tree, that header itself and the footer go, and what stays is the code, its
line numbers, its comment rows and one button at the bottom right to come back, which `z` and
`Esc` also do. A message still shows the footer for as long as it has something to say, and a key
that reaches for a field in the header comes out of zen first. Zen is a way of reading rather than
a view, so it has no address, and leaving the file leaves it. `w`, and the same menu's Wrap lines,
wraps long lines where the screen ends instead of scrolling sideways, with each continuation
indented past the start of the code so a wrapped line still reads as one; the choice is kept in
that browser's own storage.

A review that ends in "this is good" ends in a commit, from the changes view: the working tree
side stages everything it lists and commits that, the staged side commits the index as it
stands, and the button says how many files either would take. The message is written in the
text sheet, which first spells out the files that are about to go in and how many comments on
them are still pending — an unanswered comment on a change is the one thing not to commit by
accident. The repository's hooks run as they do on the command line; a hook that refuses stops
the commit, and what it printed comes back into the sheet with the message kept, so it can be
tried again once the repository is happy. A commit with no message is refused before git is
asked, as is one with nothing to commit.

The page asks for that version every two seconds while it is on screen, and re-reads the
repository and redraws the current view when it has moved, so an agent's edits, another window's
or a git command's appear without anyone pressing Refresh. It does not ask while it is hidden,
and catches up as soon as it is looked at again; it holds off while a comment is being written,
while the text sheet is up and while the menu over a name is open, and a refresh nobody asked for
keeps the scroll where it was, says nothing in the footer, and leaves the agent's own view to the
agent's own poll. A write of the page's own moves the version too, so the next reading after one
becomes the baseline instead of redrawing what the page has just drawn.

A repository opens on its end-to-end tests, under the Tests tab, at the root address; the overview
is the Summary tab, at `summary`, and the specification is All tests, under the same tab. Each
end-to-end test is a card, grouped by file, in a grid as wide as the screen allows: a mark for how
its last run went, its sentence, a security or skipped badge, its file and line (which opens it),
why it counts, when it ran and whether the tree has moved since, then its recording, what any failed
check saw instead, and its steps, folded. A video is fetched with the API token and played from
memory by the browser's own player, once its card comes near the screen; a terminal recording is
fetched as the server's frames and drawn as text on a dark terminal, sized to fit its card, opening
on the first frame with anything on it, with a play button, a scrubber and the time. Tapping a step
goes to its moment of the recording, and the step at the current moment is lit as it plays. The
header counts the end-to-end tests against all the tests, the failed and the ones without a
recording. A repository without any opens on the finding, with the button that asks the agent and
one to the tests it has. A test's sentence opens its own view, at `test` with its file and name: the
recording and its failures beside the steps, each step's line of the test under it, which opens the
test there, and tapping a step goes to its moment; then the code it ran, each file with a bar of
what ran and what only it ran, the documentation marked as named or as a guess with the words it
shares, and the smaller tests. A file opened from the code it ran marks those lines in its gutter,
under a line naming the test that leads back to it; another file opened later is not marked.
The opening has the stretch buttons of the specification, and a card saying what the chosen
stretch did, between the counts and the tests; each test's card carries what the stretch has to do
with it: the changed lines it ran, and how it went before and now. A file opened from the changed
code no test ran marks those lines in another colour, under a line saying so. Recording the base
shows the command's progress and the end of its output, and the card is worked out again when it
is done.

The overview is not on the file tree either. It is what the project says it is —
the prose its README opens with, as plain text, up to the first heading after it — then where to
start reading, what the rest of the tree leans on, the
other documents beside the code, and the biggest files. Above all of it: the branch, how many
files and how many are changed, how many comments are pending and how many of those have lost
the lines they were left on, the commits of the last week with who made them, and the count of
`TODO`, `FIXME`, `HACK` and `XXX` in tracked files.
Every count is a way in, and every path opens. The tree is one button away, at its own address;
the Explore tab with no file open is the tree as well.

What a README opens with is often not prose, so none of this is kept: titles, which are the
project's name and already on the page; lines that are only images, links or markup once those
are taken out, which is what a row of badges or a table of contents is; images, even ones whose
description wraps; HTML comments, code blocks, tables, link definitions and reStructuredText
directives. What is kept loses its links, tags and emphasis and keeps their words. A last
paragraph ending in a colon introduced something that was left out, and goes too. About two
hundred characters are taken, cut at six hundred, and a README with no prose has no section.

Where to start reading is convention and nothing cleverer: `src/main.rs`, `src/lib.rs`,
`__main__.py`, `package.json`, `main.go`, a `Makefile`, an `index.html`, each with a few words
saying why. What the tree leans on is computed, and asked for separately because the first one
builds the symbol index: for every name a file defines, how many *other* files mention it,
divided by how many files define that name. Fields are left out, names of one or two characters
are left out, a name defined in more than three files is dropped as nobody's, and a plain short
word is dropped as everybody's — nothing here resolves a name to what it means, so a file
defining a method called `push` would otherwise collect every `push` in the repository. What is
left is a project's own vocabulary, and the ranking is shown with the names that earned it, so
a reader can see why a file is there and disbelieve it.

What needs the symbol index — what the tree leans on, and the tests — is asked for after the
rest of the overview is drawn, and while the server is still building the index it comes from
`OVERVIEW.md`, marked "as of" its commit, and is asked for again until it does not. A test from
the file has no line number and opens its file at the top.

The overview ends with the repository's tests. First its checks, configured and found: what the
last run in this server did, what the artefact or the output says and how old it is, how many
commits and changed files have come since, or that the result matches the tree, a button to run
one — which, for a found one, also saves it — and its output when there is any. They are asked
for on their own, so the rest of the overview does not wait for them, and while one runs they
redraw themselves every two seconds until the overview is left. Then what the tests say: every
test the repository defines, found by reading it rather than running it, counted, and grouped by
file, each file folded until it is opened and each test a phrase that opens its line — a Rust
`#[test]` function, a pytest `test*` function or `Test*` class method in a file pytest collects,
a Go `TestXxx` in a `_test.go` file, a JavaScript or TypeScript `it` or `test` call in a test
file, named with its `describe` blocks — with the ones marked to skip marked. Last, the files the
rest of the tree leans on most that no test code names: test code being a test, a `#[cfg(test)]`
module — a file of its own when declared as `#[cfg(test)] mod name;` — or a whole test file,
and naming being the rule what it leans on uses, so that a file's imports do not make what they
import look tested. A file with tests of its own is tested, and
only files in the languages above are listed. The list says that a test reaching a file through
a URL or a command line never names it, which is the way it is most often wrong. The overview is
about one repository; the header's selector switches to another.

Every view has an address in the URL fragment, which never reaches the server: the end-to-end
tests at the root, the overview, the specification, the tree, a file
with an optional line, the working tree or staged changes, the log with an optional path and
commit, a diff with its parameters, and the review, notes and agent tabs, prefixed with the
repository's number when it is not the first. The browser's back returns to the previous view, a
reload keeps the place, and a diff's back button goes to whatever opened it. Paging through the
files of one change replaces the address instead of adding to the history.

**On a phone.** Below 700 pixels of width, or 500 of height, the header is one row with the tools
(symbol and file search, colour scheme, refresh, sign out, help) behind one button, and it gets
out of the way as the content scrolls down and comes back as it scrolls up, except while its own
menu is open. The tabs run along the bottom edge, four of them rather than six: the log lives
under changes and the notes under review, each reachable by a button in the other's header and
each keeping its own address, and the tab that leads to one is the one lit while it shows. Views
show one pane at a time: the tree or the file, the commits or one commit, each with a back button,
and the symbol drawer as a sheet over the lower half. Everything the file's header can do except
Comment is behind one button, as a menu under it: seven buttons are three rows of a small screen,
more than the header, the tabs and the footer together. The footer is there while it has something
to say and gone otherwise, since a permanent row of instructions is a row a phone does not have.
Lists lay each item out as a card with its buttons underneath. The diff is unified, with the hunk
buttons floating at the bottom right. Writing a comment takes over the screen rather than opening
between two lines of code, which leaves no room for the text and its buttons: the form covers the
page, sized to what the keyboard leaves of the viewport, and the Extend range button puts it away
until the second line of the range is tapped, keeping the text typed so far. The text sheet does
the same with the keyboard. A comment row stays as wide as the panel and sticks to its left edge
while the code scrolls sideways, so a comment's text and buttons are in view whatever the line
length; this holds on every screen size, as does the comment form on a screen wide enough to keep
it between the lines. On a screen that cannot hover, the directory `+` is always visible, tap
targets are larger, inputs are 16 pixels so iOS does not zoom into them, and what the page says
about acting on something says tap rather than click and names no keys. The sign-in page, which
shares none of this stylesheet, carries the same viewport declaration and the same 16-pixel
fields, and stops centring itself on a screen too short to centre in. The page declares a web app
manifest and icons, served without a session, so a phone can put it on its home screen as a
standalone app whose title bar follows the colour scheme; the agent panel stops polling while the
page is hidden and catches up when it is shown again.

## Command line

`codereview [FILE]` opens the terminal UI, `web` the browser UI, `list` prints comments with
their current anchor state (`--all`, `--path`, `--json`), `add` and `note` write to the two
files (`add` takes `N` or `N-M` lines, or none at all for a comment on a whole file or
directory), `toggle` moves a comment between sections by its number in `list --all`, `reanchor`
rewrites line numbers, `def`, `refs` and `symbols` query the index, `e2e` lists the end-to-end
tests with how their last recorded runs went (`--json` for all of it; `--test NAME` for one of
them, with its steps, the code it ran, its documentation and the tests below it; `--stretch`
for what a stretch did, with `--record` to record the tests at its base first, and with `--test`
for one test before and after), and `agent`
sends one
prompt and streams the answer to stdout (permission requests are asked on the terminal, or
auto-allowed with `--yes`). `-C DIR` selects the repository.
