//! The page in a real browser, at a real phone's size.
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
//! The script is handed to node as an argument rather than mounted, because a bind mount into
//! a rootless container has to survive the user namespace mapping the file's owner, and an
//! argument has nothing to survive.

mod common;

use common::{have, port_is_free, run, runtime, scratch_repo, wait_for_port};
use std::process::{Child, Command, Stdio};

/// Well above the privileged range: the container shares the host's network, so this is a host
/// port and a rootless runtime may not bind the low ones.
const PORT: u16 = 18766;
const TOKEN: &str = "browser-test-token-long-enough";
/// Chromium and puppeteer in one image, so that neither a browser nor a node toolchain has to
/// exist on the machine running the tests. Spelled out in full: podman resolves a short name
/// only against the registries a machine happens to have configured.
const IMAGE: &str = "docker.io/zenika/alpine-chrome:with-puppeteer";

/// The page's own checks, run inside the container against the server below.
const PHONE: &str = include_str!("browser/phone.js");

struct Serving(Child);

impl Drop for Serving {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
#[ignore = "needs podman (or docker); run with `make browser-test`"]
fn the_phone_layout_works_in_a_real_browser() {
    for (program, probe) in [(runtime(), "--version"), ("git", "--version")] {
        assert!(have(program, probe), "{program} is needed for this test");
    }
    assert!(port_is_free(PORT), "port {PORT} is already in use");
    // Pulled here rather than by the run below, which would spend a silent minute on a
    // machine that has never run this before and then look like a hung test.
    if run(runtime(), &["images", "-q", IMAGE]).trim().is_empty() {
        eprintln!("pulling {IMAGE} — about a gigabyte, once");
        run(runtime(), &["pull", IMAGE]);
    }

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
         lean on.\n",
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
    std::fs::write(
        repo.join("src/main.rs"),
        "fn main() { parse_review(); write_review(); let _r = ReviewFile; }\n",
    )
    .unwrap();

    let server = Command::new(env!("CARGO_BIN_EXE_codereview"))
        .args([
            "web",
            "--host",
            "127.0.0.1",
            "--port",
            &PORT.to_string(),
            "--no-open",
            repo.to_str().unwrap(),
        ])
        .env("CODEREVIEW_TOKEN", TOKEN)
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
    let out = Command::new(runtime())
        .args([
            "run",
            "--rm",
            "--network",
            "host",
            "--entrypoint",
            "node",
            IMAGE,
            "-e",
            PHONE,
            &url,
        ])
        .output()
        .expect("failed to run the browser");
    let report =
        String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{report}");
    // A script that fell over before it asserted anything would otherwise pass quietly.
    assert!(
        report.contains("checks passed") && !report.contains("FAIL"),
        "{report}"
    );
    println!("{report}");
}
