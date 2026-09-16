# Specification

What codereview does, as built. PLAN.md has the reasoning and the roadmap; this file has the
behaviour. No code here.

## Repository access

Everything git-related runs the `git` binary from the repository root, found with
`rev-parse --show-toplevel` from the directory given (`-C`) or the current one. Used commands:
`ls-files` (tracked plus untracked, ignored excluded), `status --porcelain=v1 -z`, `log` with a
custom record format (`--follow` when limited to one file), `diff-tree` for a commit's files,
`diff --name-status` between two revisions, `show <rev>:<path>` for file content at a revision
(`:<path>` for the index), `blame --porcelain`, and `config user.name` for the author.

A repository without commits works: the log is empty, every file counts as added, and diffs
compare against nothing.

## REVIEW.md

Sections are level-one headings. `Pending` and `Completed` are created when missing, `Pending`
first. A comment is one bullet:

`- [author] (timestamp) In <path> on line <N>: <text> - "<anchor>"` or `on line <A>-<B>` for
a range. Author, timestamp and anchor are optional, and so is the whole `on line` clause:
without it the comment is about the path as a whole, which is how a file or a directory is
commented on. The anchor is the trimmed text of the first commented line, cut to 80
characters with an ellipsis; a comment on a path has none. The parser accepts what
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

## Anchoring

A comment is placed by its anchor. If the recorded line still holds the anchor text, it is
exact. Otherwise the nearest line holding that text wins, and the comment is moved. If no line
holds it, the comment is stale and stays at its recorded line. Ranges keep their length. A
comment without an anchor, and one without a line range, is always exact. `reanchor` rewrites
the line numbers of moved comments and nothing else.

## Diffs

Two versions of a file are diffed with codediff when the language is one it parses and both
sides are under 20,000 lines; otherwise with codediff's plain-text line diff. The result, per
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
grammar codediff maps to its language; files without a grammar get a pattern fallback
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
`timestamps`, and `[agent]` with `kind`, `claude_command`, `claude_args`, `command` and
`args`. Missing keys take defaults; a malformed file is an error. The terminal UI saves
on Enter in the picker (`t`; `j`/`k` preview live, `Esc` restores) and on every layout change
(`v`); the browser UI saves from its header dropdown. Both read the file when they start.

## Highlighting

syntect with two-face's syntax set, chosen by extension, then file name, then first line.
Files over 20,000 lines are not highlighted. The output is a list of coloured segments per
line, which both front ends cut at diff-span and search-match boundaries.

## Terminal UI

Screens form a stack; `q` or `Esc` pops one, and quits from the explorer. The explorer has the
file tree (folding, filter, status letters, pending-comment counts for files and directories)
and the file viewer (syntax highlighting, line numbers, change markers against HEAD or the
index, comment rows under the lines they refer to, comments on the whole file above its first
line, optional blame column, search with smart case, visual line selection). `c` on the tree
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

## Browser UI

One embedded page served by an axum server on a loopback address (by default) with a random port.

Signing in works one of two ways. With `CODEREVIEW_PASSWORD_HASH` set, to an Argon2 hash from
`codereview hash-password`, the page is a sign-in form and the password is checked against it;
wrong answers are counted and sign-in then stops answering for a doubling delay, capped at a
quarter of an hour, which a success clears. Otherwise a token opens a session: `CODEREVIEW_TOKEN`
when set, which keeps one URL across restarts and must be at least sixteen characters, otherwise
a fresh random one for the run. Setting both is an error, and a non-loopback address without a
password is refused. Either way the browser ends up with a session: a cookie holding an
identifier, and a second secret embedded in the page which every API request must send as a
bearer header, never the cookie, so a page on another site cannot act as the signed-in browser. A
session lasts a week, at most thirty-two are kept, and signing out ends one. Secrets are compared
without stopping at the first wrong byte. Every response forbids storing, referrers and framing,
and restricts the page to its own origin. `--public-url` names what a reverse proxy publishes: it
is the URL printed at start-up, and an HTTPS one marks the cookie `Secure`. `--no-agent` leaves
the agent routes unregistered and tells the page, which then offers no way to one. The server
stops on SIGTERM as well as on Ctrl-C. The API mirrors the session: state, file with highlighting
and change markers, blame, log, commit files, changes, diff with highlighting on both sides and
anchored comments, comments and notes (list, add, toggle, edit, delete), refresh and re-anchor,
the theme list and config, symbols (file symbols, definitions, occurrences, search, identifier at
a position), and the agent (start, poll for events since a sequence number, prompt with text or a
preset, answer a permission, cancel, stop). The server drains the agent into a buffer on every
poll, so a reloaded page rebuilds the transcript. The page has the same views as the terminal UI:
explorer with tree and file, changes, log and history, diff, review, notes, and the agent tab;
clicking a name in code offers its definition and usages in a side drawer. A comment on a whole
file comes from the Comment button in the file header and is shown above the first line; one on a
directory from the `+` on its row in the tree.

## Command line

`codereview [FILE]` opens the terminal UI, `web` the browser UI, `list` prints comments with
their current anchor state (`--all`, `--path`, `--json`), `add` and `note` write to the two
files (`add` takes `N` or `N-M` lines, or none at all for a comment on a whole file or
directory), `toggle` moves a comment between sections by its number in `list --all`, `reanchor`
rewrites line numbers, `def`, `refs` and `symbols` query the index, and `agent` sends one
prompt and streams the answer to stdout (permission requests are asked on the terminal, or
auto-allowed with `--yes`). `-C DIR` selects the repository.
