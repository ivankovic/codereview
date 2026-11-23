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

use harness::{DOWN, ENTER, ESC};

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
    t.wait_for("In the last 7 days");
    t.step("select the test, under the overview");
    t.keys(DOWN);
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
fn the_tests_open_on_the_overview_with_each_test_in_the_side_panel() {
    let mut t = term!();
    t.documents("SPECS.md#Terminal UI");
    t.wait_for("○ a review survives a restart");
    let screen = t.screen();
    let first = screen.lines().nth(2).unwrap_or_default().to_string();
    t.check(
        "the side panel starts with the overview, and beside it is what the stretch did",
        first.starts_with("│Overview") && first.contains("In the last 7 days"),
        || screen.clone(),
    );
}

#[test]
fn q_from_a_test_beside_the_list_gives_the_list_the_keys_again() {
    let mut t = term!();
    t.documents("SPECS.md#Terminal UI");
    t.wait_for("○ a review survives a restart");
    t.step("select the test and go into it");
    t.keys(DOWN);
    t.keys(ENTER);
    t.wait_for("q/Tab the list");
    t.step("come back to the list");
    t.keys("q");
    t.wait_for("Enter/Tab beside");
    let still_there = t.alternate_screen();
    t.check("q did not quit", still_there, || {
        "the program left the screen".into()
    });
}

#[test]
fn backslash_hides_the_list_beside_the_tests() {
    let mut t = term!();
    t.documents("SPECS.md#Terminal UI");
    t.wait_for("○ a review survives a restart");
    t.step("hide the side panel");
    t.keys("\\");
    t.wait_until_gone("○ a review survives a restart");
    t.wait_for("In the last 7 days");
    t.step("show it again");
    t.keys("\\");
    t.wait_for("○ a review survives a restart");
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
    t.step("select the test, and give what is beside the list the keys");
    t.keys(DOWN);
    t.keys(ENTER);
    t.wait_for("Code it ran");
    t.wait_for("3 │ assert!(true);");
    t.wait_for("src/review.rs  2 lines, 2 only this test");
    t.wait_for("Surviving a restart — README.md:");
    t.wait_for("a guess: shares");
    t.wait_for("a review round trips · api · src/review.rs");
    t.step("open the code it ran");
    t.keys("j");
    t.keys("j");
    t.keys(ENTER);
    t.wait_for("src/review.rs 1/");
    t.wait_for("1 ▌pub fn parse_review");
    t.wait_for("ran by “a review survives a restart”");
}

#[test]
fn changed_code_no_end_to_end_test_ran_is_marked() {
    let mut t = term!(|root: &Path| {
        // The last run of tests/e2e.rs, with what it ran of src/main.rs: everything but the
        // line the uncommitted edit added.
        let dir = root.join("target/e2e/terminal/a-review-survives-a-restart");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("test.json"),
            r#"{"test": "a_review_survives_a_restart", "file": "tests/e2e.rs", "line": 2,
                "passed": true, "duration_ms": 1000, "steps": []}"#,
        )
        .unwrap();
        std::fs::write(
            dir.join("coverage.lcov"),
            "SF:src/main.rs\nDA:3,1\nDA:4,1\nDA:5,1\nDA:6,0\nDA:7,1\nend_of_record\n",
        )
        .unwrap();
    });
    t.documents("SPECS.md#What a repository opens on");
    t.wait_for("✓ a review survives a restart");
    t.step("mark the uncommitted work instead of the last 7 days");
    t.keys("w");
    t.wait_for("marking the last commit");
    t.keys("w");
    t.wait_for("In uncommitted work");
    t.wait_for("No end-to-end test ran 1 of 1 changed lines");
    t.wait_for("▌src/main.rs  1 of 1");
    t.wait_for(": no command records the tests");
    t.step("go into the overview, which starts on the changed code nothing ran");
    t.keys(ENTER);
    t.wait_for("ran in no end-to-end");
    t.step("move to the base the tests could be recorded at, and back");
    t.keys("j");
    t.wait_for("There is no command");
    t.keys("k");
    t.wait_for("ran in no end-to-end");
    t.keys(ENTER);
    t.wait_for("src/main.rs 6/7");
    t.wait_for("6 ▌    println!");
    t.wait_for("changed in uncommitted work, run by no end-to-end test");
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
fn the_focuses_are_picked_at_the_top_right_with_the_tests_first() {
    let mut t = term!();
    t.documents("SPECS.md#Terminal UI");
    t.on_the_tests();
    let screen = t.screen();
    let top = screen.lines().next().unwrap_or_default().to_string();
    let order = ["1 Tests", "2 Code", "3 Docs", "4 Data"].map(|name| top.find(name));
    t.check(
        "the focus picker is Tests, Code, Docs, Data, in that order",
        order.iter().all(Option::is_some) && order.is_sorted(),
        || top.clone(),
    );
    t.check(
        "the tests are the first screen, with nothing under them",
        screen
            .lines()
            .last()
            .is_some_and(|l| l.starts_with(" end to end ")),
        || screen.clone(),
    );
}

#[test]
fn the_breadcrumb_says_where_in_the_repository_one_is() {
    let mut t = term!();
    t.documents("SPECS.md#Terminal UI");
    t.on_the_tests();
    let top = t.screen().lines().next().unwrap_or_default().to_string();
    let root = t.repo().display().to_string();
    let name = root.rsplit('/').next().unwrap_or_default().to_string();
    t.check(
        "the breadcrumb names the checkout, the branch, the range and the commit",
        top.contains(&name)
            && top.contains("› main ›")
            && top.contains("› last 7 days ›")
            && top.split(" › ").count() == 4,
        || top.clone(),
    );
}

#[test]
fn a_remote_s_address_is_shown_without_its_scheme() {
    let mut t = term!(|root: &Path| {
        let out = std::process::Command::new("git")
            .args([
                "remote",
                "add",
                "origin",
                "git@github.com:someone/scratch.git",
            ])
            .current_dir(root)
            .output()
            .unwrap();
        assert!(out.status.success());
    });
    t.documents("SPECS.md#Terminal UI");
    t.on_the_tests();
    t.wait_for("github.com/someone/scratch › main");
}

#[test]
fn a_focus_left_keeps_its_place() {
    let mut t = term!();
    t.documents("SPECS.md#Terminal UI");
    t.go_to_the_code();
    t.step("open src/main.rs");
    t.keys(DOWN);
    t.keys(ENTER);
    t.wait_for("src/main.rs 1/7");
    t.step("go to the documents, then back to the code");
    t.keys("3");
    t.wait_for("files 1");
    t.wait_until_gone("src/main.rs 1/7");
    t.keys("2");
    t.wait_for("src/main.rs 1/7");
}

#[test]
fn r_opens_the_review_over_the_code() {
    let mut t = term!();
    t.documents("SPECS.md#Terminal UI");
    t.on_the_tests();
    t.step("open the review from the tests");
    t.keys("R");
    t.wait_for(" review 0 pending, 0 completed");
    t.step("step back to the code under it");
    t.keys("q");
    t.wait_for("files 5");
}

/// A branch in the scratch repository: `feature`, one commit past `main`, changing the README.
fn feature_branch(root: &Path) -> (String, String) {
    let git = |args: &[&str]| {
        let out = std::process::Command::new("git")
            .args(args)
            .current_dir(root)
            .output()
            .unwrap();
        assert!(out.status.success(), "{args:?}: {out:?}");
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    };
    git(&["switch", "-q", "-c", "feature"]);
    let readme = root.join("README.md");
    let mut text = std::fs::read_to_string(&readme).unwrap();
    text.push_str("\nA line the feature adds.\n");
    std::fs::write(&readme, text).unwrap();
    git(&["commit", "-q", "-m", "the feature", "--", "README.md"]);
    git(&["switch", "-q", "main"]);
    (git(&["rev-parse", "main"]), git(&["rev-parse", "feature"]))
}

#[test]
fn a_stretch_picked_from_the_range_picker_is_what_the_tests_mark() {
    let mut t = term!();
    t.documents("SPECS.md#Terminal UI");
    t.on_the_tests();
    t.step("open the range picker with gp, which o also opens, and pick the uncommitted work");
    t.keys("g");
    t.keys("p");
    t.wait_for("review what?");
    t.keys("j");
    t.keys("j");
    t.keys(ENTER);
    t.wait_for("marking uncommitted work (w)");
    let top = t.screen().lines().next().unwrap_or_default().to_string();
    t.check(
        "the breadcrumb names the stretch",
        top.contains("› uncommitted ›"),
        || top.clone(),
    );
}

#[test]
fn a_worktree_s_uncommitted_work_is_reviewed_against_its_last_commit() {
    let mut t = term!(|root: &Path| {
        // An agent's worktree beside the checkout, with work it has not committed.
        let agent = root.with_file_name(format!(
            "{}-agent",
            root.file_name().unwrap().to_string_lossy()
        ));
        let out = std::process::Command::new("git")
            .args(["worktree", "add", "-q", "-b", "agent-work"])
            .arg(&agent)
            .current_dir(root)
            .output()
            .unwrap();
        assert!(out.status.success(), "{out:?}");
        std::fs::write(agent.join("src/review.rs"), "pub fn parse_review() {}\n").unwrap();
        std::fs::write(agent.join("NEW.txt"), "the agent wrote this\n").unwrap();
    });
    t.documents("SPECS.md#Pull requests and comparisons");
    t.on_the_tests();
    t.step("open the range picker, and find the agent's worktree");
    t.keys("o");
    t.wait_for("Worktrees, as they are on disk");
    t.keys("/");
    t.keys("-agent");
    t.keys(ENTER);
    t.wait_for("agent-work");
    t.step("pick it, and compare it with its own last commit, offered first");
    t.keys(ENTER);
    t.wait_for("its last commit, agent-work — its uncommitted work alone");
    t.keys(ENTER);
    t.wait_for("In worktree");
    t.step("go to its code");
    t.keys("2");
    t.wait_for("-agent, uncommitted against");
    t.wait_for("Files it changes 2");
    t.wait_for("NEW.txt");
    t.step("open the new file's diff");
    t.keys(ENTER);
    t.wait_for("the agent wrote this");
}

#[test]
fn a_branch_is_compared_as_a_pull_request_into_the_current_one() {
    let mut t = term!(|root: &Path| {
        feature_branch(root);
    });
    t.documents("SPECS.md#Pull requests and comparisons");
    t.on_the_tests();
    t.step("open the range picker, and find the feature branch");
    t.keys("o");
    t.wait_for("review what?");
    t.keys("/");
    t.keys("feature");
    t.keys(ENTER);
    t.wait_for("  feature");
    t.step("pick it, and compare it with the default branch, offered first");
    t.keys(ENTER);
    t.wait_for("compare feature with what?");
    t.wait_for("main — the default branch, where pull requests go");
    t.keys(ENTER);
    t.step("it opens on what it did to the tests");
    t.wait_for("In feature against main");
    t.wait_for("2 its code, 3 its documents, 4 its data");
    t.step("its documents: the README it changed");
    t.keys("3");
    t.wait_for("Documents it changes 1");
    t.step("its code");
    t.keys("2");
    t.wait_for(" comparison feature against main ");
    t.wait_for("Files it changes 1");
    let top = t.screen().lines().next().unwrap_or_default().to_string();
    t.check(
        "the breadcrumb names the range",
        top.contains("› feature against main ›"),
        || top.clone(),
    );
    t.step("open the diff of the file it changes");
    t.keys(ENTER);
    t.wait_for("A line the feature adds.");
}

#[test]
fn a_pull_request_from_the_forge_opens_with_its_files_and_comments() {
    let mut t = term!(|root: &Path| {
        let (base, head) = feature_branch(root);
        let out = std::process::Command::new("git")
            .args([
                "remote",
                "add",
                "origin",
                "git@github.com:someone/scratch.git",
            ])
            .current_dir(root)
            .output()
            .unwrap();
        assert!(out.status.success());
        let git = root.join(".git");
        let pull = format!(
            r#"{{"number": 7, "title": "Say what the feature does", "user": {{"login": "ana"}},
                "state": "open", "draft": false, "merged_at": null,
                "base": {{"ref": "main", "sha": "{base}"}},
                "head": {{"ref": "feature", "sha": "{head}"}},
                "html_url": "https://github.com/someone/scratch/pull/7",
                "updated_at": "2026-11-20T10:00:00Z"}}"#
        );
        std::fs::write(git.join("pulls.json"), format!("[[{pull}]]")).unwrap();
        std::fs::write(
            git.join("on-lines.json"),
            r#"[[{"user": {"login": "bo"}, "body": "Why this line?", "path": "README.md",
                  "line": 5, "created_at": "2026-11-20T12:00:00Z"}]]"#,
        )
        .unwrap();
        std::fs::write(
            git.join("on-whole.json"),
            r#"[[{"user": {"login": "ana"}, "body": "Ready for a look.",
                  "created_at": "2026-11-20T11:00:00Z"}]]"#,
        )
        .unwrap();
        let gh = git.join("gh");
        std::fs::write(
            &gh,
            "#!/bin/sh\nhere=$(dirname \"$0\")\ncase \"$4\" in\n\
             */pulls/7/comments*) cat \"$here/on-lines.json\" ;;\n\
             */issues/7/comments*) cat \"$here/on-whole.json\" ;;\n\
             */pulls\\?*) cat \"$here/pulls.json\" ;;\n\
             *) echo \"no answer for $4\" >&2; exit 1 ;;\nesac\n",
        )
        .unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&gh, std::fs::Permissions::from_mode(0o755)).unwrap();
    });
    t.documents("SPECS.md#Pull requests and comparisons");
    t.on_the_tests();
    t.step("open the range picker, which asks the forge for its pull requests");
    t.keys("o");
    t.wait_for("#7 Say what the feature does — ana");
    t.step("pick the pull request, which is a base and a head already");
    t.keys("/");
    t.keys("#7");
    t.keys(ENTER);
    t.keys(ENTER);
    t.wait_for("In pull request #7");
    t.step("go to its code");
    t.keys("2");
    t.wait_for(" pull request #7 Say what the feature does ");
    t.wait_for("open · feature into main by ana");
    t.wait_for("Files it changes 1");
    t.wait_for("ana on the pull request: Ready for a look.");
    t.wait_for("bo on README.md:5: Why this line?");
    let top = t.screen().lines().next().unwrap_or_default().to_string();
    t.check(
        "the breadcrumb names the pull request",
        top.contains("› #7 ›"),
        || top.clone(),
    );
    t.step("open the line the comment is on, in the pull request's diff");
    t.keys("G");
    t.keys(ENTER);
    t.wait_for("A line the feature adds.");
    t.wait_for("-0 +2 hunk 1/1");
}

#[test]
fn a_number_goes_to_a_focus_and_q_comes_back_to_the_tests() {
    let mut t = term!();
    t.documents("SPECS.md#Terminal UI");
    t.on_the_tests();
    t.step("go to the code, the second focus");
    t.keys("2");
    t.wait_for("files 5");
    t.step("step back out of the code");
    t.keys("q");
    t.wait_for("1 end-to-end test of 2 tests");
}

#[test]
fn the_docs_focus_lists_only_the_documents() {
    let mut t = term!();
    t.documents("SPECS.md#Terminal UI");
    t.on_the_tests();
    t.step("go to the documents, the third focus");
    t.keys("3");
    t.wait_for("files 1");
    let screen = t.screen();
    t.check(
        "the tree holds README.md and no code",
        screen.contains("README.md") && !screen.contains("main.rs"),
        || screen.clone(),
    );
}

#[test]
fn the_data_focus_lists_only_the_datasets() {
    let mut t = term!(|root: &Path| {
        std::fs::create_dir_all(root.join("data")).unwrap();
        std::fs::write(root.join("data/scores.csv"), "name,score\nana,3\n").unwrap();
        std::fs::write(root.join("package.json"), "{}\n").unwrap();
    });
    t.documents("SPECS.md#Terminal UI");
    t.on_the_tests();
    t.step("go to the data, the fourth focus");
    t.keys("4");
    t.wait_for("scores.csv");
    let screen = t.screen();
    t.check(
        "the tree holds the CSV and not the JSON that configures a tool, nor the code",
        !screen.contains("package.json") && !screen.contains("main.rs"),
        || screen.clone(),
    );
    t.step("open the dataset");
    t.keys(DOWN);
    t.keys(ENTER);
    t.wait_for("ana,3");
}

#[test]
fn backslash_hides_the_side_panel_and_brings_it_back() {
    let mut t = term!();
    t.documents("SPECS.md#Terminal UI");
    t.go_to_the_code();
    t.step("open src/main.rs");
    t.keys(DOWN);
    t.keys(ENTER);
    t.wait_for("src/main.rs 1/7");
    t.step("hide the side panel");
    t.keys("\\");
    t.wait_until_gone("files 5");
    t.step("show it again");
    t.keys("\\");
    t.wait_for("files 5");
}

#[test]
fn a_file_named_on_the_command_line_opens_in_the_code_with_the_tests_a_q_away() {
    let mut t = term!(|_: &Path| {}, &["src/main.rs"]);
    t.documents("SPECS.md#Terminal UI");
    t.wait_for("src/main.rs 1/7");
    t.wait_for(" explore ");
    t.step("step back out of the file and the tree");
    t.keys("q");
    t.keys("q");
    t.wait_for("1 end-to-end test of 2 tests");
}

#[test]
fn a_pending_comment_is_counted_in_the_top_row() {
    let mut t = term!();
    t.documents("SPECS.md#Terminal UI");
    t.go_to_the_code();
    t.step("open src/main.rs and comment on its first line");
    t.keys(DOWN);
    t.keys(ENTER);
    t.wait_for("src/main.rs 1/7");
    t.keys("c");
    t.wait_for("comment on src/main.rs:1:");
    t.keys("why here?");
    t.keys(ENTER);
    t.wait_for("src/main.rs 1/7 ●1");
    let top = t.screen().lines().next().unwrap_or_default().to_string();
    t.check(
        "the top row counts one pending comment",
        top.contains("·1 "),
        || top.clone(),
    );
}

#[test]
fn a_test_s_source_opens_over_the_tests_and_q_goes_back_to_them() {
    let mut t = term!();
    t.documents("SPECS.md#Terminal UI");
    t.wait_for("○ a review survives a restart");
    t.step("select the test and open its source");
    t.keys(DOWN);
    t.keys("s");
    t.wait_for("tests/e2e.rs 2/4");
    t.wait_for(" end to end › explore ");
    t.step("step back out of the file and the tree");
    t.keys("q");
    t.keys("q");
    t.wait_for("○ a review survives a restart");
    let screen = t.screen();
    t.check(
        "the tests are on screen again, not the code",
        !screen.contains("files 5"),
        || screen.clone(),
    );
}

#[test]
fn the_code_says_what_the_repository_holds_and_where_to_go() {
    let mut t = term!();
    t.documents("SPECS.md#Terminal UI");
    t.go_to_the_code();
    t.wait_for("5 files, 0 pending comments, 0 notes");
    t.wait_for("L  repository log");
    t.wait_for("S  working tree changes");
    t.wait_for("README.md");
}

#[test]
fn a_comment_left_on_a_line_is_written_to_review_md() {
    let mut t = term!();
    t.documents("SPECS.md#REVIEW.md");
    t.go_to_the_code();
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
    t.go_to_the_code();
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
    t.on_the_tests();
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
    t.on_the_tests();
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
fn the_tests_read_as_a_specification_grouped_by_how_they_interact() {
    let mut t = term!();
    t.documents("SPECS.md#What kind of test each one is");
    t.on_the_tests();
    t.step("list every test");
    t.keys("P");
    t.wait_for("specification 2 tests");
    t.wait_for("How does the test interact with it?");
    t.wait_for("GUI/TUI 1");
    t.wait_for("a review survives a restart");
    t.wait_for("CLI 0");
    t.wait_for("API 1");
    t.wait_for("a review round trips");
}

#[test]
fn c_groups_the_specification_by_the_next_question() {
    let mut t = term!();
    t.documents("SPECS.md#What kind of test each one is");
    t.on_the_tests();
    t.step("list every test");
    t.keys("P");
    t.wait_for("How does the test interact with it?");
    t.step("group the tests by the next question");
    t.keys("c");
    t.wait_for("Is the test performing a numerical measurement?");
    t.wait_for("No measurement 2");
}

#[test]
fn k_answers_every_question_for_the_test_under_the_cursor() {
    let mut t = term!();
    t.documents("SPECS.md#What kind of test each one is");
    t.on_the_tests();
    t.step("list every test and find the unit test");
    t.keys("P");
    t.wait_for("API 1");
    t.keys("/");
    t.keys("round trips");
    t.keys(ENTER);
    t.wait_for("1 test matches");
    t.keys("G");
    t.step("ask what kind of test it is");
    t.keys("K");
    t.wait_for("what kind of test a review round trips is");
    t.wait_for("Code — it names code from src/review.rs");
    t.wait_for("API — it names code from src/review.rs");
    t.wait_for("Real dependencies");
}

#[test]
fn q_steps_back_and_then_gives_the_terminal_back() {
    let mut t = term!();
    t.documents("SPECS.md#Terminal UI");
    t.on_the_tests();
    t.step("open the log over the tests, then step back out of it");
    t.keys("L");
    t.wait_for("log main 2 commits");
    t.keys("q");
    t.wait_for("1 end-to-end test of 2 tests");
    t.step("leave");
    t.keys("q");
    t.exits();
    let alternate = t.alternate_screen();
    t.check("the terminal is back on its own screen", !alternate, || {
        "still on the alternate screen".into()
    });
}

/// Turns the scratch directory into a directory of checkouts: no repository of its own, and
/// two below it.
fn checkouts(root: &Path) {
    std::fs::remove_dir_all(root.join(".git")).unwrap();
    for name in ["alpha", "beta"] {
        common::scratch_repo(&root.join(name));
    }
}

#[test]
fn a_directory_of_checkouts_opens_on_a_list_of_its_repositories() {
    let mut t = term!(checkouts);
    t.documents("SPECS.md#Several repositories");
    t.wait_for("Select repository · 2");
    t.wait_for(" 1 alpha");
    t.wait_for(" 2 beta");
    t.step("open the second repository");
    t.keys("j");
    t.keys(ENTER);
    t.wait_for("repository 2: beta");
    t.wait_for("end to end");
    let screen = t.screen();
    t.check(
        "the list is gone",
        !screen.contains("Select repository"),
        || screen.clone(),
    );
}

#[test]
fn the_mouse_wheel_moves_through_the_list_of_repositories() {
    let mut t = term!(checkouts);
    t.documents("SPECS.md#Several repositories");
    t.wait_for("Select repository · 2");
    t.step("turn the wheel down a notch and open what it lands on");
    // A wheel notch down, as a terminal reports it once mouse reporting is on (SGR).
    t.keys("\x1b[<65;20;5M");
    t.keys(ENTER);
    t.wait_for("repository 2: beta");
}

#[test]
fn q_from_a_repository_goes_back_to_the_list_of_repositories() {
    let mut t = term!(checkouts);
    t.documents("SPECS.md#Several repositories");
    t.wait_for("Select repository · 2");
    t.step("open the first repository");
    t.keys(ENTER);
    t.wait_for("end to end");
    t.step("step back out of its tests");
    t.keys("q");
    t.wait_for("Select repository · 2");
    t.step("leave from the list");
    t.keys("q");
    t.exits();
}

/// What the last run of tests/e2e.rs left in `root`: its recording, and the lines of
/// src/main.rs it ran, made after the scratch repository's uncommitted change to that file.
fn recorded_with_coverage(root: &Path) {
    let dir = root.join("target/e2e/terminal/a-review-survives-a-restart");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("test.json"),
        r#"{"test": "a_review_survives_a_restart", "file": "tests/e2e.rs", "line": 2,
            "passed": true, "duration_ms": 900, "steps": [
              {"at_ms": 0, "line": 3, "kind": "action", "text": "write a review"}]}"#,
    )
    .unwrap();
    std::fs::write(
        dir.join("coverage.lcov"),
        "SF:src/main.rs\nDA:3,1\nDA:4,1\nDA:5,1\nDA:6,0\nend_of_record\n",
    )
    .unwrap();
}

#[test]
fn the_e2e_command_lists_the_tests_and_how_they_ran() {
    let mut t = term!(recorded_with_coverage, &["e2e"]);
    t.documents("SPECS.md#Command line");
    t.wait_for("✓ tests/e2e.rs:2 a review survives a restart");
    t.wait_for("1 end-to-end tests of 2, 0 failed, 0 without a recording");
    t.exits();
}

#[test]
fn the_e2e_command_says_what_uncommitted_work_left_untested() {
    let mut t = term!(recorded_with_coverage, &["e2e", "--stretch", "uncommitted"]);
    t.documents("SPECS.md#What a stretch of history did");
    t.wait_for("uncommitted work");
    t.wait_for("1 of 1 changed lines no end-to-end test ran");
    t.wait_for("src/main.rs — 1 of 1 lines");
    t.wait_for("not recorded at");
    t.exits();
}

/// A dataset of one drawn test, where `codereview kinds label` looks for one by default.
fn a_drawn_sample(root: &Path) {
    use codereview::kinds::{DEFAULT_DATASET, Dataset, Sample, Status, options};
    let dir = root.join(DEFAULT_DATASET);
    std::fs::create_dir_all(dir.join("someone-alpha-0123456-lib-parses")).unwrap();
    std::fs::write(
        dir.join("someone-alpha-0123456-lib-parses/file.rs.test"),
        "pub fn parse_review(s: &str) -> usize {\n    s.len()\n}\n\n#[test]\nfn parses() {\n    assert_eq!(parse_review(\"ab\"), 2);\n}\n",
    )
    .unwrap();
    let dataset = Dataset {
        dir,
        samples: vec![Sample {
            name: "someone-alpha-0123456-lib-parses".into(),
            repository: "someone/alpha".into(),
            commit: "0123456789abcdef0123456789abcdef01234567".into(),
            path: "src/lib.rs".into(),
            line: 6,
            end: 8,
            test: "parses".into(),
            label: "parses".into(),
            names: vec!["src/lib.rs".into()],
            drives: String::new(),
            files: vec![],
            status: Status::Pending,
            answers: vec![None; options().len()],
            notes: String::new(),
        }],
    };
    dataset.save().unwrap();
}

#[test]
fn a_sampled_test_is_classified_by_hand_and_every_answer_saved() {
    let mut t = term!(a_drawn_sample, &["kinds", "label"]);
    t.documents("SPECS.md#A sample classified by hand");
    t.wait_for("someone/alpha @ 0123456 · src/lib.rs:6–8 · pending");
    t.wait_for("▌fn parses() {");
    t.wait_for("What artifact is being tested?");
    t.step("say it tests code, and nothing else");
    t.keys("y");
    t.keys("N");
    t.wait_for("What artifact is being tested?  Code");
    let csv = std::fs::read_to_string(
        t.repo()
            .join(codereview::kinds::DEFAULT_DATASET)
            .join("kinds.csv"),
    )
    .unwrap();
    t.check(
        "kinds.csv has the answers",
        csv.lines()
            .nth(1)
            .is_some_and(|row| row.contains(",yes,no,no,no,no,")),
        || csv.clone(),
    );
    t.step("leave");
    t.keys("q");
    t.exits();
}

#[test]
fn the_rules_answers_stay_hidden_until_asked_for() {
    let mut t = term!(a_drawn_sample, &["kinds", "label"]);
    t.documents("SPECS.md#A sample classified by hand");
    t.wait_for("What artifact is being tested?");
    let screen = t.screen();
    t.check(
        "the rules say nothing yet",
        !screen.contains("rules y"),
        || screen.clone(),
    );
    t.step("answer the first option, then show the rules");
    t.keys("y");
    t.keys("r");
    t.wait_for("questions · rules shown (r)");
    t.wait_for("[y] Code           rules y ✓");
    t.step("back to the first option, to read why the rules said yes");
    t.keys("k");
    t.wait_for("rules: it names code from src/lib.rs");
}

/// Two checkouts beside the scratch repository's files: one of a GitHub repository with a
/// test, one with no remote at all.
fn checkouts_to_draw_from(root: &Path) {
    let git = |dir: &Path, args: &[&str]| {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .unwrap();
        assert!(out.status.success(), "{args:?}: {out:?}");
    };
    let alpha = root.join("checkouts/alpha");
    common::scratch_repo(&alpha);
    git(
        &alpha,
        &[
            "remote",
            "add",
            "origin",
            "https://github.com/someone/alpha.git",
        ],
    );
    std::fs::write(
        alpha.join("tests.rs"),
        "#[test]\nfn checks_it() {\n    assert!(true);\n}\n",
    )
    .unwrap();
    git(&alpha, &["add", "."]);
    git(&alpha, &["commit", "-qm", "a test"]);
    common::scratch_repo(&root.join("checkouts/elsewhere"));
}

#[test]
fn tests_are_drawn_from_github_checkouts_into_the_dataset() {
    let mut t = term!(
        checkouts_to_draw_from,
        &["kinds", "draw", "checkouts", "--count", "5", "--seed", "1"]
    );
    t.documents("SPECS.md#A sample classified by hand");
    t.wait_for("passed over");
    t.wait_for("its origin is not on GitHub");
    t.wait_for("someone-alpha-");
    t.wait_for("drew 1 of 1 tests in 1 repositories");
    t.exits();
    let dir = t.repo().join(codereview::kinds::DEFAULT_DATASET);
    let csv = std::fs::read_to_string(dir.join("kinds.csv")).unwrap_or_default();
    t.check(
        "the CSV names the repository and the commit",
        csv.contains(",someone/alpha,") && csv.contains(",tests.rs,"),
        || csv.clone(),
    );
    let stored = std::fs::read_dir(&dir)
        .unwrap()
        .flatten()
        .any(|e| e.path().join("test.rs.test").is_file());
    t.check("the test's text is stored beside it", stored, || {
        format!(
            "{:?}",
            std::fs::read_dir(&dir)
                .unwrap()
                .flatten()
                .map(|e| e.path())
                .collect::<Vec<_>>()
        )
    });
}

/// The drawn sample, classified by hand as testing nothing at all.
fn a_finished_sample(root: &Path) {
    use codereview::classify::Answer;
    use codereview::kinds::{DEFAULT_DATASET, Dataset, Status};
    a_drawn_sample(root);
    let dir = root.join(DEFAULT_DATASET);
    let mut dataset = Dataset::open(&dir).unwrap();
    let sample = &mut dataset.samples[0];
    sample
        .answers
        .iter_mut()
        .for_each(|a| *a = Some(Answer::No));
    sample.status = Status::Done;
    dataset.save().unwrap();
}

#[test]
fn the_rules_are_measured_against_the_samples_classified_by_hand() {
    let mut t = term!(a_finished_sample, &["kinds", "agree", "--verbose"]);
    t.documents("SPECS.md#A sample classified by hand");
    t.wait_for("1 samples classified by hand");
    t.wait_for("What artifact is being tested?");
    t.wait_for("0/1   options: Code 0, Config 1");
    t.wait_for("by hand Nothing, rules Code (it names code from src/lib.rs)");
    t.exits();
}

#[test]
fn the_file_of_a_sample_is_read_and_searched_while_labelling() {
    let mut t = term!(a_drawn_sample, &["kinds", "label"]);
    t.documents("SPECS.md#A sample classified by hand");
    t.wait_for("What artifact is being tested?");
    t.step("move to the file, to read the test");
    t.keys("c");
    t.wait_for("line 6 of 8");
    t.wait_for("Esc questions");
    t.step("search for the function the test calls");
    t.keys("/parse_review");
    t.wait_for("/parse_review▏");
    t.keys(ENTER);
    t.wait_for("/parse_review: 2 of 2");
    t.wait_for("line 7 of 8");
    t.step("search again, for where it is defined");
    t.keys("n");
    t.wait_for("/parse_review: 1 of 2, from the other end");
    t.wait_for("line 1 of 8");
    t.step("back to the questions, and answer one");
    t.keys(ESC);
    t.wait_for("y n i answer");
    t.keys("y");
    t.wait_for("[y] Code");
}

/// Two drawn samples: the one above, and a second from another repository.
fn two_drawn_samples(root: &Path) {
    use codereview::kinds::{DEFAULT_DATASET, Dataset};
    a_drawn_sample(root);
    let dir = root.join(DEFAULT_DATASET);
    let mut dataset = Dataset::open(&dir).unwrap();
    let mut second = dataset.samples[0].clone();
    second.name = "someone-beta-0123456-lib-prints".into();
    second.repository = "someone/beta".into();
    second.test = "prints".into();
    second.label = "prints".into();
    std::fs::create_dir_all(dir.join(&second.name)).unwrap();
    std::fs::write(
        dir.join(&second.name).join("file.rs.test"),
        "pub fn print() {}\n\n\n\n\n#[test]\nfn prints() {\n    print();\n}\n",
    )
    .unwrap();
    dataset.samples.push(second);
    dataset.save().unwrap();
}

#[test]
fn a_sample_set_aside_leads_to_the_next_one_pending() {
    let mut t = term!(two_drawn_samples, &["kinds", "label"]);
    t.documents("SPECS.md#A sample classified by hand");
    t.wait_for("1/2 · 0 done · someone/alpha");
    t.step("set the first sample aside");
    t.keys("s");
    t.wait_for("now skipped");
    t.step("go to the next sample still pending");
    t.keys("}");
    t.wait_for("2/2 · 0 done · someone/beta");
    t.wait_for("▌fn prints() {");
}

#[test]
fn a_note_is_kept_with_the_sample() {
    let mut t = term!(a_drawn_sample, &["kinds", "label"]);
    t.documents("SPECS.md#A sample classified by hand");
    t.wait_for("What artifact is being tested?");
    t.step("write a note on the sample");
    t.keys("e");
    t.wait_for("Enter saves, Esc leaves it");
    t.keys("calls a helper");
    t.keys(ENTER);
    t.wait_for("note saved");
    t.wait_for("note: calls a helper");
    let csv = std::fs::read_to_string(
        t.repo()
            .join(codereview::kinds::DEFAULT_DATASET)
            .join("kinds.csv"),
    )
    .unwrap();
    t.check(
        "the note is in kinds.csv",
        csv.trim_end().ends_with(",calls a helper"),
        || csv.clone(),
    );
}
