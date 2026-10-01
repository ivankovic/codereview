//! How long the page's requests take, end to end through the server, against this repository
//! itself: every request the page makes, each timed cold and then warm, against a budget a
//! reader never notices. Ignored by default and run in release by `make perf`, since a debug
//! build measures the compiler rather than the server; run in debug it prints its table and
//! enforces nothing.
//!
//! The repository is a clone of this one in a temporary directory, with one file edited so
//! that the working tree has a change to show, so the run neither depends on the state of the
//! checkout it is run from nor writes into it.

use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant};

use super::tests::{request, serve};

/// What any one request may take, in milliseconds. The page waits for most of them before it
/// draws, so this is the pause between a tap and the screen changing.
const BUDGET_MS: u128 = 400;

/// A warm timing is the best of this many requests after the first; the first is the cold
/// one. Best rather than mean, because the machine running this is doing other things too.
const WARM_RUNS: usize = 3;

/// A clone of this repository, with the largest source file edited so that the working tree
/// has a change, and a user configured so that a commit can be made.
fn clone_of_this_repository() -> (tempfile::TempDir, String) {
    let here = env!("CARGO_MANIFEST_DIR");
    let dir = tempfile::tempdir().unwrap();
    let git = |args: &[&str]| {
        let out = Command::new("git")
            .arg("-C")
            .arg(dir.path())
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        out.stdout
    };
    let out = Command::new("git")
        .args(["clone", "--quiet", "--local", here])
        .arg(dir.path())
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "cloning: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    git(&["config", "user.email", "perf@example.com"]);
    git(&["config", "user.name", "Perf"]);
    let files = git(&["ls-files", "-z"]);
    let biggest = String::from_utf8_lossy(&files)
        .split('\0')
        .filter(|f| f.ends_with(".rs"))
        .map(|f| {
            let bytes = std::fs::metadata(dir.path().join(f)).map_or(0, |m| m.len());
            (bytes, f.to_string())
        })
        .max()
        .map(|(_, f)| f)
        .expect("a Rust file to open");
    let full: PathBuf = dir.path().join(&biggest);
    let mut text = std::fs::read_to_string(&full).unwrap();
    text.push_str("\n// a line for the diff to show\n");
    std::fs::write(&full, text).unwrap();
    (dir, biggest)
}

struct Op {
    name: String,
    method: &'static str,
    path: String,
    body: Option<String>,
    /// Timed once, because it changes the repository: a comment, a commit.
    once: bool,
}

fn get(name: &str, path: String) -> Op {
    Op {
        name: name.to_string(),
        method: "GET",
        path,
        body: None,
        once: false,
    }
}

fn post(name: &str, path: &str, body: Option<&str>, once: bool) -> Op {
    Op {
        name: name.to_string(),
        method: "POST",
        path: path.to_string(),
        body: body.map(str::to_string),
        once,
    }
}

/// Every request the page makes, in roughly the order it makes them: opening the repository,
/// reading a file and what is known about it, the views behind the tabs, the symbol index,
/// then the writes.
fn operations(file: &str) -> Vec<Op> {
    let f = |p: &str| p.replace("{file}", file);
    vec![
        get("repositories", "/api/repos".into()),
        get("state", "/api/state".into()),
        get("comments", "/api/comments".into()),
        get("overview", "/api/overview".into()),
        get("leans on (builds the index)", "/api/leans_on".into()),
        get("tests", "/api/tests".into()),
        get("open the largest file", f("/api/file?path={file}")),
        get("blame", f("/api/blame?path={file}")),
        get("version poll", f("/api/version?path={file}")),
        get("symbols of the file", f("/api/symbols?path={file}")),
        get(
            "definitions of a name",
            "/api/definitions?name=Session".into(),
        ),
        get(
            "references to a name",
            "/api/references?name=Session".into(),
        ),
        get("symbol search", "/api/symbol_search?q=sess".into()),
        get(
            "identifier under the cursor",
            f("/api/identifier?path={file}&line=0&col=4"),
        ),
        get("working tree changes", "/api/changes".into()),
        get(
            "diff of the changed file",
            f("/api/diff?kind=working&path={file}&status=M"),
        ),
        get("log", "/api/log".into()),
        get("log of the file", f("/api/log?path={file}")),
        get("files of a commit", "/api/commit_files?hash=HEAD".into()),
        get("notes", "/api/notes".into()),
        get("checks", "/api/analysis".into()),
        get("themes", "/api/themes".into()),
        post("refresh", "/api/refresh", None, false),
        post(
            "add a comment",
            "/api/comments",
            Some(&format!(r#"{{"path":"{file}","line":1,"text":"timed"}}"#)),
            true,
        ),
        post("reanchor", "/api/reanchor", None, false),
        post(
            "commit everything",
            "/api/commit",
            Some(r#"{"message":"perf: a commit to time","all":true}"#),
            true,
        ),
        get("overview after the commit", "/api/overview".into()),
    ]
}

/// One run of an operation: how long it took and whether it succeeded.
fn timed(addr: std::net::SocketAddr, token: &str, op: &Op) -> (Duration, u16, String) {
    let start = Instant::now();
    let (status, body) = request(addr, op.method, &op.path, Some(token), op.body.as_deref());
    (start.elapsed(), status, body)
}

#[test]
#[ignore = "timing in release; run with `make perf`"]
fn every_request_of_the_page_is_within_budget() {
    let (dir, file) = clone_of_this_repository();
    let (addr, token, _rt) = serve(&[dir.path()]);
    let enforced = !cfg!(debug_assertions);
    println!();
    println!(
        "{:<32} {:>8} {:>8}   budget {BUDGET_MS} ms{}",
        "request",
        "cold",
        "warm",
        if enforced {
            ""
        } else {
            " (debug build: not enforced)"
        }
    );
    let mut over = Vec::new();
    for op in operations(&file) {
        let (cold, status, body) = timed(addr, &token, &op);
        assert_eq!(status, 200, "{}: {body}", op.name);
        let warm = if op.once {
            None
        } else {
            (0..WARM_RUNS)
                .map(|_| {
                    let (took, status, body) = timed(addr, &token, &op);
                    assert_eq!(status, 200, "{}: {body}", op.name);
                    took
                })
                .min()
        };
        let ms = |d: Duration| d.as_millis();
        let mark = |d: Duration| if ms(d) > BUDGET_MS { " over" } else { "" };
        println!(
            "{:<32} {:>5} ms {:>8}{}{}",
            op.name,
            ms(cold),
            warm.map_or("-".to_string(), |w| format!("{} ms", ms(w))),
            mark(cold),
            warm.map_or("", mark)
        );
        if ms(cold) > BUDGET_MS {
            over.push(format!("{} cold: {} ms", op.name, ms(cold)));
        }
        if let Some(w) = warm.filter(|w| ms(*w) > BUDGET_MS) {
            over.push(format!("{} warm: {} ms", op.name, ms(w)));
        }
    }
    println!();
    if enforced {
        assert!(
            over.is_empty(),
            "over the {BUDGET_MS} ms budget:\n  {}",
            over.join("\n  ")
        );
    }
    drop(dir);
}
