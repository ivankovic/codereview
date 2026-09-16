# Development workflow

- Read README.md before anything else. Read PLAN.md before changing the shape of the project.
- SPECS.md describes what the tool does and how its files are laid out. Update it with every
  change in behaviour. It must not contain code snippets.
- TODO.md and REVIEW.md list open work. Remove an item when you finish it.
- `make check` (fmt, clippy with `-D warnings` on every feature set, tests) must pass before a
  commit.
- Tests run through `cargo nextest run`, never plain `cargo test`. Use it for targeted runs too,
  for example `cargo nextest run --features web -E 'test(tui::)'`.
- Both front ends build the agent transcript with `src/transcript.rs`. The page renders what
  the server wrote; do not give it a state machine of its own again.
- `make integration-test` puts a real nginx in front of the browser UI, in a container. It
  needs docker; `make check` does not. `tests/nginx/site.conf` mirrors the site in
  `docs/deploy.md`, and the test fails if the document loses a directive it exercises.
- Markdown lines wrap at 100 columns.

# Rust

- Prefer Ratatui's `Stylize` helpers (`"text".dim()`, `.bold()`, `.cyan()`) over manual `Style`
  values, and `"text".into()` / `vec![..].into()` over `Span::from` / `Line::from` where the
  target type is obvious.
- No `.white()`: use the default foreground.
- Anything that talks to git goes through `repo.rs`; anything that touches REVIEW.md or NOTES.md
  goes through `review.rs` / `notes.rs`. The front ends never read those files themselves.
- Both front ends consume the same paint model from `diff.rs` and `highlight.rs`. Do not add
  rendering logic that only one of them can use to the core.
