//! What an end-to-end test leaves behind when it runs: a recording of the software being
//! driven, and `test.json`, which says which test it was, how it went, and what it did when.
//!
//! The tool runs nothing to get these. A project's own test run writes them, the way it
//! writes a `junit.xml`, and they are read from disk when asked for, like every other result:
//! a recording is a file the run left behind and nothing the tool keeps. They are looked for
//! in a few directories by convention, each holding a directory per test; `SPECS.md` has the
//! format.
//!
//! A terminal recording is an asciicast, the bytes a program wrote with the time it wrote
//! them. Neither front end is a terminal emulator, so the bytes are played through one here,
//! once, into frames of styled text that both of them can draw: the page as spans, the
//! terminal UI as cells.

use std::path::Path;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

/// Where recordings are looked for, relative to the repository root: this tool's own
/// convention, then Playwright's default output directory.
pub const DIRS: &[&str] = &["target/e2e", "test-results", "e2e-results"];

/// How deep below one of [`DIRS`] a `test.json` may be: a suite, then a test.
const DEPTH: usize = 4;

/// At most this many recordings are read, so a directory of thousands costs a bounded walk.
const MOST: usize = 2000;

/// The file a test leaves beside its recording.
pub const FILE_NAME: &str = "test.json";

/// One thing a test did, or checked, at a moment of its recording.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Step {
    /// Milliseconds since the recording started.
    pub at_ms: u64,
    /// The line of the test's file that made the step; 0 when it is not known.
    #[serde(default)]
    pub line: usize,
    /// `action` or `check`.
    pub kind: String,
    pub text: String,
    /// For a check: whether it passed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub passed: Option<bool>,
    /// For a check that failed: what was there instead.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// The test that ran another file's tests: a harness that starts a browser and hands it a
/// script, say. It is not a test of its own, and is left out of the list its tests are in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunBy {
    pub file: String,
    pub test: String,
}

/// What became a recording, as a front end shows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// A video the browser can play.
    Video,
    /// An asciicast, played as frames of text.
    Terminal,
}

/// One test's run, as its `test.json` says.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Recording {
    /// The test's name as written: a function's name, or the string a `test(...)` call was
    /// given.
    pub test: String,
    /// The file it is defined in, relative to the repository root.
    pub file: String,
    #[serde(default)]
    pub line: usize,
    pub passed: bool,
    /// When it started, RFC 3339.
    #[serde(default)]
    pub started: Option<String>,
    #[serde(default)]
    pub duration_ms: u64,
    /// The commit under test, and whether the tree had changes on top of it.
    #[serde(default)]
    pub commit: Option<String>,
    #[serde(default)]
    pub dirty: bool,
    /// The recording's file name, beside `test.json`. `None` when it names nothing that is
    /// there.
    #[serde(default)]
    pub recording: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_by: Option<RunBy>,
    #[serde(default)]
    pub steps: Vec<Step>,
    /// Where `test.json` was found, relative to the repository root. Filled in when read.
    #[serde(default)]
    pub dir: String,
    /// What the recording is, from its name. Filled in when read.
    #[serde(default)]
    pub kind: Option<Kind>,
}

impl Recording {
    /// The recording's path relative to the repository root, when there is one.
    pub fn media(&self) -> Option<String> {
        self.recording
            .as_ref()
            .map(|name| format!("{}/{name}", self.dir))
    }

    /// The checks that failed, in order.
    pub fn failures(&self) -> impl Iterator<Item = &Step> {
        self.steps
            .iter()
            .filter(|s| s.kind == "check" && s.passed == Some(false))
    }
}

/// What a recording's file name says it is.
pub fn kind_of(name: &str) -> Option<Kind> {
    let ext = name.rsplit_once('.')?.1.to_ascii_lowercase();
    match ext.as_str() {
        "webm" | "mp4" | "ogv" => Some(Kind::Video),
        "cast" => Some(Kind::Terminal),
        _ => None,
    }
}

/// The media type a recording is served as.
pub fn media_type(name: &str) -> Option<&'static str> {
    let ext = name.rsplit_once('.')?.1.to_ascii_lowercase();
    Some(match ext.as_str() {
        "webm" => "video/webm",
        "mp4" => "video/mp4",
        "ogv" => "video/ogg",
        "cast" => "application/x-asciicast",
        _ => return None,
    })
}

/// Every recording under [`DIRS`] in the repository at `root`, in the order of the files and
/// lines of their tests. A `test.json` that does not parse is left out, and so is a recording
/// it names that is not a plain file beside it: the name comes from whatever wrote the file,
/// and is served on the strength of having been found here.
pub fn find(root: &Path) -> Vec<Recording> {
    let mut out = Vec::new();
    for dir in DIRS {
        walk(root, &root.join(dir), DEPTH, &mut out);
    }
    out.sort_by(|a, b| (&a.file, a.line, &a.test).cmp(&(&b.file, b.line, &b.test)));
    out
}

/// A number that moves when a test run leaves, replaces or removes a `test.json` under
/// [`DIRS`]: those directories are ignored by git, so its status never says.
pub fn stamp(root: &Path) -> u64 {
    use std::hash::{DefaultHasher, Hash, Hasher};
    fn visit(dir: &Path, depth: usize, seen: &mut usize, hasher: &mut DefaultHasher) {
        let Ok(meta) = std::fs::symlink_metadata(dir) else {
            return;
        };
        if !meta.is_dir() || *seen >= MOST {
            return;
        }
        if let Ok(file) = std::fs::symlink_metadata(dir.join(FILE_NAME)) {
            *seen += 1;
            dir.hash(hasher);
            file.len().hash(hasher);
            file.modified().ok().hash(hasher);
            return;
        }
        if depth == 0 {
            return;
        }
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        let mut children: Vec<_> = entries.flatten().map(|e| e.path()).collect();
        children.sort();
        for child in children {
            visit(&child, depth - 1, seen, hasher);
        }
    }
    let mut hasher = DefaultHasher::new();
    let mut seen = 0;
    for dir in DIRS {
        visit(&root.join(dir), DEPTH, &mut seen, &mut hasher);
    }
    hasher.finish()
}

fn walk(root: &Path, dir: &Path, depth: usize, out: &mut Vec<Recording>) {
    if out.len() >= MOST {
        return;
    }
    // Not followed through a link: what is served from here has to be inside the repository.
    let Ok(meta) = std::fs::symlink_metadata(dir) else {
        return;
    };
    if !meta.is_dir() {
        return;
    }
    let file = dir.join(FILE_NAME);
    if std::fs::symlink_metadata(&file).is_ok_and(|m| m.is_file()) {
        if let Some(recording) = read(root, dir) {
            out.push(recording);
        }
        return;
    }
    if depth == 0 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut children: Vec<_> = entries.flatten().map(|e| e.path()).collect();
    children.sort();
    for child in children {
        walk(root, &child, depth - 1, out);
    }
}

/// The `test.json` in `dir`, with where it was found and what its recording is.
fn read(root: &Path, dir: &Path) -> Option<Recording> {
    let text = std::fs::read_to_string(dir.join(FILE_NAME)).ok()?;
    let mut recording: Recording = serde_json::from_str(&text).ok()?;
    recording.dir = dir
        .strip_prefix(root)
        .ok()?
        .to_str()?
        .replace(std::path::MAIN_SEPARATOR, "/");
    recording.recording = recording.recording.filter(|name| {
        !name.is_empty()
            && !name.contains(['/', '\\'])
            && name != ".."
            && kind_of(name).is_some()
            && std::fs::symlink_metadata(dir.join(name)).is_ok_and(|m| m.is_file())
    });
    recording.kind = recording.recording.as_deref().and_then(kind_of);
    Some(recording)
}

// ----- terminal recordings -------------------------------------------------------------------

/// A colour as a terminal names it: an index into its palette, which the front end maps to
/// its own colours, or red, green and blue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Colour {
    Index(u8),
    Rgb(u8, u8, u8),
}

/// A stretch of one row with the same look.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Run {
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fg: Option<Colour>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bg: Option<Colour>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub bold: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub italic: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub underline: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub inverse: bool,
}

fn is_false(b: &bool) -> bool {
    !b
}

/// The screen at one moment: the rows that changed since the frame before, each whole. The
/// first frame has every row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Frame {
    pub at_ms: u64,
    pub rows: Vec<(u16, Vec<Run>)>,
}

/// A terminal recording played into frames.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cast {
    pub width: u16,
    pub height: u16,
    pub duration_ms: u64,
    pub frames: Vec<Frame>,
}

impl Cast {
    /// The whole screen at `at_ms`: every row as the last frame at or before then left it.
    pub fn screen_at(&self, at_ms: u64) -> Vec<Vec<Run>> {
        let mut rows = vec![Vec::new(); usize::from(self.height)];
        for frame in self.frames.iter().take_while(|f| f.at_ms <= at_ms) {
            for (row, runs) in &frame.rows {
                if let Some(slot) = rows.get_mut(usize::from(*row)) {
                    *slot = runs.clone();
                }
            }
        }
        rows
    }
}

/// A recording larger than this is not played: a terminal session of that size is not a test.
const MOST_CAST_BYTES: usize = 32 << 20;

/// Output closer together than this is one frame: a program draws a screen in several writes.
const SETTLE_MS: u64 = 16;

/// Larger screens than this are refused, rather than allocated.
const MOST_CELLS: usize = 500 * 200;

/// Plays an asciicast (version 2) through a terminal emulator into frames.
pub fn frames(text: &str) -> Result<Cast> {
    if text.len() > MOST_CAST_BYTES {
        bail!("the recording is over {} MB", MOST_CAST_BYTES >> 20);
    }
    let mut lines = text.lines();
    let header: serde_json::Value =
        serde_json::from_str(lines.next().context("an empty recording")?)
            .context("the recording's first line is not its header")?;
    if header["version"] != 2 {
        bail!("only version 2 asciicasts are played");
    }
    let size = |key: &str| {
        header[key]
            .as_u64()
            .and_then(|n| u16::try_from(n).ok())
            .filter(|n| *n > 0)
    };
    let (Some(width), Some(height)) = (size("width"), size("height")) else {
        bail!("the recording does not say how big its terminal was");
    };
    if usize::from(width) * usize::from(height) > MOST_CELLS {
        bail!("a {width} by {height} terminal is too big to play");
    }
    let mut parser = vt100::Parser::new(height, width, 0);
    let mut frames: Vec<Frame> = Vec::new();
    let mut shown: Vec<Vec<Run>> = vec![Vec::new(); usize::from(height)];
    let mut pending: Option<u64> = None;
    let mut last_ms = 0;
    let mut take = |parser: &vt100::Parser, at_ms: u64, frames: &mut Vec<Frame>| {
        let screen = parser.screen();
        let mut rows = Vec::new();
        for row in 0..height {
            let runs = row_runs(screen, row, width);
            if frames.is_empty() || runs != shown[usize::from(row)] {
                shown[usize::from(row)] = runs.clone();
                rows.push((row, runs));
            }
        }
        if !rows.is_empty() {
            frames.push(Frame { at_ms, rows });
        }
    };
    for line in lines {
        if line.trim().is_empty() {
            continue;
        }
        let Ok((at, kind, data)) = serde_json::from_str::<(f64, String, String)>(line) else {
            continue;
        };
        if kind != "o" {
            continue;
        }
        let at_ms = (at.max(0.0) * 1000.0) as u64;
        if let Some(start) = pending {
            if at_ms > start + SETTLE_MS {
                take(&parser, start, &mut frames);
                pending = None;
            }
        }
        parser.process(data.as_bytes());
        pending.get_or_insert(at_ms);
        last_ms = at_ms;
    }
    if let Some(start) = pending {
        take(&parser, start, &mut frames);
    }
    Ok(Cast {
        width,
        height,
        duration_ms: last_ms,
        frames,
    })
}

fn colour(c: vt100::Color) -> Option<Colour> {
    match c {
        vt100::Color::Default => None,
        vt100::Color::Idx(i) => Some(Colour::Index(i)),
        vt100::Color::Rgb(r, g, b) => Some(Colour::Rgb(r, g, b)),
    }
}

/// One row of the screen as runs of the same look, without the blank tail.
fn row_runs(screen: &vt100::Screen, row: u16, width: u16) -> Vec<Run> {
    let mut runs: Vec<Run> = Vec::new();
    for col in 0..width {
        let Some(cell) = screen.cell(row, col) else {
            continue;
        };
        // The second half of a wide character: drawn by the first.
        if cell.is_wide_continuation() {
            continue;
        }
        let contents = cell.contents();
        let text = if contents.is_empty() { " " } else { contents };
        let look = Run {
            text: String::new(),
            fg: colour(cell.fgcolor()),
            bg: colour(cell.bgcolor()),
            bold: cell.bold(),
            italic: cell.italic(),
            underline: cell.underline(),
            inverse: cell.inverse(),
        };
        match runs.last_mut() {
            Some(last)
                if Run {
                    text: String::new(),
                    ..last.clone()
                } == look =>
            {
                last.text.push_str(text);
            }
            _ => runs.push(Run {
                text: text.to_string(),
                ..look
            }),
        }
    }
    // Spaces nothing colours are the screen, not the row.
    while let Some(last) = runs.last_mut() {
        if last.bg.is_some() || last.inverse || last.underline {
            break;
        }
        let kept = last.text.trim_end_matches(' ').len();
        last.text.truncate(kept);
        if !last.text.is_empty() {
            break;
        }
        runs.pop();
    }
    runs
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cast(events: &[(f64, &str)]) -> String {
        let mut out = String::from("{\"version\":2,\"width\":20,\"height\":3}\n");
        for (at, data) in events {
            out.push_str(&serde_json::json!([at, "o", data]).to_string());
            out.push('\n');
        }
        out.push_str("[9.0,\"i\",\"q\"]\n");
        out
    }

    #[test]
    fn a_terminal_recording_plays_into_frames_of_what_changed() {
        let text = cast(&[
            (0.0, "hello"),
            // Drawn in two writes a moment apart: one frame.
            (0.005, "\r\n\x1b[1;31mred\x1b[0m"),
            (0.5, "\x1b[1;1Hjello"),
        ]);
        let cast = frames(&text).unwrap();
        assert_eq!((cast.width, cast.height, cast.duration_ms), (20, 3, 500));
        assert_eq!(cast.frames.len(), 2, "{:#?}", cast.frames);
        let first = &cast.frames[0];
        assert_eq!(first.at_ms, 0);
        assert_eq!(first.rows.len(), 3, "the first frame is the whole screen");
        assert_eq!(first.rows[0].1[0].text, "hello");
        let red = &first.rows[1].1[0];
        assert_eq!(
            (red.text.as_str(), red.fg, red.bold),
            ("red", Some(Colour::Index(1)), true)
        );
        assert!(first.rows[2].1.is_empty(), "a blank row has no runs");
        // Only the row that changed, and the input event is not output.
        let second = &cast.frames[1];
        assert_eq!(second.at_ms, 500);
        assert_eq!(second.rows.len(), 1);
        assert_eq!(second.rows[0].1[0].text, "jello");
        let screen = cast.screen_at(10_000);
        assert_eq!(screen[0][0].text, "jello");
        assert_eq!(screen[1][0].text, "red");
        assert_eq!(cast.screen_at(100)[0][0].text, "hello");
    }

    #[test]
    fn a_recording_that_is_not_one_is_refused() {
        assert!(frames("").is_err());
        assert!(frames("{\"version\":1,\"width\":2,\"height\":2}\n").is_err());
        assert!(frames("{\"version\":2}\n").is_err());
        assert!(frames("{\"version\":2,\"width\":100000,\"height\":100000}\n").is_err());
        // A broken event is skipped rather than fatal.
        let cast =
            frames("{\"version\":2,\"width\":4,\"height\":1}\nnonsense\n[0.1,\"o\",\"ok\"]\n")
                .unwrap();
        assert_eq!(cast.frames.len(), 1);
    }

    #[test]
    fn recordings_are_found_where_runs_leave_them() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let put = |path: &str, text: &str| {
            let path = root.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        };
        put(
            "target/e2e/terminal/opens/test.json",
            r#"{"test": "opens", "file": "tests/t.rs", "line": 3, "passed": true,
                "recording": "terminal.cast", "steps": [{"at_ms": 5, "line": 4,
                "kind": "check", "text": "shows it", "passed": true}]}"#,
        );
        put("target/e2e/terminal/opens/terminal.cast", "{}");
        // A name that leads elsewhere is not served, and nor is one that is not there.
        put(
            "test-results/a/test.json",
            r#"{"test": "a", "file": "e2e/a.spec.ts", "passed": false,
                "recording": "../../secret.webm"}"#,
        );
        put(
            "test-results/b/test.json",
            r#"{"test": "b", "file": "e2e/a.spec.ts", "line": 9, "passed": true,
                "recording": "video.webm"}"#,
        );
        put("test-results/c/test.json", "not json");
        put(
            "elsewhere/test.json",
            r#"{"test": "x", "file": "x", "passed": true}"#,
        );

        let found = find(root);
        let names: Vec<&str> = found.iter().map(|r| r.test.as_str()).collect();
        assert_eq!(names, ["a", "b", "opens"]);
        let opens = &found[2];
        assert_eq!(opens.dir, "target/e2e/terminal/opens");
        assert_eq!(
            opens.media().as_deref(),
            Some("target/e2e/terminal/opens/terminal.cast")
        );
        assert_eq!(opens.kind, Some(Kind::Terminal));
        assert_eq!(opens.steps[0].text, "shows it");
        assert_eq!(found[0].media(), None);
        assert_eq!(found[1].media(), None, "video.webm is not there");
        assert_eq!(found[0].failures().count(), 0);
    }
}
