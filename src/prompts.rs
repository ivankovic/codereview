//! The fixed prompts the front ends offer, so that both word a request the same way. Each
//! says what to change and where to write the answer, because an agent asked vaguely answers
//! vaguely.

/// Asks the agent to act on one review comment. `line_label` is `None` for a comment on a
/// whole file or directory.
pub fn address_comment(path: &str, line_label: Option<&str>, text: &str) -> String {
    let where_ = match line_label {
        Some(label) => format!("In {path} on line {label}"),
        None => format!("In {path}"),
    };
    format!(
        "Address this review comment from REVIEW.md, then move it from Pending to Completed \
         in REVIEW.md (keep the line's format).\n\n{where_}: {text}"
    )
}

/// Asks the agent to work through every pending comment.
pub fn address_all() -> &'static str {
    "Work through every comment under `# Pending` in REVIEW.md, in order. For each one: make \
     the change it asks for, then move its line to `# Completed`, keeping the line's format. \
     If a comment is unclear or wrong, leave it pending and say why."
}

/// Asks a question about a piece of code, or about a whole path when there is no range.
pub fn about_code(path: &str, lines: Option<(usize, usize)>, question: &str) -> String {
    match lines {
        Some((first, last)) => {
            let range = crate::review::line_label(first, Some(last));
            format!("{question}\n\n(About {path}, line {range}.)")
        }
        None => format!("{question}\n\n(About {path}.)"),
    }
}

/// Asks for a review of a diff, with findings written as REVIEW.md comments.
pub fn review_diff(path: &str, target: &str) -> String {
    format!(
        "Review the change to {path} ({target}). For each problem you find, add a line under \
         `# Pending` in REVIEW.md of the form `- In {path} on line N: <finding> - \"<the line's \
         text>\"`, where N is the line number in the current file. Then summarise."
    )
}

/// Asks for a repository's first end-to-end tests, made so that each leaves the recording
/// this tool shows beside it. `kind` is what the software looks like it is, when that is known.
pub fn first_end_to_end(kind: Option<&str>) -> String {
    let what = match kind {
        Some("a web page") => {
            "Drive the page in a real headless browser (Playwright or puppeteer), the way a \
             person would: open it, tap, type, and check what is on the screen."
        }
        Some("a terminal program") => {
            "Run the built program in a pseudo-terminal, the way a person would: press keys, \
             and check what is on the screen."
        }
        Some(_) => {
            "Run the built program the way a person would: with the arguments and input they \
             would give it, checking what it prints and what it leaves on disk."
        }
        None => {
            "Drive the software from outside, the way the person or program using it would, \
             and check what comes back."
        }
    };
    format!(
        "This repository has no end-to-end tests: nothing drives it the way a person does and \
         checks what comes back. Write the first ones.\n\n\
         Pick the two or three things a person most often does with it, and write one test for \
         each, named as the sentence it demonstrates. {what} Use a scratch copy of whatever \
         the software works on, never the user's own.\n\n\
         Each test must leave a record of its run in `target/e2e/<suite>/<test name, lower \
         case, dashes between words>/`, also when it fails:\n\
         - the recording: `video.webm` from a browser, or `terminal.cast` (asciicast v2) from \
         a terminal, made at a person's pace, so that someone watching can follow it;\n\
         - `test.json`: `{{\"test\": <its name as written>, \"file\": <its file, relative \
         to the repository root>, \"line\": <the line it is defined on>, \"passed\": bool, \
         \"started\": <RFC 3339>, \"duration_ms\": n, \"commit\": <git rev-parse HEAD>, \
         \"dirty\": <whether git status shows changes>, \"recording\": <the recording's \
         file name>, \"steps\": [{{\"at_ms\": <ms since the recording started>, \"line\": \
         <the line of the test that took the step>, \"kind\": \"action\" or \"check\", \
         \"text\": <what a person would say it does or checks>, \"passed\": bool, for a \
         check}}]}}`.\n\n\
         Put the recording code in one small harness the tests share. Make the tests run with \
         the project's usual test command, or a make target when they need something more, \
         such as a browser; run them, and say how to run them again."
    )
}
