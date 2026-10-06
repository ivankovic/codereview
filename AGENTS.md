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
  needs podman (rootless is enough; `CODEREVIEW_CONTAINER_RUNTIME` drives another runtime);
  `make check` does not. `tests/nginx/site.conf` mirrors the site in
  `docs/deploy.md`, and the test fails if the document loses a directive it exercises.
- End-to-end tests are what the tool, and a review of this repository, turn around. A change
  someone could see — a new view, key, button or behaviour — comes with an end-to-end test of
  it, for both front ends: `tests/terminal.rs` drives the binary in a pseudo-terminal,
  `tests/browser/phone.js` the page in a real browser. One behaviour per test, named as the
  sentence it demonstrates, with `step` saying what a person would be doing, because each test
  leaves a recording a reviewer watches with those steps beside it (`target/e2e/`).
- `make browser-test` opens the page in a headless Chromium at 360x640 and taps things.
  **Run it after any change to `src/web/index.html`.** Everything else in the suite reads what
  the server sends, which is exactly the check that passes while the page is unusable: a menu
  clipped by its parent's `overflow`, a header that stopped sticking, buttons under the
  keyboard. The tests live in `tests/browser/phone.js`; add a check when you fix a layout bug,
  and prove it by reintroducing the bug and watching it fail. Ask what a finger would hit
  (`elementFromPoint`) rather than where an element says it is: a rect is unmoved by a parent
  that clips it.
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
