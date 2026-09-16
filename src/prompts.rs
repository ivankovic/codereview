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
