# codereview

Explore and review a repository from the terminal or a browser, and keep the review in two
Markdown files anyone can read without the tool. Built for codebases that change quickly,
typically because much of the code is written with AI assistance: comments survive the churn
by remembering the line they were left on and finding it again after edits.

- **Explore** the working tree with syntax highlighting, change markers against HEAD, and blame.
- **Comment** on a line or a range. Comments go to `REVIEW.md`, pending or completed, in the
  format [nvim-review](https://codeberg.org/ivankovic/nvim-review) uses, so both tools can share
  one file.
- **Note** things that are not tasks, per file or in general, in `NOTES.md`.
- **History** of every file, and the log of the whole repository, with a syntax-aware diff of
  any file in any commit, from the [codediff](https://github.com/ivankovic/codediff) engine:
  insertions, deletions, updates and moves, not just lines that happen to align.
- **Diff** the working tree or the index against HEAD, or any commit against the working tree.
- **Navigate by symbol**: jump to where the name under the cursor is defined, list every place
  it occurs, list a file's definitions, search definitions by name. Tree-sitter, no language
  server, every file in the repository, as fast as the tree changes.
- **Talk to a coding agent**: ask about the code under the cursor, have one review comment
  addressed, have every pending comment worked through, or have a diff reviewed into
  `REVIEW.md`. Claude Code directly by default, no adapter; any agent that speaks the Agent
  Client Protocol (ACP) as well.

Rust, Python, TypeScript, HTML and Makefiles are the first-class languages. Everything codediff
and syntect know works as well; a language codediff has no grammar for gets a plain line diff.

## Installation

```
cargo install codereview                  # terminal UI
cargo install codereview --features web   # terminal and browser UI
```

You need a Rust toolchain (edition 2024, rustc 1.85 or later), a C compiler for the tree-sitter
grammars, and `git` on `PATH`.

## Usage

```
codereview                    # terminal UI in the current repository
codereview src/main.rs        # ... opened at a file
codereview ~/src/a ~/src/b    # several repositories; gt/gT and W switch between them
codereview ~/src              # a directory of checkouts: every repository below it
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
| `Tab` | switch between the file tree and the file |
| `Enter` | open the file; on a comment row, edit it |
| `c` | comment on the line, or on the `V` selection; in the tree, on the whole file or directory |
| `x` `e` `d` `u` | complete, edit, delete a comment, undo the delete |
| `n` `N` | note on the file, general note |
| `H` | history of the file |
| `D` | diff the file against HEAD |
| `L` `S` `R` `T` | repository log, working tree changes, review list, notes list |
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
codereview web --public-url https://review.example.com --no-agent --no-open
```

The page is then a sign-in form, a session lasts a week, **Sign out** ends it, and repeated
wrong passwords are answered more and more slowly. Binding a non-loopback address without a
password is refused outright.

To reach the page from another machine, put nginx in front of it rather than opening the port:
[docs/deploy.md](docs/deploy.md) has a TLS site on a custom domain, a systemd unit, and what to
weigh first. `--no-agent` serves everything except the agent, which is the right default for
anything reachable from outside the machine, since an agent runs commands in the repository.

Click a line number to comment, shift-click to extend the range; the Comment button in the
file header comments on the whole file, and the `+` on a directory in the tree on the
directory. Everything the terminal UI does is a button or a tab.

### Symbols

Definitions and occurrences come from the tree-sitter grammars codediff already ships, for
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

Over ACP, codereview answers the agent's file reads and writes itself and refuses paths
outside the repository. Zed's `claude-code-acp` adapter is the ACP fallback when `claude` is
missing; it needs Node 20 or newer.

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
- In src/lib.rs on line 3-7: duplicated below
- In src/tui: too many screens

# Completed
- In README.md on line 1: typo
```

Author and timestamp are optional, and so is the `on line` part: a comment without it is about
the whole file or directory, and is shown above the file's first line. The quoted text at the
end is the line the comment was left on. When the file changes, the tool looks for that text:
if it is on the recorded line, the comment is exact; if it is elsewhere, the comment follows
it and is shown as moved; if it is gone, the comment is shown as stale. `codereview reanchor`
writes the new line numbers back. Anything else in the file, prose or custom headings, is
preserved.

`NOTES.md`:

```markdown
# Notes

## General
- The crate is split by front end; the core never depends on either.

## src/repo.rs
- Talks to the git binary on purpose, see PLAN.md.
```

## Development

`make check` runs rustfmt, clippy with `-D warnings` on every feature set, and the tests
through [cargo-nextest](https://nexte.st) (`cargo install cargo-nextest --locked`).
PLAN.md has the design and the roadmap, SPECS.md what the tool does in detail.

## License

AGPL-3.0-or-later. Copyright 2026 Marko Ivankovic.
