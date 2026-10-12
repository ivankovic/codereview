//! What kind of test each one is, by the eight questions of "What Types of Automated Tests do
//! Developers Write?" (Ivanković, Rimanić, Budiselić, Petrović, Fraser and Just, AST 2025), and
//! a ninth of our own: whether it feeds the code malformed input, and how that input was made.
//!
//! The paper set out to sort tests into unit, integration and system tests and found that
//! nobody, its own authors included, agreed on what those meant. What it settled on instead is
//! a set of questions about what a test does, each with a few options, and every option
//! answered on its own as yes, no or inconclusive. A question is then summarised: as its one
//! option that holds, as "mixed" when several do, as "none" when all are conclusively no, and
//! as "not classified" when any option could not be decided. A test can test code and
//! configuration at once; one that cannot be read honestly is not classified.
//!
//! The answers here are worked out from what can be read without running anything: the test's
//! own lines, the few above it (attributes and decorators), the files of code it names (from
//! the symbol index), and whether its file drives the program from outside. Each answer says
//! why in a few words, because a rule a reviewer cannot check is a rule a reviewer cannot
//! trust. Where the evidence a rule needs is missing — the index not built yet — the answer is
//! inconclusive rather than a guess.

use std::path::Path;
use std::sync::OnceLock;

use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::testsuite::TestCase;

/// One of the paper's eight questions, or the ninth, on malformed input.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Question {
    Artifact,
    Execution,
    Interaction,
    Measurement,
    Outputs,
    OutputAssertions,
    ProtocolAssertions,
    Dependencies,
    MalformedInput,
}

/// What a question is summarised as when one of its options could not be decided.
pub const NOT_CLASSIFIED: &str = "Not classified";

impl Question {
    pub const ALL: [Question; 9] = [
        Question::Artifact,
        Question::Execution,
        Question::Interaction,
        Question::Measurement,
        Question::Outputs,
        Question::OutputAssertions,
        Question::ProtocolAssertions,
        Question::Dependencies,
        Question::MalformedInput,
    ];

    /// As the paper asks it.
    pub fn title(self) -> &'static str {
        match self {
            Question::Artifact => "What artifact is being tested?",
            Question::Execution => "Is the test executing the artifact?",
            Question::Interaction => "How does the test interact with it?",
            Question::Measurement => "Is the test performing a numerical measurement?",
            Question::Outputs => "How are outputs analyzed?",
            Question::OutputAssertions => "What assertions are done on the output?",
            Question::ProtocolAssertions => {
                "What assertions are done on the implementation or protocol?"
            }
            Question::Dependencies => "How does the test handle dependencies?",
            Question::MalformedInput => "Is the test using malformed input?",
        }
    }

    /// A word or two, for a column or a picker.
    pub fn short(self) -> &'static str {
        match self {
            Question::Artifact => "artifact",
            Question::Execution => "execution",
            Question::Interaction => "interaction",
            Question::Measurement => "measurement",
            Question::Outputs => "outputs",
            Question::OutputAssertions => "output assertions",
            Question::ProtocolAssertions => "protocol assertions",
            Question::Dependencies => "dependencies",
            Question::MalformedInput => "malformed input",
        }
    }

    /// The options, each answered on its own.
    pub fn options(self) -> &'static [&'static str] {
        match self {
            Question::Artifact => &["Code", "Config", "Data", "Documentation", "Hardware"],
            Question::Execution => &["Dynamic", "Static"],
            Question::Interaction => &["API", "GUI/TUI", "CLI"],
            Question::Measurement => &["Build-time", "Run-time"],
            Question::Outputs => &["As is", "Post-processed"],
            Question::OutputAssertions | Question::ProtocolAssertions => {
                &["Existence", "Constrained", "Exact match"]
            }
            Question::Dependencies => &["Mocks", "Fakes"],
            Question::MalformedInput => &["Targeted attack", "Heuristic", "Random"],
        }
    }

    /// The summary when every option is conclusively no.
    fn none(self) -> &'static str {
        match self {
            Question::Artifact => "Nothing",
            Question::Execution => "Neither",
            Question::Interaction => "No interaction",
            Question::Measurement => "No measurement",
            Question::Outputs => "Not analyzed",
            Question::OutputAssertions | Question::ProtocolAssertions => "No assertions",
            Question::Dependencies => "Real dependencies",
            Question::MalformedInput => "Well-formed input",
        }
    }

    /// The summary when more than one option holds.
    fn several(self) -> &'static str {
        match self {
            Question::Artifact => "Multiple artifacts",
            Question::Execution => "Both dynamic and static",
            Question::Interaction => "Several interfaces",
            Question::Measurement => "Both build-time and run-time",
            Question::Outputs => "Both",
            Question::OutputAssertions | Question::ProtocolAssertions => "Mixed",
            Question::Dependencies => "Both fakes and mocks",
            Question::MalformedInput => "Several kinds",
        }
    }

    /// Every summary the question can come to, in the order its groups are shown: each option
    /// on its own, then several, then none, then not classified.
    pub fn summaries(self) -> Vec<&'static str> {
        let mut out: Vec<&'static str> = self.options().to_vec();
        out.extend([self.several(), self.none(), NOT_CLASSIFIED]);
        out
    }

    /// The question after this one, wrapping: for a key that steps through them.
    pub fn next(self) -> Question {
        let at = Self::ALL
            .iter()
            .position(|q| *q == self)
            .expect("a question");
        Self::ALL[(at + 1) % Self::ALL.len()]
    }
}

/// What a group of tests sharing `summary` as their answer to `question` are, in a sentence,
/// and what it means that there are none: a finding, for the groups whose absence says the
/// software is not shown working.
pub fn describe(question: Question, summary: &str) -> (&'static str, &'static str, bool) {
    const NONE: &str = "None found.";
    match (question, summary) {
        (Question::Artifact, "Code") => ("Tests of the code itself.", NONE, false),
        (Question::Artifact, "Config") => (
            "Tests that read configuration and check what it says.",
            "No test reads the configuration: nothing checks it before it is deployed.",
            false,
        ),
        (Question::Artifact, "Data") => {
            ("Tests that check data the program ships with.", NONE, false)
        }
        (Question::Artifact, "Documentation") => (
            "Tests that read the documentation and check it.",
            "No test reads the documentation: nothing checks that it still says what the code does.",
            false,
        ),
        (Question::Artifact, "Hardware") => ("Tests of physical devices.", NONE, false),
        (Question::Execution, "Dynamic") => ("Tests that run what they test.", NONE, false),
        (Question::Execution, "Static") => (
            "Tests that read what they test without running it: analyses run as tests.",
            NONE,
            false,
        ),
        (Question::Interaction, "API") => (
            "Tests that call the code, or its HTTP API, directly.",
            NONE,
            false,
        ),
        (Question::Interaction, "GUI/TUI") => (
            "Tests that drive a screen of the program the way a person does, in a browser or a terminal. With those on its command line, these are the end-to-end tests.",
            "No test drives a screen of the program: nothing here shows it working the way someone would see it.",
            true,
        ),
        (Question::Interaction, "CLI") => (
            "Tests that run the built program on its command line and read what it prints. With those that drive a screen, these are the end-to-end tests.",
            NONE,
            false,
        ),
        (Question::Measurement, "Build-time") => {
            ("Tests that measure something built.", NONE, false)
        }
        (Question::Measurement, "Run-time") => {
            ("Tests that time a run or measure its memory.", NONE, false)
        }
        (Question::Outputs, "As is") => ("Tests that check outputs as they come.", NONE, false),
        (Question::Outputs, "Post-processed") => (
            "Tests that mask, normalise or sort an output before checking it.",
            NONE,
            false,
        ),
        (Question::OutputAssertions | Question::ProtocolAssertions, "Existence") => {
            ("Checks that something is there, or happens.", NONE, false)
        }
        (Question::OutputAssertions | Question::ProtocolAssertions, "Constrained") => (
            "Checks of a condition a correct result must meet.",
            NONE,
            false,
        ),
        (Question::OutputAssertions | Question::ProtocolAssertions, "Exact match") => (
            "Checks that a result is exactly what is expected.",
            NONE,
            false,
        ),
        (Question::ProtocolAssertions, "No assertions") => (
            "Tests that check no call, error or panic of the code.",
            NONE,
            false,
        ),
        (Question::Dependencies, "Mocks") => (
            "Tests that script what the code they call answers.",
            NONE,
            false,
        ),
        (Question::Dependencies, "Fakes") => (
            "Tests that swap a dependency for a fake or a stub.",
            NONE,
            false,
        ),
        (Question::Dependencies, "Real dependencies") => {
            ("Tests that use the real thing throughout.", NONE, false)
        }
        (Question::MalformedInput, "Targeted attack") => (
            "Tests that feed the code input made to break it: tampered, forged or injected.",
            NONE,
            false,
        ),
        (Question::MalformedInput, "Heuristic") => (
            "Tests that feed the code input someone chose to be wrong: invalid, truncated, corrupt.",
            "No test feeds the code input that is wrong: nothing shows it refusing what it should.",
            true,
        ),
        (Question::MalformedInput, "Random") => (
            "Tests that generate their input: fuzzing and property tests.",
            NONE,
            false,
        ),
        (Question::MalformedInput, "Well-formed input") => (
            "Tests that give the code only what it expects.",
            NONE,
            false,
        ),
        (_, NOT_CLASSIFIED) => (
            "Tests some option of which could not be decided from their text.",
            NONE,
            false,
        ),
        _ => ("", NONE, false),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Answer {
    Yes,
    No,
    Inconclusive,
}

/// One option, answered, and why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Said {
    pub option: String,
    pub answer: Answer,
    pub why: String,
}

/// One question, answered option by option, and what that comes to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Answered {
    pub question: Question,
    pub summary: String,
    /// Why it came to its summary, in a few words: see [`Answered::because`].
    pub reason: String,
    pub options: Vec<Said>,
}

impl Answered {
    /// Why it came to its summary: the reasons of the options that hold, or of the one that
    /// could not be decided, or of the first that does not hold.
    pub fn because(options: &[Said]) -> String {
        let of = |a: Answer| -> Vec<&str> {
            options
                .iter()
                .filter(|s| s.answer == a)
                .map(|s| s.why.as_str())
                .collect()
        };
        let inconclusive = of(Answer::Inconclusive);
        if let Some(why) = inconclusive.first() {
            return (*why).to_string();
        }
        let yes = of(Answer::Yes);
        if !yes.is_empty() {
            return yes.join("; ");
        }
        of(Answer::No)
            .first()
            .map_or(String::new(), |w| (*w).to_string())
    }
}

/// Every question, answered for one test.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Kind {
    pub answers: Vec<Answered>,
}

impl Kind {
    pub fn of(&self, question: Question) -> &Answered {
        self.answers
            .iter()
            .find(|a| a.question == question)
            .expect("every question is answered")
    }

    pub fn summary(&self, question: Question) -> &str {
        &self.of(question).summary
    }

    /// Whether `option` of `question` holds.
    pub fn holds(&self, question: Question, option: &str) -> bool {
        self.of(question)
            .options
            .iter()
            .any(|s| s.option == option && s.answer == Answer::Yes)
    }

    /// Why `option` of `question` is answered the way it is.
    pub fn why(&self, question: Question, option: &str) -> &str {
        self.of(question)
            .options
            .iter()
            .find(|s| s.option == option)
            .map_or("", |s| s.why.as_str())
    }

    /// An end-to-end test: one that runs the program and drives it the way a person does.
    pub fn end_to_end(&self) -> bool {
        self.holds(Question::Execution, "Dynamic")
            && (self.holds(Question::Interaction, "GUI/TUI")
                || self.holds(Question::Interaction, "CLI"))
    }
}

/// The paper's summary of one question from its options' answers.
pub fn summarise(question: Question, options: &[Said]) -> String {
    if options.iter().any(|s| s.answer == Answer::Inconclusive) {
        return NOT_CLASSIFIED.to_string();
    }
    let yes: Vec<&Said> = options.iter().filter(|s| s.answer == Answer::Yes).collect();
    match yes.as_slice() {
        [] => question.none().to_string(),
        [one] => one.option.clone(),
        _ => question.several().to_string(),
    }
}

/// What is known about one test, to answer the questions from.
pub struct Evidence<'a> {
    pub test: &'a TestCase,
    /// The test's own lines.
    pub body: &'a str,
    /// The few lines above it, where attributes and decorators are.
    pub above: &'a str,
    /// The files of code its body names, from the symbol index; `None` before the index is
    /// built, when nothing can be said about what it calls.
    pub names: Option<&'a [String]>,
    /// Why it drives the program from outside — a browser, a terminal, the built program —
    /// when it does, in a few words: "it drives a browser", "its path says e2e".
    pub drives: Option<&'a str>,
    /// Whether a repository-relative path is a file of the repository: to tell a file of it
    /// from a scratch file of the same name. A closure, so that the rules can run against a
    /// checkout or against what a sample recorded of one.
    pub exists: &'a dyn Fn(&str) -> bool,
}

/// `path` with `.` and `..` resolved, or `None` when it climbs out of the repository or is
/// absolute.
fn normalise(path: &str) -> Option<String> {
    if path.starts_with('/') {
        return None;
    }
    let mut parts: Vec<&str> = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            _ => parts.push(part),
        }
    }
    Some(parts.join("/"))
}

/// The repository paths a string written in a test could mean: as it is, from the repository
/// root, and from the test's own directory.
fn candidates(test_path: &str, written: &str) -> Vec<String> {
    let dir = test_path.rsplit_once('/').map_or("", |(d, _)| d);
    let mut out: Vec<String> = normalise(written).into_iter().collect();
    if !dir.is_empty()
        && let Some(p) = normalise(&format!("{dir}/{written}"))
        && !out.contains(&p)
    {
        out.push(p);
    }
    out
}

/// Every file of the repository a string in `body` names: what a sample keeps of its
/// checkout, so that the rules can be run again without it.
pub fn mentioned(body: &str, test_path: &str, exists: &dyn Fn(&str) -> bool) -> Vec<String> {
    static LITERAL: OnceLock<Regex> = OnceLock::new();
    let literal = regex(&LITERAL, r#""([^"\\\n]{1,200})"|'([^'\\\n]{2,200})'"#);
    let mut out: Vec<String> = Vec::new();
    for c in literal.captures_iter(body) {
        let Some(m) = c.get(1).or(c.get(2)) else {
            continue;
        };
        for p in candidates(test_path, m.as_str()) {
            if !p.is_empty() && !out.contains(&p) && exists(&p) {
                out.push(p);
            }
        }
    }
    out
}

fn said(option: &str, answer: Answer, why: impl Into<String>) -> Said {
    Said {
        option: option.to_string(),
        answer,
        why: why.into(),
    }
}

fn yes_no(option: &str, found: Option<String>, otherwise: &str) -> Said {
    match found {
        Some(why) => said(option, Answer::Yes, why),
        None => said(option, Answer::No, otherwise),
    }
}

fn regex(cell: &'static OnceLock<Regex>, pattern: &str) -> &'static Regex {
    cell.get_or_init(|| Regex::new(pattern).expect("regex"))
}

/// The first match of `re` in `text`, as written.
fn first(re: &Regex, text: &str) -> Option<String> {
    re.find(text).map(|m| m.as_str().trim().to_string())
}

/// What a file the test reads is, by its name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Artifact {
    Code,
    Config,
    Data,
    Documentation,
}

fn artifact_of(path: &str) -> Option<Artifact> {
    let lower = path.to_lowercase();
    let name = lower.rsplit('/').next().unwrap_or(&lower);
    let ext = name.rsplit_once('.').map_or("", |(_, e)| e);
    let under =
        |dir: &str| lower.starts_with(&format!("{dir}/")) || lower.contains(&format!("/{dir}/"));
    if matches!(ext, "md" | "markdown" | "rst" | "adoc")
        || name.starts_with("readme")
        || under("docs")
        || under("doc")
    {
        return Some(Artifact::Documentation);
    }
    if matches!(
        ext,
        "csv" | "tsv" | "jsonl" | "ndjson" | "parquet" | "sql" | "sqlite" | "db"
    ) || under("testdata")
        || under("fixtures")
        || under("data")
    {
        return Some(Artifact::Data);
    }
    if matches!(
        ext,
        "toml" | "yaml" | "yml" | "ini" | "cfg" | "conf" | "properties" | "env" | "service"
    ) || matches!(
        name,
        "dockerfile" | "makefile" | "package.json" | "tsconfig.json"
    ) || name.ends_with(".config.json")
        || name.ends_with(".config.js")
        || name.ends_with(".config.ts")
        || under(".github")
    {
        return Some(Artifact::Config);
    }
    if crate::testsuite::testable(name) {
        return Some(Artifact::Code);
    }
    None
}

/// The files of the repository the test reads as they are, with what each is: a string that
/// names a file that exists, on a line that reads a file, in a test that either reads it by
/// that name directly or anchors its paths at the repository (`CARGO_MANIFEST_DIR`,
/// `__dirname`, ...). A file the test wrote into a scratch directory under the same name is
/// what the program did, not a file of the repository, and has no such anchor.
fn reads(e: &Evidence) -> Vec<(String, Artifact)> {
    static READ: OnceLock<Regex> = OnceLock::new();
    static DIRECT: OnceLock<Regex> = OnceLock::new();
    static ANCHOR: OnceLock<Regex> = OnceLock::new();
    static LITERAL: OnceLock<Regex> = OnceLock::new();
    let read = regex(
        &READ,
        r"read_to_string|read_text|readFile|ReadFile|include_str!|include_bytes!|fs::read\b|\bopen\s*\(|\bload\s*\(|\bparse_file\b",
    );
    let direct = regex(
        &DIRECT,
        r#"(?:read_to_string|read_text|readFileSync|readFile|ReadFile|include_str!|include_bytes!|fs::read|\bopen)\s*\(\s*["'](?:\./)?([^"']+)["']"#,
    );
    let anchor = regex(
        &ANCHOR,
        r"CARGO_MANIFEST_DIR|__dirname|__file__|import\.meta|repo_root|project_root|REPO_ROOT|PROJECT_ROOT",
    );
    let literal = regex(&LITERAL, r#""([^"\\\n]{1,200})"|'([^'\\\n]{2,200})'"#);
    let anchored = anchor.is_match(e.body);
    let mut out: Vec<(String, Artifact)> = Vec::new();
    for line in e.body.lines() {
        if !read.is_match(line) || line.contains("write") || line.contains("create") {
            continue;
        }
        let mut paths: Vec<String> = direct
            .captures_iter(line)
            .map(|c| c[1].to_string())
            .collect();
        if anchored {
            paths.extend(
                literal
                    .captures_iter(line)
                    .filter_map(|c| c.get(1).or(c.get(2)))
                    .map(|m| m.as_str().to_string()),
            );
        }
        for written in paths {
            let Some(p) = candidates(&e.test.path, &written)
                .into_iter()
                .find(|p| (e.exists)(p))
            else {
                continue;
            };
            if out.iter().any(|(q, _)| *q == p) {
                continue;
            }
            if let Some(kind) = artifact_of(&p) {
                out.push((p, kind));
            }
        }
    }
    out
}

/// Whether the body runs a linter, formatter or other analysis over the code.
fn analyses(body: &str) -> Option<String> {
    static RE: OnceLock<Regex> = OnceLock::new();
    first(
        regex(
            &RE,
            r"\b(?:clippy|rustfmt|cargo fmt|eslint|prettier|ruff|flake8|pylint|mypy|golangci-lint|go vet|shellcheck|tsc --noEmit)\b",
        ),
        body,
    )
}

/// An HTTP call: an API of the program reached over the network, the paper's RPC endpoint.
fn http(body: &str) -> Option<String> {
    static RE: OnceLock<Regex> = OnceLock::new();
    first(
        regex(
            &RE,
            r#"\bfetch\s*\(|\breqwest::|\brequests\.(?:get|post|put|delete|patch)\b|\bhttpx\.|\baxios\b|\bhttp\.(?:Get|Post|NewRequest)\b|\bcurl\b|\bsupertest\b|["']/api/"#,
        ),
        body,
    )
}

/// A clock read for a measurement: a duration taken and compared.
fn timer(body: &str) -> Option<String> {
    static RE: OnceLock<Regex> = OnceLock::new();
    static COMPARED: OnceLock<Regex> = OnceLock::new();
    let found = first(
        regex(
            &RE,
            r"Instant::now\(\)|\.elapsed\(\)|performance\.now\(\)|time\.perf_counter\(\)|time\.monotonic\(\)|time\.Since\(|Date\.now\(\)|tracemalloc|memoryUsage\(\)|\bb\.N\b",
        ),
        body,
    )?;
    let compared = regex(
        &COMPARED,
        r"elapsed|duration|took|\bms\b|millis|secs|memory",
    );
    body.lines()
        .any(|l| (l.contains('<') || l.contains('>')) && compared.is_match(l) && !l.contains("->"))
        .then_some(found)
}

/// A size of something built, compared: the paper's build-time measurement.
fn built_size(body: &str) -> Option<String> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = regex(
        &RE,
        r#"(?:metadata\([^)]*\)\.(?:unwrap\(\)\.)?len\(\)|stat\([^)]*\)\.st_size|\.size\b)[^\n]*|["'](?:target|dist|build|out)/[^"']*["'][^\n]*(?:len|size)"#,
    );
    re.find(body)
        .filter(|m| {
            let s = m.as_str();
            s.contains("target") || s.contains("dist") || s.contains("build") || s.contains("bin")
        })
        .map(|m| m.as_str().trim().chars().take(40).collect())
}

/// What the test does to an output before it looks at it.
fn post_processing(body: &str) -> Option<String> {
    static RE: OnceLock<Regex> = OnceLock::new();
    first(
        regex(
            &RE,
            r"\bmask\w*|\bnormali[sz]e\w*|\bredact\w*|\bsanitize\w*|\bstrip_ansi\w*|\bre\.sub\b|\bscrub\w*",
        ),
        body,
    )
}

/// The assertions made on outputs, by what each demands: exactly this value, a condition it
/// must meet, or only that there is one.
fn output_checks(body: &str) -> [Option<String>; 3] {
    static EXACT: OnceLock<Regex> = OnceLock::new();
    static CONSTRAINED: OnceLock<Regex> = OnceLock::new();
    static EXISTENCE: OnceLock<Regex> = OnceLock::new();
    let exact = regex(
        &EXACT,
        r"\bassert_eq!|\bassertEquals?\b|\bassertListEqual\b|\bassertDictEqual\b|\.toBe\(|\.toEqual\(|\.toStrictEqual\(|\bdeepEqual\b|\bstrictEqual\b|\bassert\.equal\b|\bt\.is\(|\b(?:assert|require)\.Equal\b|\bassert [^\n]*==|\bif got != want\b|\.toMatchSnapshot\(|\bassert_snapshot!",
    );
    let constrained = regex(
        &CONSTRAINED,
        r"\bassert_ne!|\bassert!\s*\(|\bdebug_assert!|\bassert(?:True|False|In|NotIn|Greater\w*|Less\w*|Regex|AlmostEqual|NotEqual)\b|\.to(?:Contain\w*|Match\w*|BeGreater\w*|BeLess\w*|HaveLength|BeCloseTo)\(|\b(?:assert|require)\.(?:Contains|Greater\w*|Less\w*|Regexp|True|False|Len|NotEqual)\b|\bassert [^\n]*(?:\bin\b|<|>|!=)|\bt\.ok\(|\bt\.check\(|\bt\.wait_for\(|\bwait_for\w*\(|\bexpect\s*\([^)]*\)\.(?:not\.)?to\w*\(|\bassert\.(?:ok|match)\b|\bt\.Error",
    );
    let existence = regex(
        &EXISTENCE,
        r"\.is_some\(\)|\.is_ok\(\)|\.is_none\(\)|\.exists\(\)|\.toBeDefined\(|\.toBeTruthy\(|\.toBeNull\(|\.toBeVisible\(|\bassertIsNotNone\b|\bassertIsNone\b|\b(?:assert|require)\.(?:NotNil|Nil|NoError|NotEmpty)\b|(?m)\bassert [a-z_][a-z0-9_.]*\s*$",
    );
    static HELPER: OnceLock<Regex> = OnceLock::new();
    // A project's own checking function: what it demands is in its body, not its call, so it
    // is taken for a condition, and says that it is a helper.
    let helper = regex(&HELPER, r"\b(?:assert|check|verify)_[a-z_]+\s*\(");
    let mut constrained_found = None;
    for line in body.lines() {
        // `assert!(x.is_some())` is a check of existence, not a condition on a value.
        if constrained.is_match(line) && !existence.is_match(line) && !exact.is_match(line) {
            constrained_found = first(constrained, line);
            break;
        }
    }
    let constrained_found =
        constrained_found.or_else(|| first(helper, body).map(|w| format!("{w}, a helper")));
    [
        first(existence, body),
        constrained_found,
        first(exact, body),
    ]
}

/// The assertions made on the implementation or the protocol: that something is called, or
/// raises, or does not — on mocks, and on errors.
fn protocol_checks(body: &str, above: &str, calls_code: bool) -> [Option<String>; 3] {
    static EXACT: OnceLock<Regex> = OnceLock::new();
    static CONSTRAINED: OnceLock<Regex> = OnceLock::new();
    static EXISTENCE: OnceLock<Regex> = OnceLock::new();
    static UNWRAP: OnceLock<Regex> = OnceLock::new();
    let exact = regex(
        &EXACT,
        r"toHaveBeenCalledWith\(|toHaveBeenCalledTimes\(|toHaveBeenNthCalledWith\(|assert_called_once_with\(|assert_called_with\(|assert_has_calls\(|\.times\(\d+\)|\.Times\(\d+\)|EXPECT_CALL|\.with\(eq\(",
    );
    let constrained = regex(
        &CONSTRAINED,
        r"\.times\(\d+\.\.|\.Times\(gomock\.(?:AtLeast|AtMost)|\bcall_count\s*[<>]|\.atLeast\(|\.atMost\(|AnyTimes\(\)",
    );
    let existence = regex(
        &EXISTENCE,
        r"toHaveBeenCalled\(\)|assert_called\(\)|assert_called_once\(\)|assert_not_called\(\)|\.called\b|should_panic|assertRaises|pytest\.raises|\.toThrow\w*\(|\.rejects\.|\.unwrap_err\(\)|\.expect_err\(|\.is_err\(\)|catch_unwind|\bassert(?:\.|_)?[Pp]anics\b|require\.Error\b|assert\.Error\b",
    );
    let unwrap = regex(&UNWRAP, r"\.unwrap\(\)|\.expect\(|\?;");
    let existence_found = first(existence, &format!("{above}\n{body}")).or_else(|| {
        // Calling the code and requiring that it does not fail is the paper's example of an
        // existence assertion on the protocol: "calling a chain of methods does not throw".
        calls_code
            .then(|| first(unwrap, body))
            .flatten()
            .map(|w| format!("{w} on what it calls"))
    });
    [
        existence_found,
        first(constrained, body),
        first(exact, body),
    ]
}

fn mocks(body: &str) -> Option<String> {
    static RE: OnceLock<Regex> = OnceLock::new();
    first(
        regex(
            &RE,
            r"\bMock[A-Z]\w*|\bmockall\b|\.expect_[a-z_]+\(\)|\bunittest\.mock\b|\bMagicMock\b|\bMock\(|\bpatch(?:\.object)?\(|\bmocker\.|\bjest\.(?:fn|mock|spyOn)\b|\bvi\.(?:fn|mock|spyOn)\b|\bsinon\.|\bgomock\b|\bmock\.Mock\b|\bnock\(",
        ),
        body,
    )
}

fn fakes(body: &str) -> Option<String> {
    static RE: OnceLock<Regex> = OnceLock::new();
    first(
        regex(
            &RE,
            r"(?i)\b\w*(?:fake|stub|dummy|in_memory|inmemory|test_double)\w*\b",
        ),
        body,
    )
}

/// Input made to break the code: a word of the test's name saying it was tampered with or
/// forged, or a payload of a known attack in its body.
fn attack(test: &TestCase, body: &str) -> Option<String> {
    const WORDS: &[&str] = &[
        "tamper",
        "tampered",
        "tampering",
        "forged",
        "forge",
        "forgery",
        "spoof",
        "spoofed",
        "malicious",
        "attack",
        "attacker",
        "evil",
        "hostile",
        "injection",
        "inject",
        "injected",
        "traversal",
        "xss",
        "csrf",
        "ssrf",
        "smuggling",
        "replay",
        "replayed",
        "overflow",
        "exploit",
    ];
    static PAYLOAD: OnceLock<Regex> = OnceLock::new();
    let name = crate::spec::words(&format!("{} {}", test.name, test.label));
    if let Some(word) = name.iter().find(|w| WORDS.contains(&w.as_str())) {
        return Some(format!("its name says {word}"));
    }
    first(
        regex(
            &PAYLOAD,
            r#"\.\./\.\./|<script|javascript:|' OR '|' OR 1|; DROP TABLE|%00|\\x00|\$\{jndi:"#,
        ),
        body,
    )
    .map(|w| format!("it sends {w}"))
}

/// Input someone chose to be wrong: a word of the test's name saying so.
fn heuristic(test: &TestCase) -> Option<String> {
    const WORDS: &[&str] = &[
        "invalid",
        "malformed",
        "bad",
        "broken",
        "corrupt",
        "corrupted",
        "garbage",
        "junk",
        "bogus",
        "nonsense",
        "unparsable",
        "unparseable",
        "truncated",
        "illegal",
        "wrong",
        "garbled",
        "mangled",
    ];
    let name = crate::spec::words(&format!("{} {}", test.name, test.label));
    name.iter()
        .find(|w| WORDS.contains(&w.as_str()))
        .map(|w| format!("its name says {w}"))
}

/// Input generated for the test: a fuzzer or a property-based testing library.
fn generated(body: &str, above: &str) -> Option<String> {
    static RE: OnceLock<Regex> = OnceLock::new();
    first(
        regex(
            &RE,
            r"\bproptest!|\bprop_compose!|#\[quickcheck\]|\bquickcheck\b|\barbitrary::|\bArbitrary\b|\bfuzz_target!|\bbolero::|@given\b|\bhypothesis\b|\bfc\.(?:assert|property)\b|\bfast-check\b|\bf\.Fuzz\(|\btesting/quick\b|\bfuzz(?:er|ers|ing|ed)?\b",
        ),
        &format!("{above}\n{body}"),
    )
}

/// Whether a test in `test` can call code in `file` directly: the same language, and for a
/// Rust test outside `src`, built as a crate of its own, not the binary's own files.
fn callable(test: &str, file: &str) -> bool {
    let family = |p: &str| match p.rsplit_once('.').map_or("", |(_, e)| e) {
        "rs" => "rust",
        "py" => "python",
        "go" => "go",
        "js" | "jsx" | "mjs" | "cjs" | "ts" | "tsx" | "mts" | "cts" => "javascript",
        _ => "",
    };
    if family(test).is_empty() || family(test) != family(file) {
        return false;
    }
    let outside_src = !test.starts_with("src/") && !test.contains("/src/");
    !(family(test) == "rust"
        && outside_src
        && (file.ends_with("src/main.rs") || file.contains("src/bin/")))
}

/// Answers every question for one test.
pub fn classify(e: &Evidence) -> Kind {
    let read = reads(e);
    let read_of = |kind: Artifact| {
        read.iter()
            .find(|(_, k)| *k == kind)
            .map(|(p, _)| format!("it reads {p}"))
    };
    // What it calls of the program, through its API.
    let named = e
        .names
        .map(|n| n.iter().find(|f| callable(&e.test.path, f)).cloned());
    let calls = match named {
        Some(Some(file)) => Some(Some(format!("it names code from {file}"))),
        Some(None) => Some(None),
        None => None,
    };
    let over_http = http(e.body).map(|w| format!("it calls an HTTP API ({w})"));
    let drives = e.drives.map(str::to_string);
    let source = read_of(Artifact::Code);
    let linted = analyses(e.body).map(|w| format!("it runs {w}"));
    let dynamic = calls
        .clone()
        .map(|c| c.or(drives.clone()).or(over_http.clone()));

    let code = match &dynamic {
        Some(Some(why)) => said("Code", Answer::Yes, why.clone()),
        _ if source.is_some() || linted.is_some() => said(
            "Code",
            Answer::Yes,
            source.clone().or(linted.clone()).unwrap_or_default(),
        ),
        Some(None) if !read.is_empty() => {
            said("Code", Answer::No, "it names no code; it reads files")
        }
        Some(None) => said(
            "Code",
            Answer::Inconclusive,
            "it names no code of the repository",
        ),
        None => said(
            "Code",
            Answer::Inconclusive,
            "the symbol index is not built yet",
        ),
    };
    let artifact = vec![
        code,
        yes_no(
            "Config",
            read_of(Artifact::Config),
            "it reads no configuration file",
        ),
        yes_no("Data", read_of(Artifact::Data), "it reads no data file"),
        yes_no(
            "Documentation",
            read_of(Artifact::Documentation),
            "it reads no documentation",
        ),
        said("Hardware", Answer::No, "it drives no device"),
    ];

    let statically = source
        .clone()
        .or(linted.clone())
        .or_else(|| read.first().map(|(p, _)| format!("it reads {p} as it is")));
    let execution = vec![
        match &dynamic {
            Some(Some(why)) => said("Dynamic", Answer::Yes, why.clone()),
            Some(None) if statically.is_some() => said(
                "Dynamic",
                Answer::No,
                "it reads files without running anything",
            ),
            Some(None) => said("Dynamic", Answer::Inconclusive, "it names no code it runs"),
            None => said(
                "Dynamic",
                Answer::Inconclusive,
                "the symbol index is not built yet",
            ),
        },
        yes_no("Static", statically, "it reads no file of the repository"),
    ];

    let api = match &calls {
        Some(Some(why)) => said("API", Answer::Yes, why.clone()),
        _ if over_http.is_some() => said("API", Answer::Yes, over_http.clone().unwrap_or_default()),
        Some(None) => said("API", Answer::No, "it calls no code of the repository"),
        None => said(
            "API",
            Answer::Inconclusive,
            "the symbol index is not built yet",
        ),
    };
    // The built program run on its command line is the CLI; anything else that drives the
    // program from outside — a browser, a terminal, a path or a recording that says so — a
    // screen of it.
    let on_the_command_line = drives
        .as_deref()
        .is_some_and(|w| w.contains("the built program"));
    let interaction = vec![
        api,
        yes_no(
            "GUI/TUI",
            drives.clone().filter(|_| !on_the_command_line),
            if on_the_command_line {
                "it drives no browser or terminal; it runs the built program"
            } else {
                "it drives no browser or terminal"
            },
        ),
        yes_no(
            "CLI",
            drives.clone().filter(|_| on_the_command_line),
            "it runs no built program on its command line",
        ),
    ];

    let measurement = vec![
        yes_no(
            "Build-time",
            built_size(e.body).map(|w| format!("it measures {w}")),
            "it measures nothing built",
        ),
        yes_no(
            "Run-time",
            timer(e.body).map(|w| format!("it times a run with {w}")),
            "it times nothing",
        ),
    ];

    let [exists, constrained, exact] = output_checks(e.body);
    let checks_output = exists.is_some() || constrained.is_some() || exact.is_some();
    let output_assertions = vec![
        yes_no(
            "Existence",
            exists.map(|w| format!("it checks with {w}")),
            "no check of presence alone",
        ),
        yes_no(
            "Constrained",
            constrained.map(|w| format!("it checks with {w}")),
            "no check of a condition",
        ),
        yes_no(
            "Exact match",
            exact.map(|w| format!("it checks with {w}")),
            "no check of an exact value",
        ),
    ];

    let processed = post_processing(e.body);
    let outputs = match (checks_output, processed) {
        (false, _) => vec![
            said("As is", Answer::No, "it checks no output"),
            said("Post-processed", Answer::No, "it checks no output"),
        ],
        (true, None) => vec![
            said("As is", Answer::Yes, "it checks outputs as they come"),
            said(
                "Post-processed",
                Answer::No,
                "nothing is done to them first",
            ),
        ],
        (true, Some(w)) => vec![
            said(
                "As is",
                Answer::Inconclusive,
                "some checks may look at the output as it comes",
            ),
            said(
                "Post-processed",
                Answer::Yes,
                format!("it changes an output with {w}"),
            ),
        ],
    };

    let calls_code = matches!(calls, Some(Some(_)));
    let [p_exists, p_constrained, p_exact] = protocol_checks(e.body, e.above, calls_code);
    let protocol_assertions = vec![
        yes_no(
            "Existence",
            p_exists.map(|w| format!("it checks with {w}")),
            "no check that a call happens or fails",
        ),
        yes_no(
            "Constrained",
            p_constrained.map(|w| format!("it checks with {w}")),
            "no bound on how often something is called",
        ),
        yes_no(
            "Exact match",
            p_exact.map(|w| format!("it checks with {w}")),
            "no exact expectation of calls",
        ),
    ];

    let dependencies = vec![
        yes_no(
            "Mocks",
            mocks(e.body).map(|w| format!("it uses {w}")),
            "it scripts no mock",
        ),
        yes_no(
            "Fakes",
            fakes(e.body).map(|w| format!("it uses {w}")),
            "it uses no fake or stub",
        ),
    ];

    let malformed_input = vec![
        yes_no(
            "Targeted attack",
            attack(e.test, e.body),
            "nothing in it is made to break the code",
        ),
        yes_no(
            "Heuristic",
            heuristic(e.test),
            "its name says nothing of input that is wrong",
        ),
        yes_no(
            "Random",
            generated(e.body, e.above).map(|w| format!("it generates input with {w}")),
            "it generates no input",
        ),
    ];

    let answers = [
        (Question::Artifact, artifact),
        (Question::Execution, execution),
        (Question::Interaction, interaction),
        (Question::Measurement, measurement),
        (Question::Outputs, outputs),
        (Question::OutputAssertions, output_assertions),
        (Question::ProtocolAssertions, protocol_assertions),
        (Question::Dependencies, dependencies),
        (Question::MalformedInput, malformed_input),
    ]
    .into_iter()
    .map(|(question, options)| Answered {
        question,
        summary: summarise(question, &options),
        reason: Answered::because(&options),
        options,
    })
    .collect();
    Kind { answers }
}

/// Reads tests' lines from their files, each file once.
pub struct Reader<'a> {
    root: &'a Path,
    files: std::collections::HashMap<String, Vec<String>>,
}

impl<'a> Reader<'a> {
    pub fn new(root: &'a Path) -> Self {
        Self {
            root,
            files: std::collections::HashMap::new(),
        }
    }

    /// The test's own lines, and the few above it.
    pub fn lines(&mut self, test: &TestCase) -> (String, String) {
        let root = self.root;
        let lines = self.files.entry(test.path.clone()).or_insert_with(|| {
            std::fs::read_to_string(root.join(&test.path))
                .unwrap_or_default()
                .lines()
                .map(str::to_string)
                .collect()
        });
        slice(lines, test)
    }
}

/// A test's own lines of its file, and the few above it, from the file's lines.
pub fn slice(lines: &[String], test: &TestCase) -> (String, String) {
    let from = test.line.saturating_sub(1).min(lines.len());
    let to = test.end.max(test.line).min(lines.len());
    let above = from.saturating_sub(4);
    (lines[from..to].join("\n"), lines[above..from].join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn case(path: &str, line: usize, end: usize) -> TestCase {
        TestCase {
            name: "t".into(),
            label: "t".into(),
            path: path.into(),
            line,
            end,
            ignored: false,
            checks: 1,
            shape: 0,
        }
    }

    fn kind(body: &str, names: Option<&[String]>, drives: Option<&str>) -> Kind {
        kind_in(Path::new("/nonexistent"), "tests/t.rs", body, names, drives)
    }

    fn kind_in(
        root: &Path,
        path: &str,
        body: &str,
        names: Option<&[String]>,
        drives: Option<&str>,
    ) -> Kind {
        let test = case(path, 1, body.lines().count());
        classify(&Evidence {
            test: &test,
            body,
            above: "",
            names,
            drives,
            exists: &|p| root.join(p).is_file(),
        })
    }

    #[test]
    fn a_question_is_summarised_the_way_the_paper_does() {
        let s = |answers: &[Answer]| {
            let options: Vec<Said> = answers
                .iter()
                .zip(Question::Interaction.options())
                .map(|(a, o)| said(o, *a, ""))
                .collect();
            summarise(Question::Interaction, &options)
        };
        assert_eq!(s(&[Answer::Yes, Answer::No, Answer::No]), "API");
        assert_eq!(
            s(&[Answer::Yes, Answer::Yes, Answer::No]),
            "Several interfaces"
        );
        assert_eq!(s(&[Answer::No, Answer::No, Answer::No]), "No interaction");
        assert_eq!(
            s(&[Answer::Yes, Answer::Inconclusive, Answer::No]),
            NOT_CLASSIFIED
        );
        assert_eq!(Question::Artifact.summaries().last(), Some(&NOT_CLASSIFIED));
    }

    #[test]
    fn a_unit_test_calls_code_and_checks_an_exact_value() {
        let names = vec!["src/review.rs".to_string()];
        let k = kind_in(
            Path::new("/nonexistent"),
            "src/review.rs",
            "fn parses() {\n    let r = parse_review(TEXT).unwrap();\n    assert_eq!(r.len(), 2);\n}",
            Some(&names),
            None,
        );
        assert_eq!(k.summary(Question::Artifact), "Code");
        assert_eq!(
            k.why(Question::Artifact, "Code"),
            "it names code from src/review.rs"
        );
        assert_eq!(k.summary(Question::Execution), "Dynamic");
        assert_eq!(k.summary(Question::Interaction), "API");
        assert_eq!(k.summary(Question::OutputAssertions), "Exact match");
        assert_eq!(k.summary(Question::ProtocolAssertions), "Existence");
        assert_eq!(k.summary(Question::Outputs), "As is");
        assert_eq!(k.summary(Question::Measurement), "No measurement");
        assert_eq!(k.summary(Question::Dependencies), "Real dependencies");
        assert!(!k.end_to_end());
    }

    #[test]
    fn a_test_that_drives_the_program_is_end_to_end() {
        let k = kind(
            "fn opens() {\n    let mut t = term!();\n    t.wait_for(\"files 5\");\n}",
            Some(&[]),
            Some("it drives a pseudo-terminal"),
        );
        assert_eq!(k.summary(Question::Interaction), "GUI/TUI");
        assert_eq!(
            k.why(Question::Interaction, "GUI/TUI"),
            "it drives a pseudo-terminal"
        );
        assert_eq!(k.summary(Question::Execution), "Dynamic");
        assert_eq!(k.summary(Question::OutputAssertions), "Constrained");
        assert!(k.end_to_end());
    }

    #[test]
    fn a_test_that_runs_the_built_program_is_end_to_end_on_its_command_line() {
        let k = kind(
            "fn lists() {\n    let out = Command::new(env!(\"CARGO_BIN_EXE_x\")).output().unwrap();\n    assert!(out.status.success());\n}",
            Some(&[]),
            Some("it drives the built program"),
        );
        assert_eq!(k.summary(Question::Interaction), "CLI");
        assert_eq!(
            k.why(Question::Interaction, "GUI/TUI"),
            "it drives no browser or terminal; it runs the built program"
        );
        assert!(k.end_to_end());
    }

    #[test]
    fn mocks_fakes_and_their_expectations_are_told_apart() {
        let k = kind_in(
            Path::new("/nonexistent"),
            "src/store.test.ts",
            "it('saves', () => {\n  const save = jest.fn();\n  store(save);\n  expect(save).toHaveBeenCalledWith(1);\n});",
            Some(&["src/store.ts".to_string()]),
            None,
        );
        assert_eq!(k.summary(Question::Dependencies), "Mocks");
        assert_eq!(k.summary(Question::ProtocolAssertions), "Exact match");
        let k = kind(
            "fn agent() {\n    let a = FakeAgent::new();\n    assert!(a.ran());\n}",
            Some(&["src/agent.rs".to_string()]),
            None,
        );
        assert_eq!(k.summary(Question::Dependencies), "Fakes");
    }

    #[test]
    fn a_test_that_reads_the_documentation_tests_documentation() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("docs")).unwrap();
        std::fs::write(dir.path().join("docs/deploy.md"), "listen 443;\n").unwrap();
        let body = "fn the_doc_agrees() {\n    let root = PathBuf::from(env!(\"CARGO_MANIFEST_DIR\"));\n    let doc = std::fs::read_to_string(root.join(\"docs/deploy.md\")).unwrap();\n    assert!(doc.contains(\"listen\"));\n}";
        let k = kind_in(dir.path(), "tests/t.rs", body, Some(&[]), None);
        assert_eq!(k.summary(Question::Artifact), "Documentation");
        assert_eq!(
            k.why(Question::Artifact, "Documentation"),
            "it reads docs/deploy.md"
        );
        assert_eq!(k.summary(Question::Execution), "Static");
        assert_eq!(k.summary(Question::Interaction), "No interaction");
        // The same name in a scratch directory is what the program wrote, not the document.
        let body = "fn writes() {\n    let review = std::fs::read_to_string(dir.join(\"docs/deploy.md\")).unwrap();\n    assert!(review.contains(\"x\"));\n}";
        let k = kind_in(dir.path(), "tests/t.rs", body, Some(&[]), None);
        assert_eq!(k.summary(Question::Artifact), NOT_CLASSIFIED);
    }

    #[test]
    fn without_the_index_what_a_test_calls_is_not_classified() {
        let k = kind("fn x() {\n    assert_eq!(f(), 1);\n}", None, None);
        assert_eq!(k.summary(Question::Artifact), NOT_CLASSIFIED);
        assert_eq!(
            k.why(Question::Interaction, "API"),
            "the symbol index is not built yet"
        );
    }

    #[test]
    fn a_test_calls_only_code_in_its_own_language() {
        // A browser test that shares a word with a Rust function has not called it.
        let rust = vec!["src/anchor.rs".to_string()];
        let k = kind_in(
            Path::new("/nonexistent"),
            "tests/browser/phone.js",
            "test('x', async t => {\n  t.ok('anchored', true);\n});",
            Some(&rust),
            Some("its path says browser"),
        );
        assert_eq!(k.summary(Question::Interaction), "GUI/TUI");
        // A Rust test outside src is a crate of its own: the binary's files are out of reach.
        let main = vec!["src/main.rs".to_string()];
        let k = kind(
            "fn x() {\n    main_loop();\n    assert!(true);\n}",
            Some(&main),
            None,
        );
        assert_eq!(k.summary(Question::Interaction), "No interaction");
    }

    #[test]
    fn a_projects_own_checking_function_is_a_condition() {
        let names = vec!["src/diff.rs".to_string()];
        let k = kind(
            "fn moves() {\n    let d = diff(A, B);\n    assert_matches_human_mapping(&d, MAPPING);\n}",
            Some(&names),
            None,
        );
        assert_eq!(k.summary(Question::OutputAssertions), "Constrained");
        assert_eq!(
            k.why(Question::OutputAssertions, "Constrained"),
            "it checks with assert_matches_human_mapping(, a helper"
        );
        assert_eq!(k.summary(Question::Outputs), "As is");
    }

    #[test]
    fn a_timed_run_is_a_run_time_measurement() {
        let k = kind(
            "fn fast() {\n    let start = Instant::now();\n    build();\n    assert!(start.elapsed() < Duration::from_millis(50));\n}",
            Some(&["src/build.rs".to_string()]),
            None,
        );
        assert_eq!(k.summary(Question::Measurement), "Run-time");
    }

    #[test]
    fn malformed_input_is_told_by_how_it_was_made() {
        let named = |name: &str, body: &str| {
            let mut test = case("tests/t.rs", 1, body.lines().count());
            test.name = name.into();
            test.label = name.replace('_', " ");
            classify(&Evidence {
                test: &test,
                body,
                above: "",
                names: Some(&[]),
                drives: None,
                exists: &|_| false,
            })
        };
        let k = named(
            "a_tampered_signing_request_is_refused",
            "fn x() {\n    assert!(sign(req).is_err());\n}",
        );
        assert_eq!(k.summary(Question::MalformedInput), "Targeted attack");
        assert_eq!(
            k.why(Question::MalformedInput, "Targeted attack"),
            "its name says tampered"
        );
        let k = named(
            "serves_files",
            "fn x() {\n    get(\"/static/../../etc/passwd\");\n}",
        );
        assert_eq!(k.summary(Question::MalformedInput), "Targeted attack");
        let k = named(
            "rejects_an_unparsable_command",
            "fn x() {\n    assert!(parse(\"{\").is_err());\n}",
        );
        assert_eq!(k.summary(Question::MalformedInput), "Heuristic");
        let k = named(
            "round_trips",
            "proptest! {\n    fn x(s in \".*\") {\n        assert_eq!(parse(&print(&s)), s);\n    }\n}",
        );
        assert_eq!(k.summary(Question::MalformedInput), "Random");
        let k = named(
            "fuzzy_matching_ranks_closer_first",
            "fn x() {\n    assert_eq!(rank(\"ab\"), 1);\n}",
        );
        assert_eq!(k.summary(Question::MalformedInput), "Well-formed input");
    }
}
