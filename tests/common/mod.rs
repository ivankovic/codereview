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

/// An LLVM tool of the toolchain that builds the tests: `llvm-profdata` or `llvm-cov`, which
/// `rustup component add llvm-tools` puts beside the compiler.
fn llvm_tool(name: &str) -> Option<std::path::PathBuf> {
    let sysroot = Command::new("rustc")
        .args(["--print", "sysroot"])
        .output()
        .ok()?;
    let sysroot = std::path::PathBuf::from(String::from_utf8_lossy(&sysroot.stdout).trim());
    std::fs::read_dir(sysroot.join("lib/rustlib"))
        .ok()?
        .flatten()
        .map(|e| e.path().join("bin").join(name))
        .find(|p| p.is_file())
}

/// The lines `binary` ran, as lcov, from the coverage profiles it wrote. `None` when there is
/// no profile or no tool to read one: a test's coverage is something to show, not a check.
pub fn lcov_of(profiles: &[std::path::PathBuf], binary: &str) -> Option<Vec<u8>> {
    if profiles.is_empty() {
        return None;
    }
    let work = tempfile::tempdir().ok()?;
    let merged = work.path().join("merged.profdata");
    let ok = Command::new(llvm_tool("llvm-profdata")?)
        .args(["merge", "-sparse", "-o"])
        .arg(&merged)
        .args(profiles)
        .status()
        .is_ok_and(|s| s.success());
    if !ok {
        return None;
    }
    let out = Command::new(llvm_tool("llvm-cov")?)
        .args([
            "export",
            "-format=lcov",
            "-ignore-filename-regex=(\\.cargo/registry|/rustc/)",
        ])
        .arg("-instr-profile")
        .arg(&merged)
        .arg(binary)
        .output()
        .ok()?;
    out.status.success().then_some(out.stdout)
}

/// A test's `coverage.lcov`, and when it was made.
pub struct Kept(Vec<u8>, std::time::SystemTime);

/// The coverage the last counting run left in `dir`, to keep across a run that does not
/// count: without it every plain run of the tests would forget what each one runs.
pub fn keep_coverage(dir: &Path) -> Option<Kept> {
    let file = dir.join("coverage.lcov");
    let made = std::fs::metadata(&file).and_then(|m| m.modified()).ok()?;
    Some(Kept(std::fs::read(&file).ok()?, made))
}

/// Puts kept coverage back in `dir` as it was, made when it was made: a reader judges by that
/// whether it knows the code as it is now.
pub fn restore_coverage(dir: &Path, kept: Kept) {
    let file = dir.join("coverage.lcov");
    if std::fs::write(&file, kept.0).is_ok() {
        if let Ok(f) = std::fs::File::options().write(true).open(&file) {
            let _ = f.set_modified(kept.1);
        }
    }
}
