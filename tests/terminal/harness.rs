//! The binary in a pseudo-terminal, a screen read the way a terminal would draw it, and a
//! recording of the whole run.
//!
//! A test drives a `Term`: it presses keys, waits for text to appear, and checks things, and
//! each of those is a step with the line of the test that took it. When the `Term` goes, it
//! writes what happened under `target/e2e/terminal/<test>/`: `terminal.cast`, an asciicast v2
//! recording of everything the program drew and every key it was sent, and `test.json`, which
//! names the test, says whether it passed and lists its steps against the recording's clock.
//! The browser's end-to-end tests leave the same `test.json` beside a video, so a reviewer
//! watches either the same way. It is written when a test fails too, which is when it is most
//! worth watching.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime};

use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};
use serde_json::{Value, json};

pub const ENTER: &str = "\r";
pub const ESC: &str = "\x1b";
pub const TAB: &str = "\t";
pub const DOWN: &str = "\x1b[B";
pub const UP: &str = "\x1b[A";

/// How long a screen has to show what a test waits for. Generous: a debug build takes a second
/// or two to load the syntax definitions the first time a file is drawn, a parallel run on a
/// loaded machine is slower still, and a wait that is met returns at once.
const PATIENCE: Duration = Duration::from_secs(20);

/// How long the recording holds on a screen before the next key: run at machine speed, a test
/// is over before anyone watching has seen what it did. Text is typed a character at a time,
/// at a typist's pace.
const HOLD: Duration = Duration::from_millis(450);
const KEYSTROKE: Duration = Duration::from_millis(45);

/// Wide enough for the side panel and a file beside it, short enough to read in a recording.
const WIDTH: u16 = 110;
const HEIGHT: u16 = 32;

/// What the program drew and what it was sent, against the time since the run started.
#[derive(Default)]
struct Recording {
    events: Vec<(f64, &'static str, String)>,
}

struct Step {
    at_ms: u128,
    line: u32,
    kind: &'static str,
    text: String,
    passed: Option<bool>,
    detail: Option<String>,
}

/// Starts a `Term` for the test it is written in, named after its function; with a closure,
/// that closure changes the scratch repository first.
macro_rules! term {
    () => {
        term!(|_: &std::path::Path| {})
    };
    ($setup:expr) => {
        term!($setup, &[])
    };
    ($setup:expr, $args:expr) => {{
        fn here() {}
        let path = std::any::type_name_of_val(&here);
        let name = path.trim_end_matches("::here").rsplit("::").next().unwrap();
        $crate::harness::Term::start_with(name, file!(), $setup, $args)
    }};
}

/// The program running in a terminal, and the record of what a test did to it.
pub struct Term {
    name: String,
    file: &'static str,
    started: Instant,
    started_at: SystemTime,
    child: Box<dyn Child + Send + Sync>,
    writer: Box<dyn Write + Send>,
    // Kept so the terminal stays open until the test is done with it.
    _master: Box<dyn MasterPty + Send>,
    screen: Arc<Mutex<vt100::Parser>>,
    recording: Arc<Mutex<Recording>>,
    reader: Option<JoinHandle<()>>,
    steps: Vec<Step>,
    /// The documentation the test demonstrates, as `path#heading`.
    docs: Vec<String>,
    /// Whether the keys pressed now are already described by a step: `step` says what the
    /// next keys are for, and a check ends that.
    described: bool,
    // Dropped after the program is, which `Drop` sees to.
    repo: tempfile::TempDir,
    _home: tempfile::TempDir,
    /// Where an instrumented binary writes its coverage profile, when the tests run under
    /// `cargo llvm-cov`: one directory per test, so each test's coverage is its own.
    profiles: Option<tempfile::TempDir>,
    /// When the test was over: what the program drew after that, leaving, is not part of it.
    ended: Option<f64>,
}

/// `name` as a directory name: lower case, anything else between words a single dash.
pub fn slug(name: &str) -> String {
    let mut out = String::new();
    for c in name.chars() {
        if c.is_alphanumeric() {
            out.extend(c.to_lowercase());
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_matches('-').to_string()
}

/// The 1-based line of `fn name(` in `file`, read from the source itself so that it is the
/// line the test is defined on, wherever in its body the `Term` was started.
fn line_of(file: &str, name: &str) -> usize {
    let source = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join(file)).unwrap();
    let needle = format!("fn {name}(");
    source
        .lines()
        .position(|l| l.contains(&needle))
        .map_or(0, |i| i + 1)
}

impl Term {
    /// Starts the binary in a fresh scratch repository, with a home and a configuration file
    /// of its own, so that nothing of the person running the tests is read or written.
    ///
    /// `name` is the test's function name as written and `file` the file it is in; `term!`
    /// fills both in.
    /// `setup` changes the scratch repository before the program is started in it, and `args`
    /// are its command line: none for the terminal UI.
    pub fn start_with(
        name: &str,
        file: &'static str,
        setup: impl FnOnce(&Path),
        args: &[&str],
    ) -> Term {
        let repo = scratch_repo();
        setup(repo.path());
        let home = tempfile::tempdir().unwrap();
        let pty = native_pty_system()
            .openpty(PtySize {
                rows: HEIGHT,
                cols: WIDTH,
                pixel_width: 0,
                pixel_height: 0,
            })
            .expect("a pseudo-terminal");
        let mut cmd = CommandBuilder::new(env!("CARGO_BIN_EXE_codereview"));
        cmd.args(args);
        cmd.cwd(repo.path());
        cmd.env("TERM", "xterm-256color");
        cmd.env("HOME", home.path());
        cmd.env("CODEREVIEW_CONFIG", home.path().join("config.toml"));
        let profiles = std::env::var_os("CARGO_LLVM_COV").map(|_| tempfile::tempdir().unwrap());
        if let Some(dir) = &profiles {
            cmd.env(
                "LLVM_PROFILE_FILE",
                dir.path().join("codereview-%p-%m.profraw"),
            );
        }
        cmd.env_remove("CODEREVIEW_PASSWORD_HASH");
        let child = pty.slave.spawn_command(cmd).expect("the binary starts");
        // The program holds the other end now; ours would keep the terminal open after it
        // exits, and the reader below would never see the end.
        drop(pty.slave);

        let screen = Arc::new(Mutex::new(vt100::Parser::new(HEIGHT, WIDTH, 0)));
        let recording = Arc::new(Mutex::new(Recording::default()));
        let started = Instant::now();
        let mut source = pty.master.try_clone_reader().unwrap();
        let reader = {
            let screen = screen.clone();
            let recording = recording.clone();
            std::thread::spawn(move || {
                let mut buf = [0u8; 8192];
                // A character can be split between two reads; its first bytes wait here for
                // the rest, since the recording is text.
                let mut partial = Vec::new();
                loop {
                    let n = match source.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => n,
                    };
                    let at = started.elapsed().as_secs_f64();
                    screen.lock().unwrap().process(&buf[..n]);
                    partial.extend_from_slice(&buf[..n]);
                    let whole = match std::str::from_utf8(&partial) {
                        Ok(_) => partial.len(),
                        Err(e) if e.error_len().is_none() => e.valid_up_to(),
                        // Not UTF-8 at all: recorded as best it can be rather than held forever.
                        Err(_) => partial.len(),
                    };
                    let text = String::from_utf8_lossy(&partial[..whole]).into_owned();
                    partial.drain(..whole);
                    if !text.is_empty() {
                        recording.lock().unwrap().events.push((at, "o", text));
                    }
                }
            })
        };
        let writer = pty.master.take_writer().unwrap();
        Term {
            name: name.to_string(),
            file,
            started,
            started_at: SystemTime::now(),
            child,
            writer,
            _master: pty.master,
            screen,
            recording,
            reader: Some(reader),
            steps: Vec::new(),
            described: false,
            docs: Vec::new(),
            repo,
            _home: home,
            profiles,
            ended: None,
        }
    }

    /// Waits for the end-to-end tests, the first section, which a repository opens on.
    #[track_caller]
    pub fn on_the_tests(&mut self) {
        let line = std::panic::Location::caller().line();
        self.wait_until(line, "shows the end-to-end tests", |s| {
            s.contains("end-to-end test") || s.contains("No end-to-end tests")
        });
    }

    /// Goes from the end-to-end tests, which a repository opens on, to the code, the third
    /// section: the file tree and the file beside it.
    #[track_caller]
    pub fn go_to_the_code(&mut self) {
        let line = std::panic::Location::caller().line();
        self.on_the_tests();
        self.push(line, "action", "go to the code (3)".into(), None, None);
        self.described = true;
        self.keys("3");
        self.wait_until(line, "shows “files 5”", |s| s.contains("files 5"));
    }

    /// The scratch repository the program was started in.
    pub fn repo(&self) -> &Path {
        self.repo.path()
    }

    fn at_ms(&self) -> u128 {
        self.started.elapsed().as_millis()
    }

    /// Names the section of the documentation this test shows working, as `path#heading`:
    /// where a reviewer watching it is led to read what it is meant to do.
    pub fn documents(&mut self, section: &str) {
        self.docs.push(section.to_string());
    }

    /// Says what the keys pressed next are for.
    #[track_caller]
    pub fn step(&mut self, text: &str) {
        let line = std::panic::Location::caller().line();
        self.push(line, "action", text.to_string(), None, None);
        self.described = true;
    }

    /// Sends `keys` as typed. Without a `step` before them they are a step of their own.
    #[track_caller]
    pub fn keys(&mut self, keys: &str) {
        let line = std::panic::Location::caller().line();
        if !self.described {
            self.push(line, "action", format!("press {}", shown(keys)), None, None);
        }
        std::thread::sleep(HOLD);
        // One key, or one escape sequence, at a time; anything longer is text, typed.
        let presses: Vec<String> = if keys.starts_with('\x1b') || keys.chars().count() == 1 {
            vec![keys.to_string()]
        } else {
            keys.chars().map(String::from).collect()
        };
        for press in presses {
            let at = self.started.elapsed().as_secs_f64();
            self.recording
                .lock()
                .unwrap()
                .events
                .push((at, "i", press.clone()));
            self.writer.write_all(press.as_bytes()).unwrap();
            self.writer.flush().unwrap();
            // A key that arrives with the next one in the same read is taken for an escape
            // sequence; a moment between them is what a person's fingers leave anyway.
            std::thread::sleep(KEYSTROKE);
        }
    }

    /// The screen as text, a line per row.
    pub fn screen(&self) -> String {
        self.screen.lock().unwrap().screen().contents()
    }

    /// Whether the program has the terminal on its alternate screen, as a full-screen
    /// program does while it runs and must undo when it ends.
    pub fn alternate_screen(&self) -> bool {
        self.screen.lock().unwrap().screen().alternate_screen()
    }

    /// Waits until the screen shows `text`, and fails the test if it does not in time.
    #[track_caller]
    pub fn wait_for(&mut self, text: &str) {
        let line = std::panic::Location::caller().line();
        self.wait_until(line, &format!("shows “{text}”"), |s| s.contains(text));
    }

    fn wait_until(&mut self, line: u32, what: &str, ok: impl Fn(&str) -> bool) {
        let deadline = Instant::now() + PATIENCE;
        loop {
            let screen = self.screen();
            if ok(&screen) {
                self.push(line, "check", what.to_string(), Some(true), None);
                return;
            }
            if Instant::now() > deadline {
                self.fail(line, what, format!("the screen was:\n{screen}"));
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// A check of something other than the screen: a file on disk, the exit status.
    #[track_caller]
    pub fn check(&mut self, what: &str, passed: bool, detail: impl FnOnce() -> String) {
        let line = std::panic::Location::caller().line();
        if !passed {
            self.fail(line, what, detail());
        }
        self.push(line, "check", what.to_string(), Some(true), None);
    }

    /// Waits for the program to end by itself, and says whether it ended well.
    #[track_caller]
    pub fn exits(&mut self) {
        let line = std::panic::Location::caller().line();
        let deadline = Instant::now() + PATIENCE;
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                if status.success() {
                    self.push(line, "check", "exits cleanly".into(), Some(true), None);
                    return;
                }
                self.fail(line, "exits cleanly", format!("it exited with {status:?}"));
            }
            if Instant::now() > deadline {
                let screen = self.screen();
                self.fail(line, "exits cleanly", format!("still running:\n{screen}"));
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn fail(&mut self, line: u32, what: &str, detail: String) -> ! {
        self.push(
            line,
            "check",
            what.to_string(),
            Some(false),
            Some(detail.clone()),
        );
        panic!("{what}: {detail}");
    }

    fn push(
        &mut self,
        line: u32,
        kind: &'static str,
        text: String,
        passed: Option<bool>,
        detail: Option<String>,
    ) {
        if kind == "check" {
            self.described = false;
        }
        let at_ms = self.at_ms();
        self.steps.push(Step {
            at_ms,
            line,
            kind,
            text,
            passed,
            detail,
        });
    }

    fn write_out(&mut self) {
        let duration_ms = self.at_ms();
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target/e2e/terminal")
            .join(slug(&self.name));
        // A run that does not count keeps what the last one that did said this test ran.
        let kept = if self.profiles.is_none() {
            crate::common::keep_coverage(&dir)
        } else {
            None
        };
        // This test's directory only: the others run at the same time in other processes.
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        if let Some(kept) = kept {
            crate::common::restore_coverage(&dir, kept);
        }

        let unix = self
            .started_at
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let mut cast = json!({
            "version": 2,
            "width": WIDTH,
            "height": HEIGHT,
            "timestamp": unix,
            "env": { "TERM": "xterm-256color" },
        })
        .to_string();
        cast.push('\n');
        let ended = self.ended.unwrap_or(f64::MAX);
        for (at, kind, text) in &self.recording.lock().unwrap().events {
            if *at > ended {
                break;
            }
            cast.push_str(&json!([at, kind, text]).to_string());
            cast.push('\n');
        }
        std::fs::write(dir.join("terminal.cast"), cast).unwrap();
        if let Some(profiles) = &self.profiles {
            coverage(profiles.path(), &dir.join("coverage.lcov"));
        }

        let steps: Vec<Value> = self
            .steps
            .iter()
            .map(|s| {
                let mut v = json!({
                    "at_ms": s.at_ms,
                    "line": s.line,
                    "kind": s.kind,
                    "text": s.text,
                });
                if let Some(p) = s.passed {
                    v["passed"] = p.into();
                }
                if let Some(d) = &s.detail {
                    v["detail"] = d.clone().into();
                }
                v
            })
            .collect();
        let (commit, dirty) = this_commit();
        let started: chrono::DateTime<chrono::Utc> = self.started_at.into();
        let test = json!({
            "test": self.name,
            "file": self.file,
            "line": line_of(self.file, &self.name),
            "passed": !std::thread::panicking(),
            "started": started.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            "duration_ms": duration_ms,
            "commit": commit,
            "dirty": dirty,
            "recording": "terminal.cast",
            "docs": self.docs,
            "steps": steps,
        });
        std::fs::write(
            dir.join("test.json"),
            serde_json::to_string_pretty(&test).unwrap() + "\n",
        )
        .unwrap();
    }
}

impl Drop for Term {
    fn drop(&mut self) {
        // The last screen is what the test was about: the recording holds on it too.
        std::thread::sleep(HOLD);
        self.ended = Some(self.started.elapsed().as_secs_f64());
        // Asked to leave rather than killed: a program writes its coverage profile on its
        // way out, and a killed one writes nothing. Esc first, out of any prompt that would
        // take Ctrl-C as text.
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.writer.write_all(ESC.as_bytes());
            let _ = self.writer.flush();
            std::thread::sleep(KEYSTROKE);
            let _ = self.writer.write_all(b"\x03");
            let _ = self.writer.flush();
            let deadline = Instant::now() + Duration::from_secs(3);
            while self.child.try_wait().ok().flatten().is_none() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(20));
            }
        }
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
        // The reader ends when the program does; whatever it drew last belongs in the
        // recording. Not joined unbounded: a grandchild holding the terminal open would
        // otherwise hang the test.
        if let Some(reader) = self.reader.take() {
            let deadline = Instant::now() + Duration::from_secs(20);
            while !reader.is_finished() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        self.write_out();
    }
}

/// Turns the coverage profiles the program wrote in `profiles` into `out`, an lcov file of
/// the lines it ran. Nothing is written when there is no profile or no tool to read one.
fn coverage(profiles: &Path, out: &Path) {
    let raw: Vec<PathBuf> = std::fs::read_dir(profiles)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "profraw"))
        .collect();
    if let Some(lcov) = crate::common::lcov_of(&raw, env!("CARGO_BIN_EXE_codereview")) {
        let _ = std::fs::write(out, lcov);
    }
}

/// Keys as a person would name them.
fn shown(keys: &str) -> String {
    match keys {
        ENTER => "Enter".into(),
        ESC => "Esc".into(),
        TAB => "Tab".into(),
        DOWN => "Down".into(),
        UP => "Up".into(),
        _ => keys.to_string(),
    }
}

/// The commit of the code under test, and whether the tree has changes on top of it: a
/// recording made from uncommitted code is not a recording of that commit.
fn this_commit() -> (Option<String>, bool) {
    let git = |args: &[&str]| {
        Command::new("git")
            .arg("-C")
            .arg(env!("CARGO_MANIFEST_DIR"))
            .args(args)
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
    };
    let commit = git(&["rev-parse", "HEAD"]);
    let dirty = git(&["status", "--porcelain"]).is_some_and(|s| !s.is_empty());
    (commit, dirty)
}

/// A scratch repository like a small real one: two commits, an uncommitted edit and an
/// untracked file, a README to say what it is, and a test at each end of the scale to read as
/// a specification.
fn scratch_repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let git = |args: &[&str]| {
        let out = Command::new("git")
            .arg("-C")
            .arg(&root)
            .args(args)
            .env("HOME", &root)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    };
    let write = |path: &str, text: &str| {
        let path: PathBuf = root.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    };
    git(&["init", "-q", "-b", "main"]);
    git(&["config", "user.email", "t@example.com"]);
    git(&["config", "user.name", "Tester"]);
    write(
        "README.md",
        "# scratch\n\nA small repository the terminal tests look at. It has a README for the \
         overview to quote, a module the rest of it leans on, and tests to read.\n",
    );
    write(
        "src/review.rs",
        "pub fn parse_review() {}\n\npub fn write_review() {}\n\n#[cfg(test)]\nmod tests {\n    \
         #[test]\n    fn a_review_round_trips() {\n        super::parse_review();\n        assert!(true);\n    }\n}\n",
    );
    write(
        "src/main.rs",
        "mod review;\n\nfn main() {\n    review::parse_review();\n}\n",
    );
    write(
        "tests/e2e.rs",
        "#[test]\nfn a_review_survives_a_restart() {\n    assert!(true);\n}\n",
    );
    git(&["add", "."]);
    git(&["commit", "-q", "-m", "the first version"]);
    write(
        "src/main.rs",
        "mod review;\n\nfn main() {\n    review::parse_review();\n    review::write_review();\n}\n",
    );
    git(&["add", "."]);
    git(&["commit", "-q", "-m", "write the review back"]);
    write(
        "src/main.rs",
        "mod review;\n\nfn main() {\n    review::parse_review();\n    review::write_review();\n    \
         println!(\"done\");\n}\n",
    );
    write("notes.txt", "an untracked file\n");
    dir
}
