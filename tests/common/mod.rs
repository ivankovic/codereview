//! What the tests that need a container share. Each test binary compiles its own copy, so not
//! every one of these is used by every one of them.
#![allow(dead_code)]

use std::net::{SocketAddr, TcpStream};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// The container runtime to drive, from `CODEREVIEW_CONTAINER_RUNTIME`.
///
/// Podman, and rootless: it runs without a daemon, so these tests cost nobody membership of a
/// group whose socket hands over every container on the machine, including root-owned ones
/// that have nothing to do with this project. Anything CLI-compatible does — `docker` is a
/// drop-in for the handful of commands used here. Read once: a run must not straddle two
/// runtimes.
pub fn runtime() -> &'static str {
    static RUNTIME: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    RUNTIME
        .get_or_init(|| {
            std::env::var("CODEREVIEW_CONTAINER_RUNTIME").unwrap_or_else(|_| "podman".to_string())
        })
        .as_str()
}

/// Whether `program` is there to be run. `probe` is how it is asked, which is not the same
/// argument everywhere.
pub fn have(program: &str, probe: &str) -> bool {
    Command::new(program)
        .arg(probe)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

pub fn run(program: &str, args: &[&str]) -> String {
    let out = Command::new(program)
        .args(args)
        .output()
        .unwrap_or_else(|e| panic!("cannot run {program}: {e}"));
    assert!(
        out.status.success(),
        "{program} {args:?} failed: {}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

pub fn port_is_free(port: u16) -> bool {
    std::net::TcpListener::bind(("127.0.0.1", port)).is_ok()
}

pub fn wait_for_port(port: u16, what: &str) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        let addr = SocketAddr::from(([127, 0, 0, 1], port));
        if TcpStream::connect_timeout(&addr, Duration::from_millis(200)).is_ok() {
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    panic!("{what} never came up on port {port}");
}

/// A repository with one commit, a working-tree edit, and both Markdown files, built with the
/// real git so that the tests see what a user's own repository looks like.
pub fn scratch_repo(root: &Path) {
    run("git", &["init", "-q", root.to_str().unwrap()]);
    let git = |args: &[&str]| {
        let mut all = vec!["-C", root.to_str().unwrap()];
        all.extend_from_slice(args);
        run("git", &all);
    };
    git(&["config", "user.email", "test@example.com"]);
    git(&["config", "user.name", "Test"]);
    std::fs::write(root.join("a.rs"), "fn main() {}\n").unwrap();
    std::fs::write(root.join("REVIEW.md"), "# Pending\n\n# Completed\n").unwrap();
    git(&["add", "-A"]);
    git(&["commit", "-qm", "first"]);
}
