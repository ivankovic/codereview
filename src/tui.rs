//! The terminal front end: terminal setup, the event loop, and `$EDITOR` hand-off.

pub mod agent_panel;
pub mod app;
pub mod prompt;
pub mod style;
pub mod tree;
pub mod ui;
pub mod viewer;
pub mod workspace;

use std::io::stdout;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

use anyhow::{Context, Result};
use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyEventKind, MouseEventKind,
};
use crossterm::execute;

use crate::session::Session;
use workspace::Workspace;

/// Runs the terminal UI on one or more repositories, each with a file to open first.
pub fn run(targets: Vec<(Session, Option<String>)>) -> Result<()> {
    anyhow::ensure!(!targets.is_empty(), "no repository to open");
    let mut ws = Workspace::new(targets);
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = execute!(stdout(), DisableMouseCapture);
        ratatui::restore();
        hook(info);
    }));
    let mut terminal = ratatui::init();
    execute!(stdout(), EnableMouseCapture).context("enable mouse")?;
    let result = event_loop(&mut terminal, &mut ws);
    let _ = execute!(stdout(), DisableMouseCapture);
    ratatui::restore();
    result
}

fn event_loop(terminal: &mut ratatui::DefaultTerminal, ws: &mut Workspace) -> Result<()> {
    loop {
        terminal.draw(|frame| ui::draw_workspace(frame, ws))?;
        if ws.quit() {
            return Ok(());
        }
        if let Some((path, line)) = ws.app_mut().editor_request.take() {
            let app = ws.app_mut();
            let _ = execute!(stdout(), DisableMouseCapture);
            ratatui::restore();
            let outcome = open_editor(&path, line);
            *terminal = ratatui::init();
            let _ = execute!(stdout(), EnableMouseCapture);
            match outcome {
                Ok(()) => app.refresh_all(),
                Err(e) => app.error(format!("{e:#}")),
            }
            continue;
        }
        let wait = if ws.agent_active() { 40 } else { 250 };
        if !event::poll(Duration::from_millis(wait))? {
            ws.tick();
            continue;
        }
        match event::read()? {
            Event::Key(key) if matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) => {
                ws.handle_key(key);
                ws.tick();
            }
            Event::Mouse(m) => match m.kind {
                MouseEventKind::ScrollDown => ws.app_mut().scroll_main(3),
                MouseEventKind::ScrollUp => ws.app_mut().scroll_main(-3),
                _ => {}
            },
            _ => {}
        }
    }
}

/// Runs `$VISUAL` or `$EDITOR` (default `vi`) on `path` at `line`, in the foreground.
fn open_editor(path: &Path, line: usize) -> Result<()> {
    let editor = std::env::var("VISUAL")
        .or_else(|_| std::env::var("EDITOR"))
        .unwrap_or_else(|_| "vi".to_string());
    let mut parts = editor.split_whitespace();
    let program = parts.next().unwrap_or("vi");
    let mut cmd = Command::new(program);
    cmd.args(parts);
    let base = Path::new(program)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(program);
    if matches!(
        base,
        "vi" | "vim" | "nvim" | "nano" | "emacs" | "hx" | "micro" | "kak"
    ) {
        cmd.arg(format!("+{line}"));
    } else if matches!(base, "code" | "codium" | "subl" | "zed") {
        cmd.arg("--goto").arg(format!("{}:{line}", path.display()));
        let status = cmd
            .status()
            .with_context(|| format!("cannot run {editor}"))?;
        anyhow::ensure!(status.success(), "{editor} exited with {status}");
        return Ok(());
    }
    cmd.arg(path);
    let status = cmd
        .status()
        .with_context(|| format!("cannot run {editor}"))?;
    anyhow::ensure!(status.success(), "{editor} exited with {status}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::app::{App, Screen};
    use crate::session::tests::{scratch_repo, scratch_session};
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn key(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
    }

    fn press(app: &mut App, keys: &[KeyCode]) {
        for k in keys {
            app.handle_key(KeyEvent::new(*k, KeyModifiers::NONE));
        }
    }

    fn draw(term: &mut Terminal<TestBackend>, app: &mut App) -> String {
        term.draw(|f| super::ui::draw(f, app)).unwrap();
        let buffer = term.backend().buffer().clone();
        let mut out = String::new();
        for y in 0..buffer.area.height {
            for x in 0..buffer.area.width {
                out.push_str(buffer[(x, y)].symbol());
            }
            out.push('\n');
        }
        out
    }

    #[test]
    fn comment_on_a_whole_file_from_the_tree() {
        let dir = scratch_repo();
        let session = scratch_session(dir.path());
        let mut app = App::new(session, None);
        let mut term = Terminal::new(TestBackend::new(120, 30)).unwrap();
        draw(&mut term, &mut app);

        // With the tree focused, c comments on the selected path as a whole.
        press(&mut app, &[KeyCode::Char('j'), KeyCode::Char('c')]);
        assert!(app.prompt.is_some());
        for ch in "needs tests".chars() {
            app.handle_key(key(ch));
        }
        press(&mut app, &[KeyCode::Enter]);
        let comment = app.session.review.comments()[0].clone();
        assert_eq!((comment.path.as_str(), comment.line), ("a.rs", None));

        // It heads the file, above line 1, and the review list shows it without a line.
        press(&mut app, &[KeyCode::Enter]);
        let screen = draw(&mut term, &mut app);
        let (before, after) = screen.split_once("needs tests").unwrap();
        assert!(!before.contains("fn a() {}") && after.contains("fn a() {}"));
        press(&mut app, &[KeyCode::Char('R')]);
        let screen = draw(&mut term, &mut app);
        assert!(screen.contains("◆ a.rs Tester: needs tests"));
    }

    #[test]
    fn a_diff_opened_from_the_tree_still_comments_on_lines() {
        let dir = scratch_repo();
        let session = scratch_session(dir.path());
        let mut app = App::new(session, None);
        let mut term = Terminal::new(TestBackend::new(120, 30)).unwrap();
        draw(&mut term, &mut app);

        // D from the tree keeps the tree focused; c must still take the after-side line.
        press(&mut app, &[KeyCode::Char('j'), KeyCode::Char('D')]);
        assert!(matches!(app.screen(), Screen::Diff(_)));
        draw(&mut term, &mut app);
        press(&mut app, &[KeyCode::Char('c')]);
        for ch in "on a line".chars() {
            app.handle_key(key(ch));
        }
        press(&mut app, &[KeyCode::Enter]);
        let comment = app.session.review.comments()[0].clone();
        assert_eq!(comment.path, "a.rs");
        assert!(comment.line.is_some(), "{comment:?}");
    }

    #[test]
    fn walk_every_screen() {
        let dir = scratch_repo();
        let session = scratch_session(dir.path());
        let mut app = App::new(session, None);
        let mut term = Terminal::new(TestBackend::new(120, 30)).unwrap();
        let screen = draw(&mut term, &mut app);
        assert!(screen.contains("a.rs"));
        assert!(screen.contains("Makefile"));

        // Open a.rs, comment on line 2, then mark it done.
        press(&mut app, &[KeyCode::Char('j'), KeyCode::Enter]);
        assert!(!app.focus_tree);
        let screen = draw(&mut term, &mut app);
        assert!(screen.contains("fn a() {}"));
        press(&mut app, &[KeyCode::Char('j'), KeyCode::Char('c')]);
        assert!(app.prompt.is_some());
        for ch in "needs a doc comment".chars() {
            app.handle_key(key(ch));
        }
        press(&mut app, &[KeyCode::Enter]);
        assert!(app.prompt.is_none());
        assert_eq!(app.session.review.pending_count(), 1);
        let screen = draw(&mut term, &mut app);
        assert!(screen.contains("needs a doc comment"));
        assert!(screen.contains("●"));
        // Move onto the comment row and toggle it.
        press(&mut app, &[KeyCode::Char(']'), KeyCode::Char('x')]);
        assert_eq!(app.session.review.pending_count(), 0);
        press(&mut app, &[KeyCode::Char('x')]);
        assert_eq!(app.session.review.pending_count(), 1);

        // Search and blame.
        press(&mut app, &[KeyCode::Char('/')]);
        for ch in "fn b".chars() {
            app.handle_key(key(ch));
        }
        press(&mut app, &[KeyCode::Enter]);
        assert_eq!(app.viewer.as_ref().unwrap().current_line(), Some(1));
        press(&mut app, &[KeyCode::Char('b')]);
        let screen = draw(&mut term, &mut app);
        assert!(screen.contains("Tester"));
        press(&mut app, &[KeyCode::Char('b')]);

        // Diff against HEAD (a.rs is modified), unified and side by side.
        press(&mut app, &[KeyCode::Char('D')]);
        assert!(matches!(app.screen(), Screen::Diff(_)));
        let screen = draw(&mut term, &mut app);
        assert!(screen.contains("working tree"));
        assert!(screen.contains("fn c() {}"));
        press(&mut app, &[KeyCode::Char('v'), KeyCode::Char('v')]);
        let screen = draw(&mut term, &mut app);
        assert!(screen.contains("unified"));
        press(&mut app, &[KeyCode::Char('c')]);
        for ch in "new fn".chars() {
            app.handle_key(key(ch));
        }
        press(&mut app, &[KeyCode::Enter]);
        assert_eq!(app.session.review.pending_count(), 2);
        press(&mut app, &[KeyCode::Char('q')]);
        assert!(matches!(app.screen(), Screen::Explorer));

        // History of the file, then the diff of its first commit.
        press(&mut app, &[KeyCode::Char('H')]);
        assert!(matches!(app.screen(), Screen::Log(_)));
        let screen = draw(&mut term, &mut app);
        assert!(screen.contains("history of"));
        assert!(screen.contains("second"));
        press(&mut app, &[KeyCode::Enter]);
        assert!(matches!(app.screen(), Screen::Diff(_)));
        let screen = draw(&mut term, &mut app);
        assert!(screen.contains("fn b() {}"));
        press(
            &mut app,
            &[
                KeyCode::Char(']'),
                KeyCode::Char('['),
                KeyCode::Char('q'),
                KeyCode::Char('q'),
            ],
        );

        // Repository log, files pane, diff.
        press(&mut app, &[KeyCode::Char('L')]);
        let screen = draw(&mut term, &mut app);
        assert!(screen.contains("commit"));
        press(
            &mut app,
            &[KeyCode::Enter, KeyCode::Char('j'), KeyCode::Enter],
        );
        assert!(matches!(app.screen(), Screen::Diff(_)));
        draw(&mut term, &mut app);
        press(
            &mut app,
            &[KeyCode::Char('q'), KeyCode::Char('q'), KeyCode::Char('q')],
        );

        // Changes, review, notes.
        press(&mut app, &[KeyCode::Char('S')]);
        let screen = draw(&mut term, &mut app);
        assert!(screen.contains("new.py"));
        press(&mut app, &[KeyCode::Char('s')]);
        // Nothing staged: the screen is replaced by a message and popped.
        assert!(matches!(app.screen(), Screen::Explorer));
        press(&mut app, &[KeyCode::Char('R')]);
        let screen = draw(&mut term, &mut app);
        assert!(screen.contains("needs a doc comment"));
        press(&mut app, &[KeyCode::Char('j'), KeyCode::Char('d')]);
        assert_eq!(app.session.review.pending_count(), 1);
        press(&mut app, &[KeyCode::Char('u')]);
        assert_eq!(app.session.review.pending_count(), 2);
        press(&mut app, &[KeyCode::Enter]);
        assert!(matches!(app.screen(), Screen::Explorer));
        press(&mut app, &[KeyCode::Char('N')]);
        for ch in "tiny".chars() {
            app.handle_key(key(ch));
        }
        press(&mut app, &[KeyCode::Enter, KeyCode::Char('T')]);
        let screen = draw(&mut term, &mut app);
        assert!(screen.contains("General: tiny"));
        press(&mut app, &[KeyCode::Char('?')]);
        let screen = draw(&mut term, &mut app);
        assert!(screen.contains("keys"));
        press(
            &mut app,
            &[KeyCode::Esc, KeyCode::Char('q'), KeyCode::Char('q')],
        );
        assert!(!app.quit);
        press(&mut app, &[KeyCode::Char('q')]);
        assert!(app.quit);
    }

    #[test]
    fn symbol_navigation() {
        let dir = scratch_repo();
        std::fs::write(
            dir.path().join("a.rs"),
            "fn a() {}\nfn b() { a(); }\nfn c() { b(); a(); }\n",
        )
        .unwrap();
        let session = scratch_session(dir.path());
        let mut app = App::new(session, Some("a.rs".into()));
        let mut term = Terminal::new(TestBackend::new(100, 20)).unwrap();
        draw(&mut term, &mut app);
        // Line 2, on the call to `a`: gd jumps to line 1.
        press(
            &mut app,
            &[KeyCode::Char('j'), KeyCode::Char('w'), KeyCode::Char('w')],
        );
        assert_eq!(app.viewer.as_ref().unwrap().column(), 9);
        press(&mut app, &[KeyCode::Char('g'), KeyCode::Char('d')]);
        let v = app.viewer.as_ref().unwrap();
        assert_eq!((v.current_line(), v.column()), (Some(0), 3));
        // Ctrl-o returns.
        app.handle_key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::CONTROL));
        assert_eq!(app.viewer.as_ref().unwrap().current_line(), Some(1));
        // `gr` is a chord, not a refresh: it lists every occurrence, as `*` does.
        press(&mut app, &[KeyCode::Char('g'), KeyCode::Char('r')]);
        assert!(
            matches!(app.screen(), Screen::Locations(l) if l.items.items.len() == 3),
            "gr must open the locations screen"
        );
        press(&mut app, &[KeyCode::Char('q')]);
        press(&mut app, &[KeyCode::Char('g'), KeyCode::Char('S')]);
        assert!(matches!(
            app.prompt.as_ref().map(|p| &p.kind),
            Some(crate::tui::prompt::PromptKind::SymbolSearch)
        ));
        press(&mut app, &[KeyCode::Esc]);
        // `*` lists every occurrence; Enter on the last one jumps there.
        press(&mut app, &[KeyCode::Char('*')]);
        assert!(matches!(app.screen(), Screen::Locations(l) if l.items.items.len() == 3));
        let screen = draw(&mut term, &mut app);
        assert!(screen.contains("3 occurrences of a"));
        press(&mut app, &[KeyCode::Char('G'), KeyCode::Enter]);
        let v = app.viewer.as_ref().unwrap();
        assert_eq!((v.current_line(), v.column()), (Some(2), 14));
        // `@` lists the file's symbols.
        press(&mut app, &[KeyCode::Char('@')]);
        let screen = draw(&mut term, &mut app);
        assert!(screen.contains("function c"));
        press(&mut app, &[KeyCode::Char('q')]);
        // `#` searches by name.
        press(&mut app, &[KeyCode::Char('#')]);
        app.handle_key(key('b'));
        press(&mut app, &[KeyCode::Enter]);
        assert!(matches!(app.screen(), Screen::Locations(l) if l.title.contains("matching")));
    }

    #[test]
    fn agent_turn_through_the_panel() {
        let dir = scratch_repo();
        let mut session = scratch_session(dir.path());
        session.config.agent = crate::config::AgentConfig {
            kind: "fake-acp".into(),
            ..Default::default()
        };
        let mut app = App::new(session, Some("a.rs".into()));
        let mut term = Terminal::new(TestBackend::new(100, 24)).unwrap();
        // Ask about the current line before the agent has started: the prompt is queued.
        press(&mut app, &[KeyCode::Char('a')]);
        for ch in "what is this?".chars() {
            app.handle_key(key(ch));
        }
        press(&mut app, &[KeyCode::Enter]);
        assert!(matches!(app.screen(), Screen::Agent));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while app.agent.permission.is_none() && std::time::Instant::now() < deadline {
            app.tick();
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(
            app.agent.permission.is_some(),
            "status: {}",
            app.agent.status
        );
        let screen = draw(&mut term, &mut app);
        assert!(screen.contains("agent asks: Read a.rs"));
        assert!(screen.contains("what is this?"));
        press(&mut app, &[KeyCode::Char('1')]);
        while app.agent.status != "idle" && std::time::Instant::now() < deadline {
            app.tick();
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let screen = draw(&mut term, &mut app);
        assert!(screen.contains("Hello from fake, 1 resources"), "{screen}");
        assert!(screen.contains("Read a.rs (read) [completed]"));
        assert_eq!(app.agent.status, "idle");
        assert!(app.agent.progress().is_none());
        // The backend's log lines are in the transcript and `l` hides them.
        let logs: Vec<&str> = app
            .agent
            .entries
            .iter()
            .filter(|e| e.kind == crate::tui::agent_panel::EntryKind::Log)
            .map(|e| e.text.as_str())
            .collect();
        assert!(
            logs.iter()
                .any(|l| l.starts_with("started in-process fake")),
            "{logs:?}"
        );
        assert!(
            logs.iter()
                .any(|l| l.starts_with("session ") && l.contains("open with fake-agent")),
            "{logs:?}"
        );
        assert!(
            screen.contains("log   │ started in-process fake"),
            "{screen}"
        );
        press(&mut app, &[KeyCode::Char('l')]);
        let screen = draw(&mut term, &mut app);
        assert!(!screen.contains("log   │"), "{screen}");
        assert!(screen.contains("log hidden"), "{screen}");
        press(&mut app, &[KeyCode::Char('l')]);
        // Scrolling up stops following; scrolling back to the bottom resumes it.
        term = Terminal::new(TestBackend::new(100, 8)).unwrap();
        draw(&mut term, &mut app);
        assert!(app.agent.follow);
        let bottom = app.agent.scroll;
        assert!(bottom > 0);
        press(&mut app, &[KeyCode::Char('k')]);
        draw(&mut term, &mut app);
        assert!(!app.agent.follow);
        assert_eq!(app.agent.scroll, bottom - 1);
        press(&mut app, &[KeyCode::Char('j')]);
        draw(&mut term, &mut app);
        assert!(app.agent.follow);
        assert_eq!(app.agent.scroll, bottom);
        // `R` restarts the agent from inside the panel instead of opening the review list.
        press(&mut app, &[KeyCode::Char('R')]);
        assert!(matches!(app.screen(), Screen::Agent));
        assert!(app.agent.entries.iter().any(|e| e.text == "restarting"));
        while !app.agent.running() && std::time::Instant::now() < deadline {
            app.tick();
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(app.agent.running(), "restart: {}", app.agent.status);
        // Escape leaves the panel when idle; the transcript is kept.
        press(&mut app, &[KeyCode::Esc]);
        assert!(matches!(app.screen(), Screen::Explorer));
        assert!(!app.agent.entries.is_empty());
        // `A` and `i` reach the panel from the explorer too; `t` inside it toggles thoughts.
        press(&mut app, &[KeyCode::Char('A')]);
        assert!(matches!(app.screen(), Screen::Agent));
        press(&mut app, &[KeyCode::Char('t')]);
        assert!(app.agent.show_thoughts);
        assert!(app.theme_picker.is_none());
        press(&mut app, &[KeyCode::Char('q'), KeyCode::Char('i')]);
        assert!(matches!(app.screen(), Screen::Agent));
        assert!(matches!(
            app.prompt.as_ref().map(|p| &p.kind),
            Some(crate::tui::prompt::PromptKind::Agent { .. })
        ));
        press(&mut app, &[KeyCode::Esc]);
    }

    fn draw_ws(term: &mut Terminal<TestBackend>, ws: &mut super::workspace::Workspace) -> String {
        term.draw(|f| super::ui::draw_workspace(f, ws)).unwrap();
        let buffer = term.backend().buffer().clone();
        let mut out = String::new();
        for y in 0..buffer.area.height {
            for x in 0..buffer.area.width {
                out.push_str(buffer[(x, y)].symbol());
            }
            out.push('\n');
        }
        out
    }

    #[test]
    fn workspace_switches_repositories() {
        use super::workspace::Workspace;
        let a = scratch_repo();
        let b = scratch_repo();
        let name_a = a.path().file_name().unwrap().to_string_lossy().into_owned();
        let name_b = b.path().file_name().unwrap().to_string_lossy().into_owned();
        let mut ws = Workspace::new(vec![
            (scratch_session(a.path()), Some("a.rs".into())),
            (scratch_session(b.path()), None),
        ]);
        let mut term = Terminal::new(TestBackend::new(120, 24)).unwrap();
        let screen = draw_ws(&mut term, &mut ws);
        let strip = screen.lines().next().unwrap().to_string();
        assert!(strip.contains(&format!("1 {name_a}")), "{strip}");
        assert!(strip.contains(&format!("2 {name_b}")), "{strip}");
        assert_eq!(ws.active, 0);
        assert!(
            ws.app().viewer.is_some(),
            "a.rs opened in the first repository"
        );
        // gt / gT cycle; W opens the picker; a number picks directly.
        ws.handle_key(key('g'));
        ws.handle_key(key('t'));
        assert_eq!(ws.active, 1);
        assert!(ws.app().viewer.is_none());
        ws.handle_key(key('g'));
        ws.handle_key(key('t'));
        assert_eq!(ws.active, 0, "wraps around");
        ws.handle_key(key('g'));
        ws.handle_key(key('T'));
        assert_eq!(ws.active, 1);
        ws.handle_key(key('W'));
        assert_eq!(ws.picker, Some(1));
        let screen = draw_ws(&mut term, &mut ws);
        assert!(screen.contains("repositories"), "{screen}");
        assert!(screen.contains(&format!("1 {name_a}")));
        ws.handle_key(key('1'));
        assert_eq!(ws.picker, None);
        assert_eq!(ws.active, 0);
        // A theme saved in one repository reaches the other; a cancelled preview does not.
        ws.handle_key(key('t'));
        ws.handle_key(key('j'));
        let previewed = ws.app().theme.name.clone();
        assert_ne!(ws.apps[1].theme.name, previewed, "preview stays local");
        ws.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        ws.handle_key(key('t'));
        ws.handle_key(key('j'));
        ws.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(ws.apps[1].theme.name, previewed);
        assert_eq!(ws.apps[1].session.config.theme, previewed);
        // A single repository: the strip is gone and the keys say so instead of failing.
        let mut single = Workspace::new(vec![(scratch_session(a.path()), None)]);
        let screen = draw_ws(&mut term, &mut single);
        assert!(!screen.lines().next().unwrap().contains("gt/gT"));
        single.handle_key(key('W'));
        assert_eq!(single.picker, None);
        assert!(
            single
                .app()
                .message
                .as_ref()
                .unwrap()
                .0
                .contains("one repository")
        );
    }

    /// The tree's folds and filter belong to the user: a refresh must not undo them, and
    /// cancelling the filter prompt must put back what was there before.
    #[test]
    fn the_tree_keeps_what_the_user_set_up() {
        let dir = scratch_repo();
        std::fs::create_dir_all(dir.path().join("src/deep")).unwrap();
        std::fs::write(dir.path().join("src/deep/inner.rs"), "fn inner() {}\n").unwrap();
        let session = scratch_session(dir.path());
        let mut app = App::new(session, None);
        let mut term = Terminal::new(TestBackend::new(100, 24)).unwrap();
        draw(&mut term, &mut app);

        // Fold everything, then filter: both are the user's doing.
        press(&mut app, &[KeyCode::Char('z')]);
        assert!(!app.tree.collapsed.is_empty(), "nothing folded");
        let folded = app.tree.collapsed.clone();
        press(&mut app, &[KeyCode::Char('/')]);
        for ch in "inner".chars() {
            app.handle_key(key(ch));
        }
        press(&mut app, &[KeyCode::Enter]);
        assert_eq!(app.tree.filter, "inner");
        let screen = draw(&mut term, &mut app);
        assert!(screen.contains("inner.rs"), "{screen}");
        assert!(
            !screen.contains("Makefile"),
            "the filter is not applied: {screen}"
        );

        // `r` re-reads the repository and keeps both.
        press(&mut app, &[KeyCode::Char('r')]);
        assert_eq!(app.tree.filter, "inner", "the refresh dropped the filter");
        assert_eq!(
            app.tree.collapsed, folded,
            "the refresh unfolded everything"
        );

        // Typing in the filter prompt applies as you go; Esc puts back what was there.
        press(&mut app, &[KeyCode::Char('/')]);
        for ch in "zzz".chars() {
            app.handle_key(key(ch));
        }
        assert_eq!(
            app.tree.filter, "innerzzz",
            "the filter applies as it is typed"
        );
        press(&mut app, &[KeyCode::Esc]);
        assert_eq!(app.tree.filter, "inner", "Esc did not put the filter back");
        let screen = draw(&mut term, &mut app);
        assert!(screen.contains("inner.rs"), "{screen}");
    }

    /// A message says what just happened, and is cleared by the next key. An agent working
    /// in the background must not hide it for the length of a turn.
    #[test]
    fn a_message_outranks_the_agent_status() {
        let dir = scratch_repo();
        let mut session = scratch_session(dir.path());
        session.config.agent = crate::config::AgentConfig {
            kind: "fake-acp".into(),
            ..Default::default()
        };
        let mut app = App::new(session, Some("a.rs".into()));
        let mut term = Terminal::new(TestBackend::new(100, 24)).unwrap();
        app.ask_agent("hello".into(), Vec::new());
        press(&mut app, &[KeyCode::Char('q')]);
        assert!(!matches!(app.screen(), Screen::Agent));
        assert!(app.agent_active(), "the agent should be starting");
        app.error("something went wrong");
        let screen = draw(&mut term, &mut app);
        assert!(screen.contains("something went wrong"), "{screen}");
    }

    /// Moving through the log reloads the file list for whatever commit is under the
    /// cursor, and Tab hands the keys to that list.
    #[test]
    fn the_log_follows_the_cursor() {
        let dir = scratch_repo();
        let session = scratch_session(dir.path());
        let mut app = App::new(session, None);
        let mut term = Terminal::new(TestBackend::new(100, 24)).unwrap();
        press(&mut app, &[KeyCode::Char('L')]);
        assert!(matches!(app.screen(), Screen::Log(_)));
        let screen = draw(&mut term, &mut app);
        assert!(
            screen.contains("second"),
            "the newest commit first: {screen}"
        );
        assert!(
            screen.contains("a.rs"),
            "the files of that commit: {screen}"
        );

        // Down to the first commit: the file list follows.
        press(&mut app, &[KeyCode::Char('j')]);
        let screen = draw(&mut term, &mut app);
        assert!(screen.contains("first"), "{screen}");
        let Screen::Log(state) = app.screen() else {
            panic!("not the log")
        };
        assert_eq!(state.commits.cursor, 1);
        assert_eq!(
            state.files.items.len(),
            1,
            "the first commit added one file"
        );

        // G goes to the end, which is also where another page would be asked for.
        press(&mut app, &[KeyCode::Char('G')]);
        let Screen::Log(state) = app.screen() else {
            panic!("not the log")
        };
        assert_eq!(state.commits.cursor, state.commits.items.len() - 1);
        assert!(state.exhausted, "a short history is all there is");

        // Tab hands the keys to the file list, and Enter opens the diff of that file.
        press(&mut app, &[KeyCode::Tab]);
        let Screen::Log(state) = app.screen() else {
            panic!("not the log")
        };
        assert!(state.focus_files);
        press(&mut app, &[KeyCode::Enter]);
        assert!(matches!(app.screen(), Screen::Diff(_)), "no diff opened");
        draw(&mut term, &mut app);
        press(&mut app, &[KeyCode::Char('q')]);
        assert!(matches!(app.screen(), Screen::Log(_)));
        // q in the file list goes back to the commits, and again leaves the log.
        press(&mut app, &[KeyCode::Char('q')]);
        assert!(matches!(app.screen(), Screen::Log(_)), "still in the log");
        press(&mut app, &[KeyCode::Char('q')]);
        assert!(matches!(app.screen(), Screen::Explorer));
    }

    #[test]
    fn asking_about_an_empty_file_does_not_panic() {
        let dir = scratch_repo();
        std::fs::write(dir.path().join("empty.rs"), "").unwrap();
        let session = scratch_session(dir.path());
        let mut app = App::new(session, Some("empty.rs".into()));
        press(&mut app, &[KeyCode::Char('a')]);
        assert!(app.prompt.is_none());
        assert!(app.message.as_ref().is_some_and(|m| m.0.contains("empty")));
        press(&mut app, &[KeyCode::Char('c')]);
        press(&mut app, &[KeyCode::Esc]);
    }

    #[test]
    fn narrow_terminal_does_not_panic() {
        let dir = scratch_repo();
        let session = scratch_session(dir.path());
        let mut app = App::new(session, Some("a.rs".into()));
        let mut term = Terminal::new(TestBackend::new(40, 8)).unwrap();
        draw(&mut term, &mut app);
        press(&mut app, &[KeyCode::Char('D')]);
        draw(&mut term, &mut app);
        press(&mut app, &[KeyCode::Char('?')]);
        draw(&mut term, &mut app);
    }
}
