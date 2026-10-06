//! The terminal UI, end to end: the real binary in a real pseudo-terminal, driven by keys the
//! way a person drives it, and checked by what a terminal would show.
//!
//! These are the terminal half of the end-to-end tests the tool is built around. A reviewer
//! does not start in the code; they start with what the software demonstrably does, and these
//! tests are where that is written down and checked. So each one is a single thing a person
//! would do, named as a sentence, and each leaves a recording behind — under
//! `target/e2e/terminal/`, an asciicast of the run and a `test.json` of its steps — that shows
//! it being done. `harness.rs` has the details.
//!
//! The in-process tests in `src/tui.rs` drive the same screens faster and see more of the
//! state; what only these see is the program as it is shipped: its start-up, raw mode, the
//! alternate screen, the way keys arrive as bytes, and whether it gives the terminal back.

mod common;
#[macro_use]
#[path = "terminal/harness.rs"]
mod harness;

use std::path::Path;

use harness::{DOWN, ENTER};

#[test]
fn a_repository_opens_on_its_end_to_end_tests_and_plays_a_recording() {
    let mut t = term!(|root: &Path| {
        // What the last run of tests/e2e.rs left: a recording of two screens, and its steps.
        let dir = root.join("target/e2e/terminal/a-review-survives-a-restart");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("test.json"),
            r#"{"test": "a_review_survives_a_restart", "file": "tests/e2e.rs", "line": 2,
                "passed": true, "duration_ms": 1500, "recording": "terminal.cast",
                "steps": [
                  {"at_ms": 0, "line": 3, "kind": "action", "text": "write a review"},
                  {"at_ms": 900, "line": 3, "kind": "check", "text": "it is still there",
                   "passed": true}]}"#,
        )
        .unwrap();
        std::fs::write(
            dir.join("terminal.cast"),
            "{\"version\": 2, \"width\": 40, \"height\": 5}\n\
             [0.0, \"o\", \"a review, before\"]\n\
             [0.9, \"o\", \"\\u001b[2J\\u001b[Hthe review, after the restart\"]\n",
        )
        .unwrap();
    });
    t.documents("SPECS.md#What a repository opens on");
    t.wait_for("1 end-to-end test of 2 tests");
    t.wait_for("✓ a review survives a restart");
    t.wait_for("tests/e2e.rs:2 · its path says e2e");
    t.wait_for("a review, before");
    t.wait_for("0:00.0 / 0:01.5 paused");
    t.step("play the recording");
    t.keys(" ");
    t.wait_for("the review, after the restart");
    t.step("open the test it is a recording of");
    t.keys("s");
    t.wait_for("tests/e2e.rs 2/4");
    t.wait_for("fn a_review_survives_a_restart()");
}

#[test]
fn a_test_leads_to_the_code_it_ran_and_what_documents_it() {
    let mut t = term!(|root: &Path| {
        // What the last run of tests/e2e.rs left: its recording, its steps, and the lines it
        // ran; and a section of the README that says what it shows.
        let dir = root.join("target/e2e/terminal/a-review-survives-a-restart");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("test.json"),
            r#"{"test": "a_review_survives_a_restart", "file": "tests/e2e.rs", "line": 2,
                "passed": true, "duration_ms": 1000, "recording": "terminal.cast",
                "steps": [
                  {"at_ms": 0, "line": 3, "kind": "action", "text": "write a review"},
                  {"at_ms": 600, "line": 3, "kind": "check", "text": "it survives a restart",
                   "passed": true}]}"#,
        )
        .unwrap();
        std::fs::write(
            dir.join("terminal.cast"),
            "{\"version\": 2, \"width\": 40, \"height\": 4}\n[0.0, \"o\", \"writing\"]\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("coverage.lcov"),
            "SF:src/review.rs\nDA:1,2\nDA:3,1\nend_of_record\n",
        )
        .unwrap();
        let readme = root.join("README.md");
        let mut text = std::fs::read_to_string(&readme).unwrap();
        text.push_str("\n## Surviving a restart\n\nA review written survives a restart.\n");
        std::fs::write(readme, text).unwrap();
    });
    t.documents("SPECS.md#Terminal UI");
    t.wait_for("✓ a review survives a restart");
    t.step("open the test on its own");
    t.keys(ENTER);
    t.wait_for("Code it ran");
    t.wait_for("3 │ assert!(true);");
    t.wait_for("src/review.rs  2 lines, 2 only this test");
    t.wait_for("Surviving a restart — README.md:");
    t.wait_for("a guess: shares");
    t.wait_for("a review round trips · unit · src/review.rs");
    t.step("open the code it ran");
    t.keys("j");
    t.keys("j");
    t.keys(ENTER);
    t.wait_for("src/review.rs 1/");
    t.wait_for("1 ▌pub fn parse_review");
    t.wait_for("ran by “a review survives a restart”");
}

#[test]
fn a_repository_without_end_to_end_tests_says_so_first() {
    let mut t = term!(|root: &Path| {
        std::fs::remove_file(root.join("tests/e2e.rs")).unwrap();
    });
    t.documents("SPECS.md#What a repository opens on");
    t.wait_for("No end-to-end tests.");
    t.wait_for("The repository has 1 test of other kinds: P lists them.");
    t.wait_for("ask the agent to write the first ones");
}

#[test]
fn a_repository_opens_on_what_it_holds_and_where_to_go() {
    let mut t = term!();
    t.documents("SPECS.md#Terminal UI");
    t.leave_the_tests();
    t.wait_for("5 files, 0 pending comments, 0 notes");
    t.wait_for("L  repository log");
    t.wait_for("S  working tree changes");
    t.wait_for("README.md");
}

#[test]
fn a_comment_left_on_a_line_is_written_to_review_md() {
    let mut t = term!();
    t.documents("SPECS.md#REVIEW.md");
    t.leave_the_tests();
    t.step("open src/main.rs from the tree");
    t.keys(DOWN);
    t.keys(ENTER);
    t.wait_for("src/main.rs 1/7");
    t.step("move to line 2 and start a comment");
    t.keys("j");
    t.keys("c");
    t.wait_for("comment on src/main.rs:2:");
    t.step("write the comment and save it");
    t.keys("needs a doc comment");
    t.keys(ENTER);
    t.wait_for("src/main.rs 2/7 ●1");
    let review = std::fs::read_to_string(t.repo().join("REVIEW.md")).unwrap_or_default();
    t.check(
        "REVIEW.md has the comment on line 2",
        review.contains("In src/main.rs on line 2: needs a doc comment"),
        || review.clone(),
    );
}

#[test]
fn a_search_goes_to_the_line_that_matches() {
    let mut t = term!();
    t.documents("README.md#Terminal UI");
    t.leave_the_tests();
    t.step("open src/main.rs from the tree");
    t.keys(DOWN);
    t.keys(ENTER);
    t.wait_for("src/main.rs 1/7");
    t.step("search for “write”");
    t.keys("/");
    t.wait_for("search:");
    t.keys("write");
    t.keys(ENTER);
    t.wait_for("src/main.rs 5/7");
    t.wait_for("/write (1)");
}

#[test]
fn the_log_lists_the_commits_and_opens_one_as_a_diff() {
    let mut t = term!();
    t.documents("SPECS.md#Diffs");
    t.leave_the_tests();
    t.step("open the repository log");
    t.keys("L");
    t.wait_for("log main 2 commits");
    t.wait_for("write the review back");
    t.wait_for("the first version");
    t.step("open the newest commit's file, then its diff");
    t.keys(ENTER);
    t.keys(ENTER);
    t.wait_for("M src/main.rs");
    t.wait_for("-0 +1 hunk 1/1");
    t.wait_for("review::write_review();");
    t.step("go back to the log");
    t.keys("q");
    t.wait_for("log main 2 commits");
}

#[test]
fn uncommitted_changes_are_listed_and_open_as_a_diff() {
    let mut t = term!();
    t.documents("SPECS.md#Diffs");
    t.leave_the_tests();
    t.step("open the working tree changes");
    t.keys("S");
    t.wait_for("working tree 2 files");
    t.wait_for("? notes.txt");
    t.wait_for("M src/main.rs");
    t.step("open the changed file's diff");
    t.keys(DOWN);
    t.keys(ENTER);
    t.wait_for("src/main.rs working tree");
    t.wait_for("-0 +1 hunk 1/1");
    t.wait_for("println!(\"done\");");
}

#[test]
fn the_tests_read_as_a_specification_by_level() {
    let mut t = term!();
    t.documents("SPECS.md#What a repository opens on");
    t.leave_the_tests();
    t.step("open the specification");
    t.keys("P");
    t.wait_for("specification 2 tests");
    t.wait_for("End to end 1");
    t.wait_for("a review survives a restart");
    t.wait_for("Unit 1");
    t.wait_for("a review round trips");
}

#[test]
fn q_steps_back_and_then_gives_the_terminal_back() {
    let mut t = term!();
    t.documents("SPECS.md#Terminal UI");
    t.leave_the_tests();
    t.step("open the log, then step back out of it");
    t.keys("L");
    t.wait_for("log main 2 commits");
    t.keys("q");
    t.wait_for("files 5");
    t.step("leave");
    t.keys("q");
    t.exits();
    let alternate = t.alternate_screen();
    t.check("the terminal is back on its own screen", !alternate, || {
        "still on the alternate screen".into()
    });
}
