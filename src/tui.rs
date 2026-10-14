//! The terminal front end: terminal setup, the event loop, and `$EDITOR` hand-off.

pub mod agent_panel;
pub mod app;
pub mod diff_view;
pub mod kinds;
pub mod outline;
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

use workspace::Workspace;

/// Runs the terminal UI on one or more repositories, each with a file to open first, and
/// on whatever repository appears later in a directory of checkouts it was started on.
pub fn run(opened: crate::session::Opened) -> Result<()> {
    anyhow::ensure!(!opened.targets.is_empty(), "no repository to open");
    let mut ws = Workspace::new(opened.targets);
    if !opened.checkouts.is_empty() {
        ws.open_home();
    }
    ws.watch(opened.checkouts);
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

/// How this terminal draws pictures, asked of it once, when the first picture is opened: with
/// the screen raw, since the answer comes back as input, and between reads of the keys, so
/// the answer is not taken for one. A terminal answers in milliseconds; one that does not
/// answer at all is given a second, not the library's two. Half blocks are left out — a
/// picture in coloured characters is not a picture the terminal can show.
///
/// Where nothing answers, the library's reader stays blocked on the input after giving up and
/// takes the next key pressed; it offers no way to stop it. Every terminal answers, tmux
/// included, so that is a pipe or a detached session — and a terminal that says it is `dumb`,
/// or says nothing, is not asked at all.
fn graphics() -> Option<ratatui_image::picker::Picker> {
    use ratatui_image::picker::cap_parser::QueryStdioOptions;
    use ratatui_image::picker::{Picker, ProtocolType};
    let term = std::env::var("TERM").unwrap_or_default();
    if term.is_empty() || term == "dumb" {
        return None;
    }
    // Not `{ timeout, ..Default::default() }`: the options have a private field, which
    // struct update syntax cannot fill from outside the crate.
    #[allow(clippy::field_reassign_with_default)]
    let options = {
        let mut options = QueryStdioOptions::default();
        options.timeout = Duration::from_secs(1);
        options
    };
    Picker::from_query_stdio_with_options(options)
        .ok()
        .filter(|p| p.protocol_type() != ProtocolType::Halfblocks)
}

fn event_loop(terminal: &mut ratatui::DefaultTerminal, ws: &mut Workspace) -> Result<()> {
    loop {
        if ws.app().needs_graphics() {
            ws.set_graphics(graphics());
        }
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
        let wait = if ws.agent_active() || ws.animating() {
            40
        } else {
            250
        };
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
                MouseEventKind::ScrollDown if !ws.scroll_picker(1) => ws.app_mut().scroll_main(3),
                MouseEventKind::ScrollUp if !ws.scroll_picker(-1) => ws.app_mut().scroll_main(-3),
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
    use super::app::{App, Screen, Section};
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
        app.switch_section(Section::Code);
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
        app.switch_section(Section::Code);
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

    /// A side panel set to hide is there while it has the focus and gone while the file has
    /// it. The setting is made in the options panel and saved to the config file at once.
    #[test]
    fn a_side_panel_set_to_hide_gives_the_file_its_width() {
        let dir = scratch_repo();
        let mut app = App::new(scratch_session(dir.path()), None);
        app.switch_section(Section::Code);
        let mut term = Terminal::new(TestBackend::new(120, 30)).unwrap();
        assert!(draw(&mut term, &mut app).contains(" files "));

        // The first row of the options panel is the file tree's.
        app.handle_key(key(','));
        let screen = draw(&mut term, &mut app);
        assert!(screen.contains("Files: auto-hide"), "{screen}");
        press(&mut app, &[KeyCode::Char(' '), KeyCode::Esc]);
        assert!(app.options.is_none());
        assert!(app.session.config.sidebar.files_auto_hide);
        let saved = crate::config::Config::load_from(
            &dir.path().join(".git").join("codereview-config.toml"),
        )
        .unwrap();
        assert!(saved.sidebar.files_auto_hide, "saved as it was changed");

        // With nothing open the tree has the focus and stays; opening a file hides it, and
        // Tab, or q from the file, brings it back.
        assert!(draw(&mut term, &mut app).contains(" files "));
        press(&mut app, &[KeyCode::Char('j'), KeyCode::Enter]);
        let screen = draw(&mut term, &mut app);
        assert!(
            !screen.contains(" files ") && screen.contains("fn a() {}"),
            "{screen}"
        );
        press(&mut app, &[KeyCode::Tab]);
        assert!(draw(&mut term, &mut app).contains(" files "));
        press(&mut app, &[KeyCode::Tab]);
        assert!(!draw(&mut term, &mut app).contains(" files "));
        press(&mut app, &[KeyCode::Char('q')]);
        assert!(draw(&mut term, &mut app).contains(" files "));
        assert!(!app.quit, "q from the file only brings the tree back");
    }

    /// The outline sits where the tree does, follows the file's cursor, and goes where it
    /// points; it hides the same way when told to.
    #[test]
    fn the_outline_follows_the_file_and_goes_where_it_points() {
        let dir = scratch_repo();
        let mut app = App::new(scratch_session(dir.path()), None);
        app.switch_section(Section::Code);
        let mut term = Terminal::new(TestBackend::new(120, 30)).unwrap();
        draw(&mut term, &mut app);
        app.handle_key(key('O'));
        let screen = draw(&mut term, &mut app);
        assert!(screen.contains("Open a file to see what"), "{screen}");

        // `a.rs` defines a, b and c in the working tree.
        press(
            &mut app,
            &[KeyCode::Char('O'), KeyCode::Char('j'), KeyCode::Enter],
        );
        app.handle_key(key('O'));
        let screen = draw(&mut term, &mut app);
        assert!(screen.contains(" outline 3 "), "{screen}");
        assert!(
            screen.contains("fn a") && screen.contains("fn c"),
            "{screen}"
        );

        // Enter goes to the definition and gives the file the focus.
        press(
            &mut app,
            &[KeyCode::Char('j'), KeyCode::Char('j'), KeyCode::Enter],
        );
        assert!(!app.focus_tree);
        assert_eq!(app.viewer.as_ref().unwrap().current_line(), Some(2));
        // Moving in the file moves the outline with it.
        press(&mut app, &[KeyCode::Char('k')]);
        draw(&mut term, &mut app);
        assert_eq!(app.outline.as_ref().unwrap().cursor, 1);
        assert!(draw(&mut term, &mut app).contains("┌ outline"));

        // Set to hide, it goes while the file has the focus.
        app.handle_key(key(','));
        press(
            &mut app,
            &[KeyCode::Char('j'), KeyCode::Char(' '), KeyCode::Esc],
        );
        assert!(app.session.config.sidebar.outline_auto_hide);
        assert!(!draw(&mut term, &mut app).contains("┌ outline"));
        press(&mut app, &[KeyCode::Tab]);
        assert!(draw(&mut term, &mut app).contains("┌ outline"));
    }

    /// A picture is drawn where the terminal has a way to draw one, and described where it
    /// has not, rather than called a binary file either way.
    #[test]
    fn a_picture_is_drawn_or_described() {
        let dir = scratch_repo();
        std::fs::write(dir.path().join("logo.png"), include_bytes!("web/icon.png")).unwrap();
        let mut app = App::new(scratch_session(dir.path()), Some("logo.png".into()));
        let mut term = Terminal::new(TestBackend::new(100, 30)).unwrap();
        let screen = draw(&mut term, &mut app);
        assert!(
            screen.contains("A picture,") && screen.contains("This terminal draws none"),
            "{screen}"
        );
        assert!(!screen.contains("binary file"), "{screen}");

        // Any protocol will do for the drawing path; a test terminal answers no query, so
        // half blocks stand in for kitty or sixel here.
        app.graphics = Some(ratatui_image::picker::Picker::halfblocks());
        app.picture = None;
        let screen = draw(&mut term, &mut app);
        assert!(
            !screen.contains("draws none") && !screen.contains("binary file"),
            "{screen}"
        );
        assert!(matches!(
            app.picture.as_ref().map(|p| p.shown.is_ok()),
            Some(true)
        ));
        // Another file drops it.
        app.open_file("a.rs");
        draw(&mut term, &mut app);
        assert!(app.picture.is_none());
    }

    #[test]
    fn walk_every_screen() {
        let dir = scratch_repo();
        let session = scratch_session(dir.path());
        let mut app = App::new(session, None);
        let mut term = Terminal::new(TestBackend::new(120, 30)).unwrap();
        // A repository opens on its tests; the code is the third section.
        assert_eq!(app.section, Section::Tests);
        assert!(matches!(app.screen(), Screen::EndToEnd(_)));
        press(&mut app, &[KeyCode::Char('3')]);
        assert_eq!(app.section, Section::Code);
        let screen = draw(&mut term, &mut app);
        assert!(screen.contains("1 Tests"));
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
        assert_eq!(app.section, Section::Review);
        let screen = draw(&mut term, &mut app);
        assert!(screen.contains("needs a doc comment"));
        press(&mut app, &[KeyCode::Char('j'), KeyCode::Char('d')]);
        assert_eq!(app.session.review.pending_count(), 1);
        press(&mut app, &[KeyCode::Char('u')]);
        assert_eq!(app.session.review.pending_count(), 2);
        // A comment opens its file over the review, which `q` comes back to.
        press(&mut app, &[KeyCode::Enter]);
        assert!(matches!(app.screen(), Screen::Explorer));
        assert_eq!(app.screens.len(), 2);
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
        // Out of the notes, out of the file into the tree, back to the review, back to the
        // tests, and only then out.
        press(
            &mut app,
            &[
                KeyCode::Esc,
                KeyCode::Char('q'),
                KeyCode::Char('q'),
                KeyCode::Char('q'),
            ],
        );
        assert!(matches!(app.screen(), Screen::Review(_)));
        press(&mut app, &[KeyCode::Char('q')]);
        assert_eq!(app.section, Section::Tests);
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
        // Tall enough that the whole turn fits however many rows the scratch repository's path
        // wraps to: under a long TMPDIR (a Portage build) a 24-row panel scrolled the first log
        // line out of view.
        let mut term = Terminal::new(TestBackend::new(100, 48)).unwrap();
        // Ask about the current line before the agent has started: the prompt is queued.
        press(&mut app, &[KeyCode::Char('a')]);
        for ch in "what is this?".chars() {
            app.handle_key(key(ch));
        }
        press(&mut app, &[KeyCode::Enter]);
        assert!(matches!(app.screen(), Screen::Agent));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while app.agent.permission().is_none() && std::time::Instant::now() < deadline {
            app.tick(true);
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(
            app.agent.permission().is_some(),
            "status: {}",
            app.agent.status()
        );
        let screen = draw(&mut term, &mut app);
        assert!(screen.contains("agent asks: Read a.rs"));
        assert!(screen.contains("what is this?"));
        press(&mut app, &[KeyCode::Char('1')]);
        while app.agent.status() != "idle" && std::time::Instant::now() < deadline {
            app.tick(true);
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let screen = draw(&mut term, &mut app);
        assert!(screen.contains("Hello from fake, 1 resources"), "{screen}");
        assert!(screen.contains("Read a.rs (read) [completed]"));
        assert_eq!(app.agent.status(), "idle");
        assert!(app.agent.progress().is_none());
        // The backend's log lines are in the transcript and `l` hides them.
        let logs: Vec<&str> = app
            .agent
            .transcript
            .entries()
            .iter()
            .filter(|e| e.kind == crate::transcript::EntryKind::Log)
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
        assert!(
            app.agent
                .transcript
                .entries()
                .iter()
                .any(|e| e.text == "restarting")
        );
        while !app.agent.running() && std::time::Instant::now() < deadline {
            app.tick(true);
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(app.agent.running(), "restart: {}", app.agent.status());
        // Escape leaves the panel when idle; the transcript is kept.
        press(&mut app, &[KeyCode::Esc]);
        assert!(matches!(app.screen(), Screen::Explorer));
        assert!(!app.agent.transcript.entries().is_empty());
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

    /// Ticks `app` until its symbol index is in, which is when the end-to-end screen reads
    /// the tests it could not read before.
    fn until_indexed(app: &mut App) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while !app.session.has_index() && std::time::Instant::now() < deadline {
            app.tick(true);
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(app.session.has_index(), "the index never came");
    }

    #[test]
    fn the_end_to_end_tests_play_their_recordings() {
        let dir = scratch_repo();
        let root = dir.path();
        let write = |path: &str, text: &str| {
            let path = root.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        };
        // Two tests that drive the program in a terminal; the first left a recording.
        write(
            "tests/cli.rs",
            "// drives it through portable_pty\n#[test]\nfn a_file_opens() {\n    assert!(true);\n}\n\n\
             #[test]\nfn a_comment_is_saved() {\n    assert!(true);\n}\n",
        );
        write(
            "target/e2e/terminal/a-file-opens/test.json",
            r#"{"test": "a_file_opens", "file": "tests/cli.rs", "line": 3, "passed": true,
                "duration_ms": 1200, "recording": "terminal.cast", "steps": [
                {"at_ms": 0, "line": 4, "kind": "action", "text": "start the program"},
                {"at_ms": 600, "line": 4, "kind": "check", "text": "shows the second screen",
                 "passed": true}]}"#,
        );
        write(
            "target/e2e/terminal/a-file-opens/terminal.cast",
            "{\"version\": 2, \"width\": 30, \"height\": 4}\n[0.0, \"o\", \"first screen\"]\n\
             [0.6, \"o\", \"\\u001b[2J\\u001b[Hsecond screen\"]\n",
        );
        let mut app = App::new(scratch_session(root), None);
        let mut term = Terminal::new(TestBackend::new(140, 36)).unwrap();

        press(&mut app, &[KeyCode::Char('E')]);
        assert!(matches!(app.screen(), Screen::EndToEnd(_)));
        until_indexed(&mut app);
        let screen = draw(&mut term, &mut app);
        assert!(screen.contains("2 end-to-end tests of"), "{screen}");
        assert!(screen.contains("1 without a recording"), "{screen}");
        assert!(
            screen.contains("tests/cli.rs 2"),
            "the file heads its tests: {screen}"
        );
        assert!(screen.contains("✓ a file opens"), "{screen}");
        assert!(screen.contains("○ a comment is saved"), "{screen}");
        assert!(
            screen.contains("tests/cli.rs:3 · it drives a terminal"),
            "{screen}"
        );
        assert!(screen.contains("✓ passed in 1.2 s"), "{screen}");
        // The recording, paused at its start, and its steps beside it.
        assert!(screen.contains("first screen"), "{screen}");
        assert!(screen.contains("0:00.0 / 0:01.2 paused"), "{screen}");
        assert!(screen.contains("start the program"), "{screen}");

        // `]` goes to the next step, and the screen as it was then.
        press(&mut app, &[KeyCode::Char(']')]);
        let screen = draw(&mut term, &mut app);
        assert!(screen.contains("second screen"), "{screen}");
        assert!(!screen.contains("first screen"), "{screen}");
        press(&mut app, &[KeyCode::Char('[')]);
        let screen = draw(&mut term, &mut app);
        assert!(screen.contains("first screen"), "{screen}");

        // Space plays it, and the loop is told to draw often until it ends.
        press(&mut app, &[KeyCode::Char(' ')]);
        assert!(app.animating());
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while app.animating() && std::time::Instant::now() < deadline {
            app.tick(true);
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(!app.animating(), "it stops at its end");
        let screen = draw(&mut term, &mut app);
        assert!(screen.contains("second screen"), "{screen}");

        // The test without a recording says so.
        press(&mut app, &[KeyCode::Char('j')]);
        let screen = draw(&mut term, &mut app);
        assert!(screen.contains("No recording"), "{screen}");

        // `s` is the test's source, opened over the list; `E` comes back to the list, and
        // from the list, which is the first screen there is, `q` quits.
        press(&mut app, &[KeyCode::Char('k'), KeyCode::Char('s')]);
        assert!(matches!(app.screen(), Screen::Explorer));
        let viewer = app.viewer.as_ref().unwrap();
        assert_eq!(viewer.path, "tests/cli.rs");
        assert_eq!(viewer.current_line(), Some(2));
        press(&mut app, &[KeyCode::Char('E')]);
        assert!(matches!(app.screen(), Screen::EndToEnd(_)));
        assert_eq!(app.screens.len(), 1);
        press(&mut app, &[KeyCode::Char('q')]);
        assert!(app.quit);
    }

    #[test]
    fn a_stretch_shows_what_no_test_ran_and_each_test_before_and_after() {
        use super::app::{E2eRow, TestRow};
        let dir = scratch_repo();
        let root = dir.path();
        let write = |root: &std::path::Path, path: &str, text: &str| {
            let path = root.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        };
        let git = |args: &[&str]| {
            let out = std::process::Command::new("git")
                .arg("-C")
                .arg(root)
                .args(args)
                .output()
                .unwrap();
            assert!(out.status.success(), "{args:?}");
            String::from_utf8(out.stdout).unwrap().trim().to_string()
        };
        write(root, "Makefile", "e2e:\n\tcargo test --test cli\n");
        write(
            root,
            "tests/cli.rs",
            "// drives it through portable_pty\n#[test]\nfn a_review_is_saved() {}\n",
        );
        let unit =
            "#[cfg(test)]\nmod tests {\n    #[test]\n    fn saves() {\n        assert!(true);\n";
        write(
            root,
            "src/lib.rs",
            &format!(
                "pub fn save_review() {{\n    write();\n}}\n\nfn write() {{}}\n\n{unit}        \
                 assert!(true);\n    }}\n}}\n"
            ),
        );
        git(&["add", "-A"]);
        git(&["commit", "-qm", "base"]);
        let base = git(&["rev-parse", "HEAD"]);
        let short = git(&["rev-parse", "--short", "HEAD"]);
        // Uncommitted: it saves twice, exports a review nothing runs, and its unit test checks
        // one thing fewer.
        write(
            root,
            "src/lib.rs",
            &format!(
                "pub fn save_review() {{\n    write();\n    write();\n}}\n\npub fn export_review() \
                 {{\n    write();\n}}\n\nfn write() {{}}\n\n{unit}    }}\n}}\n"
            ),
        );
        // A recording of the test now, and one at the base, in a worktree of it.
        let record = |root: &std::path::Path, passed: bool, screen: &str, ran: &[u32]| {
            let dir = "target/e2e/cli/a-review-is-saved";
            write(
                root,
                &format!("{dir}/test.json"),
                &format!(
                    r#"{{"test": "a_review_is_saved", "file": "tests/cli.rs", "line": 3,
                        "passed": {passed}, "recording": "terminal.cast", "steps": [
                        {{"at_ms": 100, "kind": "action", "text": "save it"}},
                        {{"at_ms": 900, "kind": "check", "text": "it is saved"}}]}}"#
                ),
            );
            write(
                root,
                &format!("{dir}/terminal.cast"),
                &format!(
                    "{{\"version\":2,\"width\":20,\"height\":2}}\n[0.1,\"o\",\"start\"]\n\
                     [0.8,\"o\",\"\\r\\n{screen}\"]\n"
                ),
            );
            let da: String = [1, 2, 3, 4, 6, 7, 8, 10]
                .iter()
                .map(|l| format!("DA:{l},{}\n", u32::from(ran.contains(l))))
                .collect();
            write(
                root,
                &format!("{dir}/coverage.lcov"),
                &format!("SF:src/lib.rs\n{da}end_of_record\n"),
            );
        };
        record(root, true, "saved twice", &[1, 2, 3, 4, 10]);
        let session = scratch_session(root);
        let (worktree, _) = crate::stretch::prepare(&session, &base).unwrap();
        record(&worktree, false, "saved once", &[1, 2, 3]);
        let mut app = App::new(session, None);
        let mut term = Terminal::new(TestBackend::new(160, 40)).unwrap();
        press(&mut app, &[KeyCode::Char('E')]);
        until_indexed(&mut app);

        // From the last 7 days to the last commit to the uncommitted work.
        press(&mut app, &[KeyCode::Char('w'), KeyCode::Char('w')]);
        let screen = draw(&mut term, &mut app);
        assert!(screen.contains("marking uncommitted work (w)"), "{screen}");
        assert!(screen.contains("In uncommitted work"), "{screen}");
        assert!(
            screen.contains("✗ weakened saves"),
            "the unit test that asks less: {screen}"
        );
        assert!(screen.contains("No end-to-end test ran "), "{screen}");
        assert!(screen.contains("▌src/lib.rs  "), "{screen}");
        assert!(
            screen.contains(&format!("Before {short}: recorded, 1 test")),
            "{screen}"
        );
        assert!(
            screen.contains("✓ a review is saved Δ") && screen.contains("✗→✓ screen changed"),
            "the test ran the change, failed before, passes now: {screen}"
        );

        // The changed code nothing ran opens with those lines marked.
        let Screen::EndToEnd(state) = app.screen() else {
            panic!("the list");
        };
        let unran = state
            .rows
            .items
            .iter()
            .position(|r| *r == E2eRow::Unran(0))
            .unwrap();
        if let Some(Screen::EndToEnd(state)) = app.screens.last_mut() {
            state.rows.cursor = unran;
        }
        let screen = draw(&mut term, &mut app);
        assert!(
            screen.contains("ran in no end-to-end test"),
            "the row says what it is: {screen}"
        );
        press(&mut app, &[KeyCode::Enter]);
        assert!(matches!(app.screen(), Screen::Explorer));
        let viewer = app.viewer.as_ref().unwrap();
        assert_eq!(viewer.path, "src/lib.rs");
        let marked = viewer.ran.clone().unwrap();
        assert!(marked.unran);
        // Git decides which of the moved lines are the new ones; what nothing ran is in the
        // new function, whichever it picks.
        assert!(
            !marked.ranges.is_empty() && marked.ranges.iter().all(|(a, b)| 6 <= *a && *b <= 8),
            "{:?}",
            marked.ranges
        );
        let screen = draw(&mut term, &mut app);
        assert!(
            screen.contains("▌ changed in uncommitted work, run by no end-to-end test"),
            "{screen}"
        );

        // The file was opened over the list, which `q` comes back to, where it was left.
        press(&mut app, &[KeyCode::Char('q'), KeyCode::Char('q')]);
        let Screen::EndToEnd(state) = app.screen() else {
            panic!("back on the list");
        };
        assert_eq!(state.rows.cursor, unran);
        let test = state
            .rows
            .items
            .iter()
            .position(|r| matches!(r, E2eRow::Test(_)))
            .unwrap();
        if let Some(Screen::EndToEnd(state)) = app.screens.last_mut() {
            state.rows.cursor = test;
        }

        // The test on its own: before and after, side by side, the steps paired.
        press(&mut app, &[KeyCode::Enter]);
        let Screen::EndToEndTest(state) = app.screen() else {
            panic!("the test's own screen");
        };
        let rows = state.rows.items.clone();
        assert!(rows.contains(&TestRow::Pair(1)), "{rows:?}");
        let screen = draw(&mut term, &mut app);
        assert!(
            screen.contains(&format!("Steps, as they ran at {short} → as they run now")),
            "{screen}"
        );
        assert!(screen.contains(&format!("at {short} · ")), "{screen}");
        assert!(screen.contains("now · "), "{screen}");
        assert!(screen.contains("it is saved  screen changed"), "{screen}");
        let pair = rows.iter().position(|r| *r == TestRow::Pair(1)).unwrap();
        if let Some(Screen::EndToEndTest(state)) = app.screens.last_mut() {
            state.rows.cursor = pair;
        }
        press(&mut app, &[KeyCode::Enter]);
        let screen = draw(&mut term, &mut app);
        assert!(
            screen.contains("saved once") && screen.contains("saved twice"),
            "both recordings at the step: {screen}"
        );
    }

    #[test]
    fn a_test_leads_to_the_code_it_ran_and_what_documents_it() {
        use super::app::TestRow;
        let dir = scratch_repo();
        let root = dir.path();
        let write = |path: &str, text: &str| {
            let path = root.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        };
        write(
            "src/review.rs",
            "pub fn save_review_file() {}\n\npub fn load_review_file() {}\n\npub fn unused_here() {}\n",
        );
        write(
            "src/unit.rs",
            "#[test]\nfn saving_works() {\n    crate::review::save_review_file();\n}\n",
        );
        write(
            "README.md",
            "# tool\n\nIt reviews code.\n\n## Saving a review\n\nThe review is saved to a file \
             and loaded again.\n\n## Colours\n\nSchemes.\n",
        );
        write(
            "tests/cli.rs",
            "// drives it through portable_pty\n#[test]\nfn a_review_is_saved() {\n    press(\"c\");\n    \
             press(\"Enter\");\n}\n",
        );
        write(
            "target/e2e/terminal/a-review-is-saved/test.json",
            r#"{"test": "a_review_is_saved", "file": "tests/cli.rs", "line": 3, "passed": true,
                "duration_ms": 1000, "recording": "terminal.cast", "steps": [
                {"at_ms": 0, "line": 4, "kind": "action", "text": "start a review"},
                {"at_ms": 700, "line": 5, "kind": "check", "text": "the review is saved",
                 "passed": true}]}"#,
        );
        write(
            "target/e2e/terminal/a-review-is-saved/terminal.cast",
            "{\"version\": 2, \"width\": 30, \"height\": 3}\n[0.0, \"o\", \"typing\"]\n\
             [0.7, \"o\", \"\\u001b[2J\\u001b[Hsaved\"]\n",
        );
        write(
            "target/e2e/terminal/a-review-is-saved/coverage.lcov",
            "SF:src/review.rs\nDA:1,4\nDA:3,0\nDA:5,0\nend_of_record\n\
             SF:tests/cli.rs\nDA:4,1\nend_of_record\n",
        );
        let mut app = App::new(scratch_session(root), None);
        let mut term = Terminal::new(TestBackend::new(140, 40)).unwrap();
        press(&mut app, &[KeyCode::Char('E')]);
        until_indexed(&mut app);

        // Enter opens the test on its own.
        press(&mut app, &[KeyCode::Enter]);
        let Screen::EndToEndTest(state) = app.screen() else {
            panic!("the test's own screen");
        };
        let rows = state.rows.items.clone();
        let screen = draw(&mut term, &mut app);
        assert!(screen.contains("test a review is saved"), "{screen}");
        assert!(
            screen.contains("tests/cli.rs:3 · it drives a terminal"),
            "{screen}"
        );
        assert!(screen.contains("typing"), "the recording, paused: {screen}");
        for heading in [
            "Steps",
            "Code it ran",
            "Documentation",
            "Smaller tests that reach the same code",
        ] {
            assert!(rows.contains(&TestRow::Heading(heading.into())), "{rows:?}");
        }
        assert!(screen.contains("start a review"), "{screen}");
        assert!(
            screen.contains("4 │ press(\"c\");"),
            "the step's own line: {screen}"
        );
        assert!(
            screen.contains("src/review.rs  1 line, 1 only this test"),
            "{screen}"
        );
        assert!(
            screen.contains("Saving a review — README.md:5  a guess: shares"),
            "{screen}"
        );
        assert!(
            screen.contains("saving works · api · src/unit.rs:2 · shares src/review.rs"),
            "{screen}"
        );

        // Enter on a step goes to its moment of the recording.
        let step = rows.iter().position(|r| *r == TestRow::Step(1)).unwrap();
        if let Some(Screen::EndToEndTest(state)) = app.screens.last_mut() {
            state.rows.cursor = step;
        }
        press(&mut app, &[KeyCode::Enter]);
        let screen = draw(&mut term, &mut app);
        assert!(screen.contains("saved"), "{screen}");
        assert!(
            screen.contains("▶0:00.7"),
            "the step playback is at: {screen}"
        );

        // A file it ran opens with those lines marked.
        let ran = rows.iter().position(|r| *r == TestRow::Ran(0)).unwrap();
        if let Some(Screen::EndToEndTest(state)) = app.screens.last_mut() {
            state.rows.cursor = ran;
        }
        press(&mut app, &[KeyCode::Enter]);
        assert!(matches!(app.screen(), Screen::Explorer));
        let viewer = app.viewer.as_ref().unwrap();
        assert_eq!(viewer.path, "src/review.rs");
        assert_eq!(viewer.current_line(), Some(0));
        let marked = viewer.ran.as_ref().expect("the lines it ran");
        assert!(marked.covers(0) && !marked.covers(2));
        let screen = draw(&mut term, &mut app);
        assert!(screen.contains("1 ▌pub fn save_review_file"), "{screen}");
        assert!(screen.contains("ran by “a review is saved”"), "{screen}");

        // The documentation opens at its heading.
        press(&mut app, &[KeyCode::Char('E'), KeyCode::Enter]);
        let doc = match app.screen() {
            Screen::EndToEndTest(state) => state
                .rows
                .items
                .iter()
                .position(|r| *r == TestRow::Doc(0))
                .unwrap(),
            _ => panic!("the test's own screen"),
        };
        if let Some(Screen::EndToEndTest(state)) = app.screens.last_mut() {
            state.rows.cursor = doc;
        }
        press(&mut app, &[KeyCode::Enter]);
        let viewer = app.viewer.as_ref().unwrap();
        assert_eq!(viewer.path, "README.md");
        assert_eq!(viewer.current_line(), Some(4));
        assert!(
            viewer.ran.is_none(),
            "a file opened otherwise is not marked"
        );

        // q goes back to the list.
        press(
            &mut app,
            &[KeyCode::Char('E'), KeyCode::Enter, KeyCode::Char('q')],
        );
        assert!(matches!(app.screen(), Screen::EndToEnd(_)));
    }

    #[test]
    fn without_end_to_end_tests_the_agent_is_offered_the_first() {
        let dir = scratch_repo();
        let mut session = scratch_session(dir.path());
        session.config.agent = crate::config::AgentConfig {
            kind: "fake-acp".into(),
            ..Default::default()
        };
        let mut app = App::new(session, None);
        let mut term = Terminal::new(TestBackend::new(120, 30)).unwrap();
        press(&mut app, &[KeyCode::Char('E')]);
        until_indexed(&mut app);
        let screen = draw(&mut term, &mut app);
        assert!(screen.contains("No end-to-end tests."), "{screen}");
        assert!(
            screen.contains("ask the agent to write the first ones"),
            "{screen}"
        );
        press(&mut app, &[KeyCode::Char('a')]);
        assert!(matches!(app.screen(), Screen::Agent));
    }

    #[test]
    fn the_specification_screen() {
        use super::app::SpecRow;
        let dir = scratch_repo();
        let git = |args: &[&str]| {
            let out = std::process::Command::new("git")
                .arg("-C")
                .arg(dir.path())
                .args(args)
                .output()
                .unwrap();
            assert!(out.status.success(), "{args:?}");
        };
        std::fs::create_dir(dir.path().join("tests")).unwrap();
        let login = dir.path().join("tests/login.rs");
        std::fs::write(
            &login,
            "#[test]\nfn a_wrong_password_is_refused() {\n    assert!(true);\n    assert!(true);\n}\n",
        )
        .unwrap();
        git(&["add", "tests"]);
        git(&["commit", "-q", "-m", "a test of signing in"]);
        // Uncommitted: it checks one thing fewer.
        std::fs::write(
            &login,
            "#[test]\nfn a_wrong_password_is_refused() {\n    assert!(true);\n}\n",
        )
        .unwrap();
        let mut app = App::new(scratch_session(dir.path()), None);
        let mut term = Terminal::new(TestBackend::new(140, 30)).unwrap();

        press(&mut app, &[KeyCode::Char('P')]);
        let screen = draw(&mut term, &mut app);
        assert!(screen.contains("specification"), "{screen}");
        assert!(screen.contains("last 7 days"), "{screen}");
        assert!(
            screen.contains("How does the test interact with it?"),
            "{screen}"
        );
        assert!(screen.contains("No interaction 1"), "{screen}");
        assert!(
            screen.contains("a wrong password is refused · security"),
            "security is a mark: {screen}"
        );
        assert!(
            screen.contains("No test drives a screen of the program"),
            "an empty GUI/TUI group is said: {screen}"
        );
        // c: grouped by the next question.
        press(&mut app, &[KeyCode::Char('c')]);
        let screen = draw(&mut term, &mut app);
        assert!(screen.contains("(c: by measurement)"), "{screen}");
        assert!(screen.contains("No measurement 1"), "{screen}");

        // s: the last commit, then uncommitted work, where the test asks less.
        press(&mut app, &[KeyCode::Char('s')]);
        let screen = draw(&mut term, &mut app);
        assert!(screen.contains("a test of signing in"), "{screen}");
        assert!(screen.contains("added"), "{screen}");
        press(&mut app, &[KeyCode::Char('s')]);
        let screen = draw(&mut term, &mut app);
        assert!(screen.contains("Asks less than before: 1"), "{screen}");
        assert!(screen.contains("weakened"), "{screen}");
        assert!(screen.contains("2 → 1 assertions"), "{screen}");
        assert_eq!(app.section, Section::AllTests);
        assert_eq!(app.screens.len(), 1, "cycling replaces the screen");

        // Enter on the change: its file's diff across the stretch.
        let rows = |app: &App| match app.screen() {
            Screen::Spec(s) => s.rows.items.clone(),
            _ => panic!("not the specification"),
        };
        let at = rows(&app)
            .iter()
            .position(|r| matches!(r, SpecRow::Change(..)))
            .unwrap();
        press(&mut app, &vec![KeyCode::Char('j'); at]);
        press(&mut app, &[KeyCode::Enter]);
        assert!(matches!(app.screen(), Screen::Diff(_)));
        let screen = draw(&mut term, &mut app);
        assert!(screen.contains("assert!(true);"), "{screen}");
        press(&mut app, &[KeyCode::Char('q')]);

        // Enter on a test: the file, at the test.
        let test = rows(&app)
            .iter()
            .position(|r| matches!(r, SpecRow::Test(..)))
            .unwrap();
        press(&mut app, &[KeyCode::Home]);
        press(&mut app, &vec![KeyCode::Char('j'); test]);
        press(&mut app, &[KeyCode::Enter]);
        assert!(matches!(app.screen(), Screen::Explorer));
        assert_eq!(app.viewer.as_ref().unwrap().path, "tests/login.rs");

        // From the log, P is the commit under the cursor.
        press(&mut app, &[KeyCode::Char('L'), KeyCode::Char('P')]);
        let screen = draw(&mut term, &mut app);
        assert!(screen.contains("commit"), "{screen}");
        assert!(screen.contains("a test of signing in"), "{screen}");
        press(&mut app, &[KeyCode::Char('s')]);
        let screen = draw(&mut term, &mut app);
        assert!(screen.contains("this is one commit"), "{screen}");
    }

    #[test]
    fn tests_and_code_lead_to_each_other() {
        use super::app::SpecRow;
        let dir = scratch_repo();
        std::fs::create_dir(dir.path().join("src")).unwrap();
        std::fs::write(
            dir.path().join("src/core.rs"),
            "pub fn parse_review() {}\n\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn a_review_parses() {\n        super::parse_review();\n        assert!(true);\n    }\n}\n",
        )
        .unwrap();
        let mut app = App::new(scratch_session(dir.path()), None);
        let mut term = Terminal::new(TestBackend::new(140, 40)).unwrap();
        let rows = |app: &App| match app.screen() {
            Screen::Spec(s) => s.rows.items.clone(),
            _ => panic!("not the specification"),
        };
        let go_to = |app: &mut App, at: usize| {
            press(app, &[KeyCode::Home]);
            press(app, &vec![KeyCode::Char('j'); at]);
        };

        press(&mut app, &[KeyCode::Char('P')]);
        let screen = draw(&mut term, &mut app);
        assert!(screen.contains("Tests by area"), "{screen}");
        assert!(screen.contains("0 of 1 files named by no test"), "{screen}");

        // An area narrows the list to the tests that count towards it; Esc widens it again.
        let area = rows(&app)
            .iter()
            .position(|r| matches!(r, SpecRow::Area(a) if a.name == "src"))
            .unwrap();
        go_to(&mut app, area);
        press(&mut app, &[KeyCode::Enter]);
        let screen = draw(&mut term, &mut app);
        assert!(screen.contains("Area src"), "{screen}");
        assert!(!rows(&app).iter().any(|r| matches!(r, SpecRow::Area(_))));

        // A test says what it names, and f lists it to jump to.
        let test = rows(&app)
            .iter()
            .position(|r| matches!(r, SpecRow::Test(..)))
            .unwrap();
        go_to(&mut app, test);
        let screen = draw(&mut term, &mut app);
        assert!(screen.contains("names core.rs (f)"), "{screen}");
        press(&mut app, &[KeyCode::Char('f')]);
        assert!(matches!(app.screen(), Screen::Locations(_)));
        press(&mut app, &[KeyCode::Enter]);
        assert!(matches!(app.screen(), Screen::Explorer));
        assert_eq!(app.viewer.as_ref().unwrap().path, "src/core.rs");

        // And back the other way: U on the file lists the tests that name it.
        press(&mut app, &[KeyCode::Char('U')]);
        let screen = draw(&mut term, &mut app);
        assert!(screen.contains("1 test names core.rs"), "{screen}");
        assert!(screen.contains("a review parses"), "{screen}");

        // Back on the specification, Esc first widens the area, then leaves.
        press(&mut app, &[KeyCode::Char('q'), KeyCode::Char('P')]);
        go_to(&mut app, area);
        press(&mut app, &[KeyCode::Enter, KeyCode::Esc]);
        assert!(matches!(app.screen(), Screen::Spec(_)));
        assert!(rows(&app).iter().any(|r| matches!(r, SpecRow::Area(_))));
        // Every test is a section's first screen, so leaving it goes to the end-to-end ones.
        press(&mut app, &[KeyCode::Esc]);
        assert_eq!(app.section, Section::Tests);
        assert!(matches!(app.screen(), Screen::EndToEnd(_)));
    }

    /// Thirty tests in two files, all added by the last commit: enough that every list folds.
    #[test]
    fn long_lists_fold_and_search() {
        use super::app::SpecRow;
        let dir = scratch_repo();
        let git = |args: &[&str]| {
            let out = std::process::Command::new("git")
                .arg("-C")
                .arg(dir.path())
                .args(args)
                .output()
                .unwrap();
            assert!(out.status.success(), "{args:?}");
        };
        for file in ["one", "two"] {
            let tests: String = (0..15)
                .map(|i| format!("#[test]\nfn {file}_number_{i}() {{\n    assert!(true);\n}}\n\n"))
                .collect();
            std::fs::write(dir.path().join(format!("{file}.rs")), tests).unwrap();
        }
        git(&["add", "."]);
        git(&["commit", "-q", "-m", "thirty tests"]);
        let mut app = App::new(scratch_session(dir.path()), None);
        let mut term = Terminal::new(TestBackend::new(140, 50)).unwrap();
        let rows = |app: &App| match app.screen() {
            Screen::Spec(s) => s.rows.items.clone(),
            _ => panic!("not the specification"),
        };
        let fold = |app: &App, label: &str| {
            rows(app)
                .iter()
                .position(|r| matches!(r, SpecRow::Fold { label: l, .. } if l == label))
                .unwrap_or_else(|| panic!("no fold {label}"))
        };
        let go_to = |app: &mut App, at: usize| {
            press(app, &[KeyCode::Home]);
            press(app, &vec![KeyCode::Char('j'); at]);
        };
        let tests = |app: &App| {
            rows(app)
                .iter()
                .filter(|r| matches!(r, SpecRow::Test(..)))
                .count()
        };

        // The last commit added thirty tests: one closed row says so, not thirty.
        press(&mut app, &[KeyCode::Char('P'), KeyCode::Char('s')]);
        let screen = draw(&mut term, &mut app);
        assert!(screen.contains("▸ added 30"), "{screen}");
        assert!(!rows(&app).iter().any(|r| matches!(r, SpecRow::Change(..))));
        // Opened, it is two files of fifteen, each folded.
        let added = fold(&app, "added");
        go_to(&mut app, added);
        press(&mut app, &[KeyCode::Enter]);
        let screen = draw(&mut term, &mut app);
        assert!(screen.contains("▸ one.rs 15"), "{screen}");
        assert!(!rows(&app).iter().any(|r| matches!(r, SpecRow::Change(..))));

        // The tests as they are: the largest group is folded, and opens to files, then tests.
        assert_eq!(tests(&app), 0, "thirty tests start folded");
        let unit = fold(&app, "No interaction");
        go_to(&mut app, unit);
        press(&mut app, &[KeyCode::Enter]);
        let one = rows(&app)
            .iter()
            .rposition(|r| matches!(r, SpecRow::Fold { label, .. } if label == "one.rs"))
            .unwrap();
        go_to(&mut app, one);
        press(&mut app, &[KeyCode::Enter]);
        assert_eq!(tests(&app), 15);
        // h, from a test inside, closes the file it is in and leaves the cursor on it.
        press(&mut app, &[KeyCode::Char('j'), KeyCode::Char('h')]);
        assert_eq!(tests(&app), 0);
        match app.screen() {
            Screen::Spec(s) => assert!(
                matches!(&s.rows.items[s.rows.cursor], SpecRow::Fold { label, .. } if label == "one.rs")
            ),
            _ => panic!(),
        }

        // A search, as it is typed: every word must match; Esc clears it, then leaves.
        press(&mut app, &[KeyCode::Char('/')]);
        for ch in "two 1".chars() {
            app.handle_key(key(ch));
        }
        let screen = draw(&mut term, &mut app);
        assert!(screen.contains("6 tests match \"two 1\""), "{screen}");
        assert_eq!(tests(&app), 6, "two_number_1 and 10 to 14");
        press(&mut app, &[KeyCode::Enter, KeyCode::Esc]);
        assert_eq!(tests(&app), 0);
        assert!(matches!(app.screen(), Screen::Spec(_)));
    }

    #[test]
    fn a_repository_that_appears_later_joins_the_workspace() {
        use super::workspace::Workspace;
        let parent = tempfile::tempdir().unwrap();
        let init = |name: &str| {
            let dir = parent.path().join(name);
            std::fs::create_dir(&dir).unwrap();
            std::process::Command::new("git")
                .args(["init", "-q"])
                .current_dir(&dir)
                .output()
                .unwrap();
        };
        init("first");
        let config = parent.path().join("config.toml");
        let opened =
            crate::session::open_targets(parent.path(), &[], Some(config), |_| {}).unwrap();
        let mut ws = Workspace::new(opened.targets);
        ws.watch(opened.checkouts);
        assert!(!ws.find_new());
        init("second");
        assert!(ws.find_new());
        assert_eq!(ws.apps.len(), 2);
        assert_eq!(ws.active, 0, "the reader stays where they were");
        let message = &ws.app().message.as_ref().unwrap().0;
        assert!(message.contains("new repository 2: second"), "{message}");
        let mut term = Terminal::new(TestBackend::new(120, 24)).unwrap();
        let strip = draw_ws(&mut term, &mut ws)
            .lines()
            .next()
            .unwrap()
            .to_string();
        assert!(strip.contains("2 second"), "{strip}");
        assert!(!ws.find_new(), "found once");
    }

    #[test]
    fn a_directory_of_checkouts_opens_on_the_repository_list() {
        use super::workspace::Workspace;
        let parent = tempfile::tempdir().unwrap();
        for name in ["alpha", "beta"] {
            let dir = parent.path().join(name);
            std::fs::create_dir(&dir).unwrap();
            std::process::Command::new("git")
                .args(["init", "-q", "-b", "trunk"])
                .current_dir(&dir)
                .output()
                .unwrap();
        }
        let config = parent.path().join("config.toml");
        let opened =
            crate::session::open_targets(parent.path(), &[], Some(config), |_| {}).unwrap();
        let mut ws = Workspace::new(opened.targets);
        ws.open_home();
        ws.watch(opened.checkouts);
        let mut term = Terminal::new(TestBackend::new(120, 24)).unwrap();
        let screen = draw_ws(&mut term, &mut ws);
        assert!(screen.contains("Select repository · 2"), "{screen}");
        assert!(screen.contains(" 1 alpha trunk"), "{screen}");
        assert!(screen.contains(" 2 beta  trunk"), "{screen}");
        assert!(
            !screen.contains("gt/gT"),
            "no strip over the list: {screen}"
        );
        // Esc has nothing to close onto; Enter opens the highlighted one.
        ws.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(ws.selecting);
        ws.handle_key(key('j'));
        ws.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(!ws.selecting);
        assert_eq!(ws.active, 1);
        let screen = draw_ws(&mut term, &mut ws);
        assert!(!screen.contains("Select repository"), "{screen}");
        // `q` from its last screen comes back to the list, on the one just left.
        ws.handle_key(key('q'));
        assert!(ws.selecting);
        assert_eq!(ws.picker, Some(1));
        assert!(!ws.quit());
        // A number opens one directly, even the only one; Ctrl-C quits from inside it.
        ws.handle_key(key('1'));
        assert_eq!(ws.active, 0);
        ws.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
        assert!(ws.quit());
        // `q` on the list quits.
        let opened = crate::session::open_targets(
            parent.path(),
            &[],
            Some(parent.path().join("config.toml")),
            |_| {},
        )
        .unwrap();
        let mut ws = Workspace::new(opened.targets);
        ws.open_home();
        ws.handle_key(key('q'));
        assert!(ws.quit());
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
        app.switch_section(Section::Code);
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
        app.switch_section(Section::Code);
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
