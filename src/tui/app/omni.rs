//! One file of a review range, in omnidiff's own diff view: the two sides matched node by node,
//! the minimap, change by change with `n` and `p`. omnidiff draws it and moves through it; this
//! screen keeps what belongs to the review around it — the other files of the range (`]`, `[`),
//! a comment on the line under the cursor (`c`), a search (`/`), `$EDITOR` (`ge`) and the way
//! back (`q`).
//!
//! A child module of `app`, so it goes on using the app's own methods.

use std::path::Path;

use omnidiff::tui::embed::{DiffView, Outcome, OverlayTheme};

use super::*;

pub struct OmniState {
    pub view: DiffView,
    pub target: DiffTarget,
    /// The range's files, of which this is `index`, and each one's size.
    pub files: Vec<ChangedFile>,
    pub sizes: Vec<Option<omnidiff::Stats>>,
    pub index: usize,
    /// The after side's lines, for the text a comment records.
    pub after_lines: Vec<String>,
}

impl OmniState {
    pub fn file(&self) -> &ChangedFile {
        &self.files[self.index]
    }
}

impl App {
    /// File `index` of `files` in omnidiff's view; a file that is not text in the plain diff,
    /// which says so.
    pub(super) fn push_omni(
        &mut self,
        target: DiffTarget,
        files: Vec<ChangedFile>,
        sizes: Vec<Option<omnidiff::Stats>>,
        index: usize,
    ) {
        let Some(file) = files.get(index).cloned() else {
            return;
        };
        let texts = match self.session.diff_texts(&target, &file) {
            Ok(Some(texts)) => texts,
            Ok(None) => {
                self.push_diff(target, files, index);
                return;
            }
            Err(e) => {
                self.error(format!("{e:#}"));
                return;
            }
        };
        let (before, after) = texts;
        let old_path = file.old_path.clone().unwrap_or_else(|| file.path.clone());
        let mut view = match DiffView::from_texts(
            Path::new(&old_path),
            &before,
            Path::new(&file.path),
            &after,
        ) {
            Ok(view) => view,
            Err(e) => {
                self.error(format!("{e:#}"));
                return;
            }
        };
        view.set_overlay_theme(if self.theme.dark {
            OverlayTheme::default()
        } else {
            OverlayTheme::SolarizedLight
        });
        view.set_syntax_theme(&self.theme.syntax);
        // What a review comments on, and the only side an added file has.
        view.focus_side(omnidiff::Side::After);
        let state = OmniState {
            view,
            target,
            files,
            sizes,
            index,
            after_lines: after.lines().map(str::to_string).collect(),
        };
        self.screens.push(Screen::Omni(Box::new(state)));
    }

    /// Another file of the same range, in place of this one.
    fn replace_omni(&mut self, index: usize) {
        let Some(Screen::Omni(state)) = self.screens.last() else {
            return;
        };
        if index >= state.files.len() {
            self.info(if index == 0 {
                "first file"
            } else {
                "last file"
            });
            return;
        }
        let (target, files, sizes) = (
            state.target.clone(),
            state.files.clone(),
            state.sizes.clone(),
        );
        self.screens.pop();
        self.push_omni(target, files, sizes, index);
    }

    pub(super) fn handle_omni_key(&mut self, key: KeyEvent) {
        let Some(Screen::Omni(state)) = self.screens.last_mut() else {
            return;
        };
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => self.pop_screen(),
            KeyCode::Char(']') => {
                let next = state.index + 1;
                self.replace_omni(next);
            }
            KeyCode::Char('[') => match state.index.checked_sub(1) {
                Some(previous) => self.replace_omni(previous),
                None => self.info("first file"),
            },
            KeyCode::Char('c') => self.start_comment(),
            KeyCode::Char('/') => {
                self.prompt = Some(Prompt::new(PromptKind::Search, "search", ""));
            }
            KeyCode::Char('>') => state.view.next_match(true),
            KeyCode::Char('<') => state.view.next_match(false),
            // `ge`: the after side's line in `$EDITOR`.
            KeyCode::Char('o') => {
                let line = match state.view.cursor() {
                    Some((omnidiff::Side::After, row)) => row + 1,
                    _ => 1,
                };
                let path = self.session.root().join(&state.file().path);
                self.editor_request = Some((path, line));
            }
            _ => {
                if state.view.handle_key(key) == Outcome::Ignored {
                    self.info("q back  ] [ files  c comment  / search  n p changes  v layout");
                }
            }
        }
    }
}
