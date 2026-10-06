# codereview

Explore and review a repository from the terminal or a browser, and keep the review in two
Markdown files anyone can read. Built for codebases that change quickly, typically because
much of the code is written with AI assistance: comments survive the churn by remembering the
line they were left on and finding it again after edits.

- **Open a repository on what it demonstrably does**: its end-to-end tests, the ones that drive it
  the way a person does, each beside the recording its last run left — a video of the page, or the
  terminal played back — with whether it passed and how far the code has moved since. Navigation
  into the code starts from a test: its own view lines its steps up with the recording and the
  test's source, lists the code its run executed, what only it ran first, links the documentation it
  demonstrates, and the smaller tests that reach the same code. A repository with none opens on
  saying so, and the agent is offered the job of writing the first ones, each leaving a recording to
  watch.
- **Summarise a repository** rather than show a file tree: what its README says, where
  to start reading, which files the rest of the tree leans on — computed from the symbols, not
  declared anywhere — how it is doing, down to how many review comments have lost the lines
  they were left on, and its tests: what they say the system does, file by file, whether the
  last run is about the code on disk, and which of the files everything leans on no test names.
- **Explore** the working tree with syntax highlighting, change markers against HEAD, and blame;
  pictures are shown as pictures, in the browser and in a terminal that can draw them.
- **Comment** on a line or a range. Comments go to `REVIEW.md`, pending or completed, in the
  format [nvim-review](https://codeberg.org/ivankovic/nvim-review) uses, so both tools can share
  one file.
- **Note** things that are not tasks, per file or in general, in `NOTES.md`.
- **History** of every file, and the log of the whole repository, with a syntax-aware diff of
  any file in any commit, from the [omnidiff](https://github.com/ivankovic/omnidiff) engine:
  insertions, deletions, updates and moves, not just lines that happen to align.
- **Diff** the working tree or the index against HEAD, or any commit against the working tree.
- **Navigate by symbol**: jump to where the name under the cursor is defined, list every place
  it occurs, list a file's definitions, search definitions by name. Tree-sitter, no language
  server, every file in the repository, as fast as the tree changes.
- **Run the checks** the project already has — tests, coverage, a linter, `make check` — from
  the page, found in the repository when nothing is configured, and see what they said: the
  exit code and the tail of the output from the run, the test count from it or from the
  `junit.xml` they leave behind, coverage from `lcov.info`, and how many commits and changed
  files have come since. A repository can opt in to having them run on their own, a few
  seconds after it stops changing.
- **Talk to a coding agent**: ask about the code under the cursor, have one review comment
  addressed, have every pending comment worked through, or have a diff reviewed into
  `REVIEW.md`. Claude Code directly by default, no adapter; any agent that speaks the Agent
  Client Protocol (ACP) as well.

Rust, Python, TypeScript, HTML and Makefiles are the first-class languages. Everything omnidiff
and syntect know works as well; a language omnidiff has no grammar for gets a plain line diff.

## Installation

```
cargo install codereview                  # terminal UI
cargo install codereview --features web   # terminal and browser UI
```

You need a Rust toolchain (edition 2024, rustc 1.85 or later), a C compiler for the tree-sitter
grammars, and `git` on `PATH`.

## License

AGPL-3.0-or-later. Copyright 2026 Marko Ivankovic.

## Usage

```
codereview                    # terminal UI in the current repository
codereview src/main.rs        # ... opened at a file
codereview ~/src/a ~/src/b    # several repositories; gt/gT and W switch between them
codereview ~/src              # a directory of checkouts: every repository below it, and
                              # any cloned or created there later, within seconds
codereview web                # browser UI on a random loopback port, opens your browser
codereview web ~/src/a ~/src/b   # ... serving several; a dropdown in the header switches
codereview list               # pending comments, with their current line numbers
codereview list --all --json  # every comment, machine readable
codereview add src/lib.rs 12 "why is this pub?"
codereview add src/lib.rs 12-20 "duplicates the block above"
codereview add src/lib.rs "no tests for this module"        # the file as a whole
codereview add src "too many modules"                       # a directory
codereview note --path src/lib.rs "entry point for the parser"
codereview toggle 3           # complete (or reopen) comment number 3 from `list --all`
codereview reanchor           # rewrite line numbers of comments whose lines moved
codereview overview           # write OVERVIEW.md: what it is, where to start, what its tests check
codereview e2e                # the end-to-end tests and how their last recorded runs went
codereview e2e --test NAME    # one of them: its steps, the code it ran, its docs, tests below
codereview def total          # where `total` is defined
codereview refs total         # every place `total` occurs
codereview symbols pars       # definitions whose name contains "pars"
codereview agent "Summarise the pending comments in REVIEW.md"   # one prompt, answer on stdout
```

`-C DIR` runs against another repository; positional paths are relative to it. `web` also
takes `--port N` and `--no-open`; `add` takes `--author NAME` and `--no-timestamp`.

### Terminal UI

`?` shows every key. The essentials:

| Key | What |
| --- | --- |
| `Tab` | switch between the side panel and the file |
| `O` | side panel: the file tree, or the outline of the file (`Enter` goes to a definition) |
| `,` | options: side panels that hide while the file has the focus, diff layout, colour scheme |
| `Enter` | open the file; on a comment row, edit it; on `… N more`, the rest of the directory |
| `c` | comment on the line, or on the `V` selection; in the tree, on the whole file or directory |
| `x` `e` `d` `u` | complete, edit, delete a comment, undo the delete |
| `n` `N` | note on the file, general note |
| `H` | history of the file |
| `D` | diff the file against HEAD |
| `L` `S` `R` `T` | repository log, working tree changes, review list, notes list |
| `E` | end-to-end tests, what it opens on: `Space` plays, `Enter` a test on its own, `s` source |
| `U` | the tests that name the open file |
| `B` | blame column |
| `t` | colour scheme picker, previewed live and saved |
| `h` `l` `w` `b` `0` `$` | move the column cursor |
| `gd` or `Ctrl-]` | go to the definition of the name under the cursor |
| `gr` or `*` | every occurrence of the name under the cursor |
| `gs` or `@`, `gS` or `#` | symbols in this file; search symbols by name |
| `Ctrl-o` | back to where you were before a jump |
| `a` | ask the agent about the line, the selection, or the comment under the cursor |
| `i` | ask the agent something, from anywhere |
| `A` or `Ctrl-a` | the agent panel (`A` inside it: address every pending comment) |
| `gt` `gT` `W` | next / previous repository, the repository picker (when several are open) |
| `l` | in the agent panel: show or hide the backend's log lines |
| `/` `> <` | search, next and previous match |
| `o` | open the line in `$EDITOR` |
| `q` `Esc` | back |

In a diff: `n`/`p` step through hunks, `]`/`[` through the files of the same commit, `v` cycles
between automatic, side-by-side and unified layout, `c` comments on the after-side line.

### Browser UI

`codereview web` prints a URL that includes a session token and opens it. Only that URL works:
the server binds a loopback address and rejects requests without the token, so no other page
in the browser can read the repository or write to `REVIEW.md`. The token opens a session once
and leaves a cookie behind, so it does not stay in the address bar; API requests carry a
separate per-session secret in a header, which no other site can set.

For anything reachable from another machine, use a password instead of a token:

```sh
codereview hash-password                 # asks twice, prints an Argon2 hash
export CODEREVIEW_PASSWORD_HASH='$argon2id$...'
codereview web --public-url https://review.example.com --no-open
```

The page is then a sign-in form, a session lasts a week, **Sign out** ends it, and repeated
wrong passwords are answered more and more slowly. Binding a non-loopback address without a
password is refused outright.

To reach the page from another machine, put nginx in front of it rather than opening the port:
[docs/deploy.md](docs/deploy.md) has a TLS site on a custom domain, a systemd unit for serving
someone else's machine and a user unit for serving your own, `make deploy`, and what to
weigh first. Weigh it seriously: the tool is meant to change the repositories it serves —
comments, notes, commits, and an agent running commands in them — so whoever can sign in can do
all of that, and nothing turns it off.

Click a line number to comment, shift-click to extend the range (or the form's Extend range
button and another line number, on a phone); the Comment button in the file header comments
on the whole file, and the `+` on a directory in the tree on the directory. Everything the
terminal UI does is a button or a tab. Every view has an address, so the browser's back
returns to the previous view and a file, a diff or a commit can be linked to. The page keeps
itself current: when the agent, another window or a git command changes the repository, it
notices within a couple of seconds and redraws where you are, without moving the scroll.
The `Zen` button, or `z`, leaves the file alone on the screen. When the change reads well,
**Commit** in the changes view records it: the sheet names the files going in and any comment
on them still pending, and the repository's hooks run as they would in a terminal.

The page works on a phone: one pane at a time, the tabs along the bottom, and it can be added
to the home screen as an app. Review comments, diffs and the agent's transcript are what a
phone is good for; symbol navigation is there but is desk work.

### Symbols

Definitions and occurrences come from the tree-sitter grammars omnidiff already ships, for
every language it parses; Makefiles get their targets and variables by pattern. The index is
built the first time you ask (a few seconds on a large tree) and refreshed with `r`. It is
deliberately simple: a definition is any node with a name field whose kind ends in
`_definition`, `_declaration`, `_item` and the like, and an occurrence is any identifier with
the same text. That finds the right thing nearly always in a tree that changes daily, and never
needs a build to succeed first.

### The agent

By default codereview drives **Claude Code directly**, through its own headless streaming
protocol, the same one the Claude Agent SDK uses: `claude -p --input-format stream-json
--output-format stream-json`. Nothing but the `claude` binary is needed, and it uses your
existing login. Its replies stream into the panel as they are generated; the tools it runs
and their results are shown as they happen; every permission question Claude Code would ask
in its own terminal is shown to you with Allow, Always allow (which applies the rule Claude
Code suggests, for this session) and Deny. Nothing is approved on your behalf. `Esc`
interrupts a running turn. Your Claude Code settings apply as they would in its own terminal:
with `defaultMode` set to `auto` there, most actions are approved without asking, so put
`claude_args = ["--permission-mode", "default"]` in the config file if you want to be asked
every time while reviewing.

Any agent that speaks the **Agent Client Protocol** works too, with the same panel:

```toml
[agent]
kind = "acp"                 # "auto" (default): Claude Code if `claude` is on PATH, else ACP
command = "gemini"
args = ["--experimental-acp"]
claude_args = ["--model", "opus"]   # extra arguments for Claude Code when kind is "claude"
```

`claude_args` is appended after the flags codereview passes, so anything there wins: a
`--permission-mode` of your own decides what Claude Code asks about, including asking about
nothing. Treat the config file as something that chooses what runs on your machine, because
`command` and `args` do exactly that.

Over ACP, codereview answers the agent's file reads and writes itself: paths outside the
repository are refused, so are symlinks, and so is `.git`, where a hook would run on the next
git command. Zed's `claude-code-acp` adapter is the ACP fallback when `claude` is missing; it
needs Node 20 or newer. An agent is started in a process group of its own, so stopping it
stops whatever it started, and without this server's own secrets in its environment.

Presets word the common requests so both front ends ask the same way: address one comment
(and move it to Completed), address every pending comment, review a diff into `REVIEW.md`, or
a question about selected lines (sent with the lines included).

## Colour schemes and the config file

`t` in the terminal UI opens the scheme picker; the browser UI has a dropdown in its header.
The choice is saved to `~/.config/codereview/config.toml` (or `$XDG_CONFIG_HOME`, or the file
`$CODEREVIEW_CONFIG` names) and both front ends read it. Built in: Terminal, Dark, Light,
Catppuccin Latte and Mocha, Solarized Light and Dark, Gruvbox Light and Dark, Nord, Dracula.
The default, Terminal, paints nothing but your terminal's own sixteen colours, so it is
readable on any background; the others carry their own palette and a matching syntax theme.
The config file also remembers the diff layout (`v`) and whether comments get timestamps.

## The two files

`REVIEW.md`:

```markdown
# Pending
- [Marko] (2026-09-10 14:03:11) In src/main.rs on line 12: wrong error type - "let x = parse()?;"
- In src/lib.rs on line 3-7: duplicated below - "fn parse(" (at 3f2a9c1)
- In src/tui: too many screens

# Completed
- In README.md on line 1: typo
```

Author and timestamp are optional, and so is the `on line` part: a comment without it is about
the whole file or directory, and is shown above the file's first line. The quoted text at the
end is the line the comment was left on, and `(at 3f2a9c1)` after it, when there is one, is the
commit whose version of the file the line numbers count in: the commit of the diff the comment
was left in, or HEAD when the file is committed and unchanged. Such a comment is followed: the
syntax tree of the file at that commit is matched against the file now with omnidiff's AST
diff, and the comment goes where the code on its line went, through moves and edits and lines
added above it. Any other comment is looked
for by its text: if it is on the recorded line, the comment is exact; if it is elsewhere, the
comment follows it and is shown as moved; if it is gone, the comment is shown as stale.
`codereview reanchor` writes the new line numbers back. Anything else in the file, prose or
custom headings, is preserved.

`NOTES.md`:

```markdown
# Notes

## General
- The crate is split by front end; the core never depends on either.

## src/repo.rs
- Talks to the git binary on purpose, see PLAN.md.
```

`OVERVIEW.md` is the third file, and the tool writes it: what the repository is, as of which
commit, where to start reading, what the rest of the tree leans on, every test as a sentence
under the file it is in, and the leaned-on files no test names. It is rewritten when the
repository has held still for ten seconds and something changed, and read back when the
repository is opened, so the overview page is there at once while the symbol index is built.
An `## About` section is yours, or the agent's — "Explain this repository" writes there — and
so is any heading of your own; the tool keeps them as they are.

## Development

`make check` runs rustfmt, clippy with `-D warnings` on every feature set, and the tests
through [cargo-nextest](https://nexte.st) (`cargo install cargo-nextest --locked`).

The end-to-end tests are where a review of this repository starts, as the tool says a review
of any repository should: each is one thing a person does, named as a sentence, and each leaves a
recording of itself under `target/e2e/` — an asciicast for the terminal UI, a video for the page
— with a `test.json` listing its steps against the recording. `tests/terminal.rs` runs the real
binary in a pseudo-terminal and is part of `make check`; `tests/browser/phone.js` drives the page
in a headless Chromium at a phone's size and needs podman. `make e2e` runs both;
`make e2e-coverage` runs them built to count what they run, so that each test also leaves the
code it ran (it needs cargo-llvm-cov and `rustup component add llvm-tools`).

`make perf` times every request the page makes against a clone of this repository, cold and
then warm, and fails on any that takes over 400 ms: the pause between a tap and the screen
changing. It builds in release, since a debug build would time the compiler, and prints its
table either way.

`make integration-test` runs the browser UI behind a real nginx, in a container, over TLS on a
name: the site from [docs/deploy.md](docs/deploy.md), signing in with a password, reading and
writing through the proxy. `make browser-test` runs the page's end-to-end tests in a headless
Chromium the size of a telephone, which are the only tests here that can see a layout. Both need
podman, which is why neither is part of `make check`. Rootless podman is enough, and is the
point: no daemon, so running the suite costs nobody access to every other container on the
machine. `CODEREVIEW_CONTAINER_RUNTIME` drives another runtime — `docker` works as a drop-in.

PLAN.md has the design and the roadmap, SPECS.md what the tool does in detail.
