//! The page in a real browser, at a real phone's size: the browser UI's end-to-end tests.
//!
//! Every other test in this repository reads what the server sends. That is exactly the check
//! that keeps passing while the page is unusable: a menu clipped by its own parent, a header
//! that stopped sticking, a form with its buttons under the keyboard. None of those change a
//! byte of what the server sends, and every one of them has shipped.
//!
//! So: a headless Chromium in a container, the server on the host, and a script that taps
//! things and asks what a finger would have hit. It is ignored by default like the nginx one
//! and run by `make browser-test`, which needs podman.
//!
//! The script holds one test per behaviour, and each leaves a video of itself and a
//! `test.json` of its steps, which end up in `target/e2e/browser/<test>/` for codereview to
//! show beside the test. They are made in the container and copied out of it afterwards.
//!
//! The script is handed to node as an argument rather than mounted, because a bind mount into
//! a rootless container has to survive the user namespace mapping the file's owner, and an
//! argument has nothing to survive. The same goes for the recordings on the way out, which is
//! why the container is copied from rather than written through.

mod common;

use common::{have, lcov_of, port_is_free, run, runtime, scratch_repo, wait_for_port};
use std::process::{Child, Command, Stdio};

/// Well above the privileged range: the container shares the host's network, so this is a host
/// port and a rootless runtime may not bind the low ones.
const PORT: u16 = 18766;
const TOKEN: &str = "browser-test-token-long-enough";
/// Chromium, puppeteer and ffmpeg in one image, so that neither a browser nor a node toolchain
/// has to exist on the machine running the tests. Built here from a published image, and
/// tagged with a hash of what it is built from, so that a change to it is a new image.
const CONTAINERFILE: &str = include_str!("browser/Containerfile");

/// The page's own checks, run inside the container against the server below.
const PHONE: &str = include_str!("browser/phone.js");

struct Serving(Child);

impl Drop for Serving {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// A stopped container, removed when the recordings have been copied out of it, or when the
/// test fails before they could be.
struct Container(String);

impl Drop for Container {
    fn drop(&mut self) {
        let _ = Command::new(runtime())
            .args(["rm", "-f", &self.0])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

/// The image the tests run in, built the first time it is needed. Built rather than pulled by
/// the run below, which would spend a silent minute on a machine that has never run this
/// before and then look like a hung test.
fn image() -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in CONTAINERFILE.bytes() {
        hash = (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3);
    }
    let tag = format!("localhost/codereview-browser-test:{:012x}", hash >> 16);
    if run(runtime(), &["images", "-q", &tag]).trim().is_empty() {
        eprintln!("building {tag} — about a gigabyte of browser, once");
        let mut build = Command::new(runtime())
            .args(["build", "-t", &tag, "-"])
            .stdin(Stdio::piped())
            .spawn()
            .expect("cannot run the container runtime");
        use std::io::Write;
        build
            .stdin
            .take()
            .unwrap()
            .write_all(CONTAINERFILE.as_bytes())
            .unwrap();
        assert!(build.wait().unwrap().success(), "building {tag} failed");
    }
    tag
}

/// Whether the tests run under `cargo llvm-cov`, and the server is built to count what it runs.
fn counting() -> bool {
    std::env::var_os("CARGO_LLVM_COV").is_some()
}

/// Puts together each browser test's `coverage.lcov`: what the page's script ran, which the
/// script wrote beside the video, and, when the server counted, what it ran, from the profile
/// it wrote for that test in `profiles`.
fn gather_coverage(recordings: &std::path::Path, profiles: &std::path::Path) {
    let Ok(dirs) = std::fs::read_dir(recordings) else {
        return;
    };
    for dir in dirs.flatten().map(|e| e.path()) {
        let page = dir.join("page.lcov");
        let mut lcov = std::fs::read(&page).unwrap_or_default();
        let _ = std::fs::remove_file(&page);
        let slug = dir.file_name().unwrap().to_string_lossy().into_owned();
        let profile = profiles.join(format!("{slug}.profraw"));
        if profile.is_file() {
            if let Some(more) = lcov_of(&[profile], env!("CARGO_BIN_EXE_codereview")) {
                lcov.extend(more);
            }
        }
        if !lcov.is_empty() {
            std::fs::write(dir.join("coverage.lcov"), lcov).unwrap();
        }
    }
}

/// Which commit of codereview is under test, and whether the tree has changes on top of it,
/// for the recordings to say what they are a recording of; and which test ran them.
fn under_test() -> String {
    let root = env!("CARGO_MANIFEST_DIR");
    let git = |args: &[&str]| {
        Command::new("git")
            .arg("-C")
            .arg(root)
            .args(args)
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
    };
    // The page's script starts on the line of its `<script>` tag, which is where the lines
    // Chromium says it ran are counted from.
    let script_line = include_str!("../src/web/index.html")
        .lines()
        .position(|l| l.trim() == "<script>")
        .map(|i| i + 1);
    serde_json::json!({
        "coverage": counting(),
        "script_line": script_line,
        "commit": git(&["rev-parse", "HEAD"]),
        "dirty": git(&["status", "--porcelain"]).is_some_and(|s| !s.is_empty()),
        // This test only hands the script to a browser: the tests are the script's, and
        // their recordings say so, so that this one is not listed as a test of its own.
        "run_by": { "file": "tests/browser.rs", "test": "every_browser_test_passes_in_a_real_browser" },
    })
    .to_string()
}

#[test]
#[ignore = "needs podman (or docker); run with `make browser-test`"]
fn every_browser_test_passes_in_a_real_browser() {
    for (program, probe) in [(runtime(), "--version"), ("git", "--version")] {
        assert!(have(program, probe), "{program} is needed for this test");
    }
    assert!(port_is_free(PORT), "port {PORT} is already in use");
    let image = image();

    let work = tempfile::tempdir().unwrap();
    let repo = work.path().join("repo");
    scratch_repo(&repo);
    // Something worth scrolling, sideways and down: the wrapping and the sticky header are
    // both about a file that does not fit.
    let long: String = (0..200)
        .map(|i| {
            format!(
                "fn line_{i}() {{ let x = {i}; println!(\"a long line, long enough to run off \
                 the side of a telephone: {{}}\", x); }}\n"
            )
        })
        .collect();
    std::fs::write(repo.join("long.rs"), long).unwrap();
    // And something for the overview to have an opinion about: prose, an entry point by
    // convention, and one file whose names the others use.
    std::fs::write(
        repo.join("README.md"),
        "# scratch\n\nA repository the browser test looks at. It exists so that the overview \
         has a README to quote, an entry point to find, and a module for the rest of it to \
         lean on.\n\n## Saving a review\n\nA review is written to REVIEW.md as it is saved.\n",
    )
    .unwrap();
    // A test target for the page to find and run, and a test for it to list.
    std::fs::write(
        repo.join("Makefile"),
        "all:\n\techo hi\ntest:\n\t@echo 'test result: ok. 1 passed; 0 failed; 0 ignored'\n",
    )
    .unwrap();
    std::fs::create_dir_all(repo.join("src")).unwrap();
    // A picture, for the file view to show rather than call binary.
    std::fs::write(repo.join("logo.png"), include_bytes!("../src/web/icon.png")).unwrap();
    std::fs::write(
        repo.join("src/core.rs"),
        "pub fn parse_review() {}\npub fn write_review() {}\npub struct ReviewFile;\n\n\
         #[cfg(test)]\nmod tests {\n    #[test]\n    fn a_review_round_trips() {}\n}\n",
    )
    .unwrap();
    // And enough tests that a level of them starts folded, to open and to search.
    let many: String = (0..20)
        .map(|i| format!("#[test]\nfn checks_number_{i}() {{\n    assert!(true);\n}}\n\n"))
        .collect();
    std::fs::write(repo.join("src/many.rs"), many).unwrap();
    std::fs::write(
        repo.join("src/main.rs"),
        "fn main() { parse_review(); write_review(); let _r = ReviewFile; }\n",
    )
    .unwrap();

    // Two end-to-end tests, each with what its last run left: a terminal recording of one
    // that passed, and a video of one that failed. What a repository opens on.
    std::fs::create_dir_all(repo.join("tests")).unwrap();
    std::fs::write(
        repo.join("tests/cli.rs"),
        "// runs CARGO_BIN_EXE_scratch in a terminal\n\
         #[test]\nfn a_review_is_listed() {\n    assert!(true);\n}\n\n\
         #[test]\nfn a_review_is_saved() {\n    assert!(true);\n}\n",
    )
    .unwrap();
    let recorded = |slug: &str, json: serde_json::Value, name: &str, bytes: &[u8]| {
        let dir = repo.join("target/e2e/fixture").join(slug);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("test.json"), json.to_string()).unwrap();
        std::fs::write(dir.join(name), bytes).unwrap();
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    recorded(
        "listed",
        serde_json::json!({
            "test": "a_review_is_listed", "file": "tests/cli.rs", "line": 2, "passed": true,
            "recording": "terminal.cast", "duration_ms": 1500,
            "steps": [
                { "at_ms": 100, "line": 3, "kind": "action", "text": "start it" },
                { "at_ms": 1200, "line": 3, "kind": "check", "text": "the review is listed",
                  "passed": true },
            ],
        }),
        "terminal.cast",
        format!(
            "{{\"version\":2,\"width\":40,\"height\":4,\"timestamp\":{now}}}\n\
             [0.1,\"o\",\"\\u001b[1mreviews\\u001b[0m\"]\n\
             [1.2,\"o\",\"\\r\\n\\u001b[32ma.rs: why is this pub?\\u001b[0m\"]\n"
        )
        .as_bytes(),
    );
    recorded(
        "saved",
        serde_json::json!({
            "test": "a_review_is_saved", "file": "tests/cli.rs", "line": 7, "passed": false,
            "recording": "video.webm", "duration_ms": 2300,
            "docs": ["README.md#Saving a review"],
            "steps": [
                { "at_ms": 0, "line": 9, "kind": "action", "text": "save the review" },
                { "at_ms": 1500, "line": 9, "kind": "check", "text": "REVIEW.md has it",
                  "passed": false, "detail": "REVIEW.md was empty" },
            ],
        }),
        "video.webm",
        include_bytes!("browser/fixture.webm"),
    );
    // What that run executed: two lines of src/core.rs, which a unit test lives beside.
    std::fs::write(
        repo.join("target/e2e/fixture/saved/coverage.lcov"),
        "SF:src/core.rs\nDA:1,1\nDA:2,1\nDA:3,0\nend_of_record\n",
    )
    .unwrap();
    // And a repository with no end-to-end test at all, which opens on saying so.
    let bare = work.path().join("bare");
    scratch_repo(&bare);
    std::fs::write(bare.join("README.md"), "# bare\n\nNothing drives this.\n").unwrap();
    std::fs::write(
        bare.join("lib.rs"),
        "#[test]\nfn one_and_one_make_two() {\n    assert_eq!(1 + 1, 2);\n}\n",
    )
    .unwrap();

    let profiles = counting().then(|| tempfile::tempdir().unwrap());
    let server = Command::new(env!("CARGO_BIN_EXE_codereview"))
        .args([
            "web",
            "--host",
            "127.0.0.1",
            "--port",
            &PORT.to_string(),
            "--no-open",
            repo.to_str().unwrap(),
            bare.to_str().unwrap(),
        ])
        .env("CODEREVIEW_TOKEN", TOKEN)
        .envs(
            profiles
                .as_ref()
                .map(|p| ("CODEREVIEW_COVERAGE_DIR", p.path().to_path_buf())),
        )
        .env("CODEREVIEW_CONFIG", work.path().join("config.toml"))
        .env_remove("CODEREVIEW_PASSWORD_HASH")
        .stdout(Stdio::null())
        .spawn()
        .expect("codereview web");
    let _serving = Serving(server);
    wait_for_port(PORT, "codereview");

    // `--network host` so that the page's own address works inside the container: the server
    // is on the host's loopback, and a container with a network of its own cannot reach it.
    let url = format!("http://127.0.0.1:{PORT}/?t={TOKEN}");
    let container = Container(format!("codereview-browser-test-{}", std::process::id()));
    let out = Command::new(runtime())
        .args([
            "run",
            "--name",
            &container.0,
            "--network",
            "host",
            "--entrypoint",
            "node",
            &image,
            "-e",
            PHONE,
            &url,
            &under_test(),
        ])
        .output()
        .expect("failed to run the browser");
    let report =
        String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);

    // Copied out whether or not the tests passed: the recording of a failure is the one most
    // worth watching. The whole directory is replaced, so a test that is gone takes its
    // recording with it.
    let recordings = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target/e2e/browser");
    let _ = std::fs::remove_dir_all(&recordings);
    std::fs::create_dir_all(recordings.parent().unwrap()).unwrap();
    let copied = Command::new(runtime())
        .args(["cp", &format!("{}:/tmp/e2e", container.0)])
        .arg(&recordings)
        .output()
        .expect("failed to copy the recordings out");

    gather_coverage(
        &recordings,
        profiles.as_ref().map_or(work.path(), |p| p.path()),
    );

    assert!(out.status.success(), "{report}");
    // A script that fell over before it ran a test would otherwise pass quietly.
    assert!(
        report.contains("tests passed") && !report.contains("FAIL"),
        "{report}"
    );
    assert!(
        copied.status.success(),
        "the recordings stayed in the container: {}",
        String::from_utf8_lossy(&copied.stderr)
    );
    println!("{report}");
    println!("recordings in {}", recordings.display());
}
