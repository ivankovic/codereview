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
    t.step("open the changed code nothing ran");
    t.keys("k");
    t.keys("k");
    t.wait_for("ran in no end-to-end test.");
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
fn the_sections_run_along_the_top_with_the_tests_first() {
    let mut t = term!();
    t.documents("SPECS.md#Terminal UI");
    t.on_the_tests();
    let screen = t.screen();
    let top = screen.lines().next().unwrap_or_default().to_string();
    let order =
        ["1 Tests", "2 All tests", "3 Code", "4 Review", "5 Agent"].map(|name| top.find(name));
    t.check(
        "the top row is Tests, All tests, Code, Review, Agent, in that order",
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
fn a_number_goes_to_a_section_and_q_comes_back_to_the_tests() {
    let mut t = term!();
    t.documents("SPECS.md#Terminal UI");
    t.on_the_tests();
    t.step("go to the review, the fourth section");
    t.keys("4");
    t.wait_for(" review 0 pending, 0 completed");
    t.step("go to the code, the third");
    t.keys("3");
    t.wait_for("files 5");
    t.step("step back out of the code");
    t.keys("q");
    t.wait_for("1 end-to-end test of 2 tests");
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
fn a_pending_comment_is_counted_beside_review_in_the_top_row() {
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
        "the top row counts one pending comment beside Review",
        top.contains("4 Review 1"),
        || top.clone(),
    );
}

#[test]
fn a_test_s_source_opens_over_the_tests_and_q_goes_back_to_them() {
    let mut t = term!();
    t.documents("SPECS.md#Terminal UI");
    t.wait_for("○ a review survives a restart");
    t.step("open the test's source");
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
    t.step("go to every test, the second section");
    t.keys("2");
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
    t.step("go to every test, the second section");
    t.keys("2");
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
    t.step("go to every test and find the unit test");
    t.keys("2");
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
