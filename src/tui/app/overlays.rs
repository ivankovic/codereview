//! What sits over a screen: the prompt line at the bottom, and the colour scheme picker.
//! Both take every key while they are up, and both are answered here rather than in the
//! screen underneath.
//!
//! A child module of `app`, so it goes on using the app's own methods.

use super::*;

impl App {
    pub(super) fn rehighlight(&mut self) {
        let syntax = self.theme.syntax.clone();
        if let Some(v) = &mut self.viewer {
            v.rehighlight(&syntax);
        }
        for screen in &mut self.screens {
            if let Screen::Diff(d) = screen {
                d.view.rehighlight(&syntax);
            }
        }
    }

    pub(super) fn apply_theme(&mut self, theme: Theme) {
        if theme.syntax != self.theme.syntax {
            self.theme = theme;
            self.rehighlight();
        } else {
            self.theme = theme;
        }
    }

    /// Takes over the theme another repository's app saved.
    pub fn adopt_theme(&mut self, theme: Theme) {
        self.session.adopt_theme(theme.clone());
        self.apply_theme(theme);
    }

    /// Takes over the diff layout another repository's app saved.
    pub fn adopt_layout(&mut self, layout: DiffLayout) {
        self.layout = layout;
        self.session.adopt_layout(layout.name());
    }

    pub(super) fn open_theme_picker(&mut self) {
        let index = crate::theme::all()
            .iter()
            .position(|t| t.name == self.theme.name)
            .unwrap_or(0);
        self.theme_picker = Some((index, self.theme.clone()));
    }

    pub(super) fn handle_theme_picker_key(&mut self, key: KeyEvent) {
        let Some((index, previous)) = self.theme_picker.clone() else {
            return;
        };
        let themes = crate::theme::all();
        let select = |app: &mut App, i: usize| {
            let i = i.min(themes.len() - 1);
            app.theme_picker = Some((i, previous.clone()));
            app.apply_theme(themes[i].clone());
        };
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => select(self, index + 1),
            KeyCode::Char('k') | KeyCode::Up => select(self, index.saturating_sub(1)),
            KeyCode::Char('g') | KeyCode::Home => select(self, 0),
            KeyCode::Char('G') | KeyCode::End => select(self, themes.len() - 1),
            KeyCode::Enter => {
                let name = self.theme.name.clone();
                self.theme_picker = None;
                let outcome = self.session.set_theme(&name);
                if self.report(outcome).is_some() {
                    self.info(format!("theme {name} saved to the config file"));
                }
            }
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('t') => {
                self.theme_picker = None;
                self.apply_theme(previous);
            }
            _ => {}
        }
    }

    pub(super) fn submit_prompt(&mut self) {
        let Some(prompt) = self.prompt.take() else {
            return;
        };
        let text = prompt.text.clone();
        match prompt.kind {
            PromptKind::Comment { path, lines } => {
                if text.trim().is_empty() {
                    self.info("empty comment discarded");
                    return;
                }
                let source = match (self.screen(), lines) {
                    (Screen::Diff(d), Some((line, _))) => d.view.after_lines.get(line - 1).cloned(),
                    _ => None,
                };
                let outcome = self
                    .session
                    .add_comment(&path, lines, &text, source.as_deref());
                if self.report(outcome).is_some() {
                    if let Some(v) = &mut self.viewer {
                        v.visual = None;
                    }
                    self.after_comment_change();
                    self.info("comment added to REVIEW.md");
                }
            }
            PromptKind::EditComment => {
                if let Some(EditTarget::Comment(c)) = self.edit_target.take() {
                    if text.trim().is_empty() {
                        self.info("empty comment; use d to delete");
                        return;
                    }
                    let outcome = self.session.edit_comment(&c, &text);
                    if self.report(outcome).is_some() {
                        self.after_comment_change();
                        self.info("comment updated");
                    }
                }
            }
            PromptKind::Note { path } => {
                if text.trim().is_empty() {
                    self.info("empty note discarded");
                    return;
                }
                let outcome = self.session.add_note(path.as_deref(), &text);
                if self.report(outcome).is_some() {
                    self.refresh_notes();
                    self.info("note added to NOTES.md");
                }
            }
            PromptKind::EditNote => {
                if let Some(EditTarget::Note(n)) = self.edit_target.take() {
                    if text.trim().is_empty() {
                        return;
                    }
                    let outcome = self.session.edit_note(&n, &text);
                    if self.report(outcome).is_some() {
                        self.refresh_notes();
                        self.info("note updated");
                    }
                }
            }
            PromptKind::Search => {
                let query = if text.is_empty() {
                    self.last_search.clone()
                } else {
                    text
                };
                self.last_search = query.clone();
                let n = match self.screen_mut() {
                    Screen::Diff(d) => {
                        let n = d.view.set_search(&query);
                        d.view.next_match(true);
                        n
                    }
                    _ => match &mut self.viewer {
                        Some(v) => {
                            let n = v.set_search(&query);
                            v.nearest_match();
                            n
                        }
                        None => 0,
                    },
                };
                self.info(format!("{n} match(es) for {query:?}"));
            }
            PromptKind::GoToLine => {
                if let Ok(n) = text.trim().parse::<usize>() {
                    match self.screen_mut() {
                        Screen::Diff(d) => {
                            d.view.go_to_after_line(n.saturating_sub(1));
                        }
                        _ => {
                            if let Some(v) = &mut self.viewer {
                                v.go_to_line(n.saturating_sub(1));
                            }
                        }
                    }
                }
            }
            PromptKind::Filter { .. } => {
                self.tree.filter = text;
                self.tree.rebuild();
            }
            PromptKind::Agent { context } => {
                if text.trim().is_empty() {
                    return;
                }
                self.ask_agent(text, context);
            }
            PromptKind::SymbolSearch => {
                if !text.trim().is_empty() {
                    self.symbol_search(text.trim());
                }
            }
        }
    }

    pub(super) fn handle_prompt_key(&mut self, key: KeyEvent) {
        let Some(p) = &mut self.prompt else {
            return;
        };
        match (key.code, key.modifiers) {
            (KeyCode::Esc, _) | (KeyCode::Char('c'), KeyModifiers::CONTROL) => {
                // A filter applies as it is typed, so cancelling has to undo it.
                if let PromptKind::Filter { was } = &p.kind {
                    self.tree.filter = was.clone();
                    self.prompt = None;
                    self.tree.rebuild();
                } else {
                    self.prompt = None;
                }
                self.edit_target = None;
            }
            (KeyCode::Enter, _) => self.submit_prompt(),
            (KeyCode::Char('u'), KeyModifiers::CONTROL) => {
                p.text.clear();
                p.cursor = 0;
            }
            (KeyCode::Char('w'), KeyModifiers::CONTROL) => p.delete_word(),
            (KeyCode::Char('a'), KeyModifiers::CONTROL) | (KeyCode::Home, _) => p.home(),
            (KeyCode::Char('e'), KeyModifiers::CONTROL) | (KeyCode::End, _) => p.end(),
            (KeyCode::Left, _) => p.left(),
            (KeyCode::Right, _) => p.right(),
            (KeyCode::Backspace, _) => {
                p.backspace();
                if matches!(p.kind, PromptKind::Filter { .. }) {
                    self.tree.filter = p.text.clone();
                    self.tree.rebuild();
                }
            }
            (KeyCode::Delete, _) => p.delete(),
            (KeyCode::Char(c), m) if !m.contains(KeyModifiers::CONTROL) => {
                p.insert(c);
                if matches!(p.kind, PromptKind::Filter { .. }) {
                    self.tree.filter = p.text.clone();
                    self.tree.rebuild();
                }
            }
            _ => {}
        }
    }
}
