//! A sample of tests classified by hand, to hold the rules in `classify` to.
//!
//! "What Types of Automated Tests do Developers Write?" built its classification against a
//! sample of tests that people had classified by hand, and kept the sample as the rules' test.
//! This is that sample for codereview's rules. Tests are drawn at random from checkouts of
//! GitHub repositories, each pinned to the repository's `owner/name`, the commit and the file
//! it came from, and stored the way omnidiff stores its fixtures: a directory per sample, under
//! the dataset directory, holding the file verbatim as `file.<ext>.test` and the test's own
//! lines as `test.<ext>.test` — the `.test` suffix keeps them from being read as code or tests
//! of this repository — beside a README saying where they came from and under which licence.
//! `kinds.csv` indexes them, one row per sample, with every option of the eight questions as a
//! column a person fills in from the labelling screen.
//!
//! A sample also keeps what the rules need beyond the text, worked out from the checkout when
//! it was drawn: the files of code the test names, why it drives the program if it does, and
//! the files of the repository its strings name. With those the rules run again on the sample
//! alone, which is how [`agreement`] compares them with the person's answers long after the
//! checkout is gone.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::classify::{Answer, Evidence, Kind, Question, Said};
use crate::repo::Repo;
use crate::testsuite::TestCase;

/// The index of a dataset, in its directory.
pub const CSV: &str = "kinds.csv";

/// Where a dataset is unless another is named: beside this repository's other test data.
pub const DEFAULT_DATASET: &str = "tests/data/kinds";

/// How far a sample has got.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// Drawn, and not every option answered yet.
    Pending,
    /// Every option answered.
    Done,
    /// Set aside: not a test after all, or not one a person can classify from what is there.
    Skipped,
}

impl Status {
    pub fn word(self) -> &'static str {
        match self {
            Status::Pending => "pending",
            Status::Done => "done",
            Status::Skipped => "skipped",
        }
    }

    fn parse(word: &str) -> Result<Self> {
        Ok(match word {
            "" | "pending" => Status::Pending,
            "done" => Status::Done,
            "skipped" => Status::Skipped,
            _ => bail!("{word:?} is not a status"),
        })
    }
}

/// Every option of every question, in the order of the columns.
pub fn options() -> Vec<(Question, &'static str)> {
    Question::ALL
        .iter()
        .flat_map(|&q| q.options().iter().map(move |&o| (q, o)))
        .collect()
}

/// The column an option is answered in: `interaction.gui`, `output_assertions.exact_match`.
pub fn column(question: Question, option: &str) -> String {
    let word = |s: &str| s.to_lowercase().replace([' ', '-'], "_");
    format!("{}.{}", word(question.short()), word(option))
}

fn answer_word(answer: Option<Answer>) -> &'static str {
    match answer {
        None => "",
        Some(Answer::Yes) => "yes",
        Some(Answer::No) => "no",
        Some(Answer::Inconclusive) => "inconclusive",
    }
}

fn parse_answer(word: &str) -> Result<Option<Answer>> {
    Ok(match word {
        "" => None,
        "yes" => Some(Answer::Yes),
        "no" => Some(Answer::No),
        "inconclusive" => Some(Answer::Inconclusive),
        _ => bail!("{word:?} is not an answer"),
    })
}

/// One test of the sample.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sample {
    /// Its directory under the dataset.
    pub name: String,
    /// `owner/name` on GitHub.
    pub repository: String,
    /// The full hash of the commit it was drawn at.
    pub commit: String,
    /// Its file, relative to the repository.
    pub path: String,
    /// 1-based, first and last line of the test in its file.
    pub line: usize,
    pub end: usize,
    /// As written, and as a phrase.
    pub test: String,
    pub label: String,
    /// The files of code its body names, as the symbol index of the checkout said.
    pub names: Vec<String>,
    /// Why it drives the program from outside, if it does; empty when it does not.
    pub drives: String,
    /// The files of the repository the strings in it name.
    pub files: Vec<String>,
    pub status: Status,
    /// One per entry of [`options`], `None` while unanswered.
    pub answers: Vec<Option<Answer>>,
    pub notes: String,
}

impl Sample {
    /// The file's extension, which its stored copies carry before `.test`.
    pub fn extension(&self) -> &str {
        let name = self.path.rsplit('/').next().unwrap_or(&self.path);
        name.rsplit_once('.').map_or("txt", |(_, e)| e)
    }

    /// Whether every option has an answer.
    pub fn answered(&self) -> bool {
        self.answers.iter().all(Option::is_some)
    }

    /// The test as the rules see one.
    pub fn case(&self) -> TestCase {
        TestCase {
            name: self.test.clone(),
            label: self.label.clone(),
            path: self.path.clone(),
            line: self.line,
            end: self.end,
            ignored: false,
            checks: 0,
            shape: 0,
        }
    }

    /// The person's answers to `question`, as the rules give theirs.
    pub fn said(&self, question: Question) -> Vec<Said> {
        options()
            .iter()
            .zip(&self.answers)
            .filter(|((q, _), _)| *q == question)
            .map(|((_, option), answer)| Said {
                option: option.to_string(),
                answer: answer.unwrap_or(Answer::Inconclusive),
                why: String::new(),
            })
            .collect()
    }
}

/// A dataset: its directory and the samples its CSV lists.
#[derive(Debug, Clone)]
pub struct Dataset {
    pub dir: PathBuf,
    pub samples: Vec<Sample>,
}

impl Dataset {
    /// The dataset in `dir`; an empty one when it has no CSV yet.
    pub fn open(dir: &Path) -> Result<Self> {
        let path = dir.join(CSV);
        let samples = match std::fs::read_to_string(&path) {
            Ok(text) => parse(&text).with_context(|| format!("reading {}", path.display()))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
        };
        Ok(Self {
            dir: dir.to_path_buf(),
            samples,
        })
    }

    /// Writes the CSV, whole, through a temporary file, so that a crash never leaves half of
    /// one: a person's answers are saved after every key.
    pub fn save(&self) -> Result<()> {
        std::fs::create_dir_all(&self.dir)
            .with_context(|| format!("creating {}", self.dir.display()))?;
        let path = self.dir.join(CSV);
        let temporary = self.dir.join(format!(".{CSV}.new"));
        std::fs::write(&temporary, write(&self.samples))
            .with_context(|| format!("writing {}", temporary.display()))?;
        std::fs::rename(&temporary, &path)
            .with_context(|| format!("replacing {}", path.display()))?;
        Ok(())
    }

    /// The file a sample came from, as it was at its commit.
    pub fn file_text(&self, sample: &Sample) -> Result<String> {
        let path = self
            .dir
            .join(&sample.name)
            .join(format!("file.{}.test", sample.extension()));
        std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))
    }

    /// What the rules answer for a sample, from what it keeps: no checkout needed.
    pub fn replay(&self, sample: &Sample) -> Result<Kind> {
        let text = self.file_text(sample)?;
        let lines: Vec<String> = text.lines().map(str::to_string).collect();
        let case = sample.case();
        let (body, above) = crate::classify::slice(&lines, &case);
        let files: HashSet<&str> = sample.files.iter().map(String::as_str).collect();
        Ok(crate::classify::classify(&Evidence {
            test: &case,
            body: &body,
            above: &above,
            names: Some(&sample.names),
            drives: (!sample.drives.is_empty()).then_some(sample.drives.as_str()),
            exists: &|p| files.contains(p),
        }))
    }
}

// ----- CSV ---------------------------------------------------------------------------------

const FIXED: [&str; 12] = [
    "name",
    "repository",
    "commit",
    "path",
    "line",
    "end",
    "test",
    "label",
    "names",
    "drives",
    "files",
    "status",
];

fn header() -> Vec<String> {
    FIXED
        .iter()
        .map(|s| s.to_string())
        .chain(options().iter().map(|(q, o)| column(*q, o)))
        .chain(["notes".to_string()])
        .collect()
}

fn field(text: &str) -> String {
    if text.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", text.replace('"', "\"\""))
    } else {
        text.to_string()
    }
}

/// The rows of a CSV, quoted fields with doubled quotes and line breaks inside them allowed.
fn records(text: &str) -> Vec<Vec<String>> {
    let mut rows = Vec::new();
    let mut row = Vec::new();
    let mut cell = String::new();
    let mut quoted = false;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match (quoted, c) {
            (true, '"') if chars.peek() == Some(&'"') => {
                chars.next();
                cell.push('"');
            }
            (true, '"') => quoted = false,
            (true, c) => cell.push(c),
            (false, '"') => quoted = true,
            (false, ',') => row.push(std::mem::take(&mut cell)),
            (false, '\r') => {}
            (false, '\n') => {
                row.push(std::mem::take(&mut cell));
                rows.push(std::mem::take(&mut row));
            }
            (false, c) => cell.push(c),
        }
    }
    if !cell.is_empty() || !row.is_empty() {
        row.push(cell);
        rows.push(row);
    }
    rows
}

fn list(text: &str) -> Vec<String> {
    text.split(';')
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

fn write(samples: &[Sample]) -> String {
    let mut out = header().join(",");
    out.push('\n');
    for s in samples {
        let mut cells: Vec<String> = vec![
            s.name.clone(),
            s.repository.clone(),
            s.commit.clone(),
            s.path.clone(),
            s.line.to_string(),
            s.end.to_string(),
            s.test.clone(),
            s.label.clone(),
            s.names.join(";"),
            s.drives.clone(),
            s.files.join(";"),
            s.status.word().to_string(),
        ];
        cells.extend(s.answers.iter().map(|a| answer_word(*a).to_string()));
        cells.push(s.notes.clone());
        out.push_str(&cells.iter().map(|c| field(c)).collect::<Vec<_>>().join(","));
        out.push('\n');
    }
    out
}

/// The samples a CSV lists. Columns are found by name, so one written with fewer options, or
/// in another order, still reads; an option it lacks is unanswered.
fn parse(text: &str) -> Result<Vec<Sample>> {
    let mut rows = records(text).into_iter();
    let Some(head) = rows.next() else {
        return Ok(Vec::new());
    };
    let at: HashMap<&str, usize> = head
        .iter()
        .enumerate()
        .map(|(i, h)| (h.as_str(), i))
        .collect();
    for required in FIXED {
        if !at.contains_key(required) {
            bail!("no {required} column");
        }
    }
    let mut out = Vec::new();
    for (n, row) in rows.enumerate() {
        if row.iter().all(String::is_empty) {
            continue;
        }
        let get = |name: &str| -> &str {
            at.get(name)
                .and_then(|&i| row.get(i))
                .map_or("", String::as_str)
        };
        let number = |name: &str| -> Result<usize> {
            get(name)
                .parse()
                .with_context(|| format!("row {}: {name} is not a number", n + 2))
        };
        let answers = options()
            .iter()
            .map(|(q, o)| parse_answer(get(&column(*q, o))))
            .collect::<Result<Vec<_>>>()
            .with_context(|| format!("row {}", n + 2))?;
        out.push(Sample {
            name: get("name").to_string(),
            repository: get("repository").to_string(),
            commit: get("commit").to_string(),
            path: get("path").to_string(),
            line: number("line")?,
            end: number("end")?,
            test: get("test").to_string(),
            label: get("label").to_string(),
            names: list(get("names")),
            drives: get("drives").to_string(),
            files: list(get("files")),
            status: Status::parse(get("status")).with_context(|| format!("row {}", n + 2))?,
            answers,
            notes: get("notes").to_string(),
        });
    }
    Ok(out)
}

// ----- drawing -----------------------------------------------------------------------------

/// How many to draw and how.
#[derive(Debug, Clone)]
pub struct Draw {
    /// Tests to add.
    pub count: usize,
    /// What the draw is reproduced from.
    pub seed: u64,
    /// At most this many samples from any one repository, those already in the dataset
    /// included, so that one large suite does not make up the sample.
    pub per_repository: usize,
}

/// What a draw did.
#[derive(Debug, Clone, Default)]
pub struct Drawn {
    pub added: Vec<String>,
    /// Tests there were to draw from, in repositories that were read.
    pub candidates: usize,
    pub repositories: usize,
    /// Checkouts left out, and why.
    pub passed_over: Vec<String>,
}

/// The repositories `sources` name: each one the top of a checkout, or a directory of
/// checkouts, which may itself sit inside another repository.
fn checkouts(sources: &[PathBuf]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for source in sources {
        let top = Repo::open(source).ok().is_some_and(|repo| {
            source
                .canonicalize()
                .is_ok_and(|s| repo.root().canonicalize().is_ok_and(|r| r == s))
        });
        if top {
            out.push(source.clone());
        } else {
            out.extend(crate::session::repos_below(source));
        }
    }
    out
}

/// One test that could be drawn.
struct Candidate {
    checkout: usize,
    test: TestCase,
}

/// A small generator for a draw that is the same every time for the same seed (splitmix64).
struct Seeded(u64);

impl Seeded {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn shuffle<T>(&mut self, items: &mut [T]) {
        for i in (1..items.len()).rev() {
            let j = (self.next() % (i as u64 + 1)) as usize;
            items.swap(i, j);
        }
    }
}

/// A directory name from words: lower case, anything else a single dash, at most 90 long.
fn slug(text: &str) -> String {
    let mut out = String::new();
    for c in text.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    let out = out.trim_matches('-');
    let mut cut: String = out.chars().take(90).collect();
    while cut.ends_with('-') {
        cut.pop();
    }
    cut
}

/// One checkout, read for drawing: its name on GitHub, the commit, the files it tracks
/// unchanged, and its tests at that commit.
struct Checkout {
    root: PathBuf,
    repository: String,
    commit: String,
    tracked: HashSet<String>,
    texts: HashMap<String, String>,
}

fn read_checkout(root: &Path, candidates: &mut Vec<Candidate>, index: usize) -> Result<Checkout> {
    let repo = Repo::open(root)?;
    let Some(repository) = repo.github_name() else {
        bail!("its origin is not on GitHub");
    };
    if !repo.has_commits() {
        bail!("it has no commits");
    }
    let commit = repo.resolve("HEAD")?;
    let changed = repo.status()?;
    let tracked: HashSet<String> = repo.list_files()?.into_iter().collect();
    let mut texts = HashMap::new();
    let mut paths: Vec<&String> = tracked.iter().collect();
    paths.sort();
    for path in paths {
        // A file the working tree has changed, or does not track, is not what the commit
        // says; the rest is, and is read from the disk rather than one git process a file.
        if !crate::testsuite::testable(path) || changed.contains_key(path) {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(repo.root().join(path)) else {
            continue;
        };
        let tests = crate::symbols::tests_of(path, &text);
        if tests.is_empty() {
            continue;
        }
        candidates.extend(tests.into_iter().map(|test| Candidate {
            checkout: index,
            test,
        }));
        texts.insert(path.clone(), text);
    }
    Ok(Checkout {
        root: repo.root().to_path_buf(),
        repository,
        commit,
        tracked,
        texts,
    })
}

/// Draws `how.count` tests at random from the checkouts `sources` names and adds them to
/// `dataset`, its CSV saved and each sample's directory written. A test already in the
/// dataset, at any commit, is not drawn again.
pub fn draw(dataset: &mut Dataset, sources: &[PathBuf], how: &Draw) -> Result<Drawn> {
    let mut drawn = Drawn::default();
    let mut candidates = Vec::new();
    let mut read = Vec::new();
    for root in checkouts(sources) {
        let at = read.len();
        match read_checkout(&root, &mut candidates, at) {
            Ok(checkout) => read.push(checkout),
            Err(e) => drawn.passed_over.push(format!("{}: {e:#}", root.display())),
        }
    }
    drawn.repositories = read.len();
    let present: HashSet<(String, String, String)> = dataset
        .samples
        .iter()
        .map(|s| (s.repository.clone(), s.path.clone(), s.test.clone()))
        .collect();
    candidates.retain(|c| {
        let checkout = &read[c.checkout];
        !present.contains(&(
            checkout.repository.clone(),
            c.test.path.clone(),
            c.test.name.clone(),
        ))
    });
    drawn.candidates = candidates.len();
    // Every test is as likely as every other, whatever repository it is in, up to the cap.
    candidates.sort_by(|a, b| {
        (&read[a.checkout].repository, &a.test.path, a.test.line).cmp(&(
            &read[b.checkout].repository,
            &b.test.path,
            b.test.line,
        ))
    });
    Seeded(how.seed).shuffle(&mut candidates);
    let mut per: HashMap<String, usize> = HashMap::new();
    for s in &dataset.samples {
        *per.entry(s.repository.clone()).or_default() += 1;
    }
    let mut picked: Vec<&Candidate> = Vec::new();
    for c in &candidates {
        if picked.len() >= how.count {
            break;
        }
        let n = per.entry(read[c.checkout].repository.clone()).or_default();
        if *n >= how.per_repository {
            continue;
        }
        *n += 1;
        picked.push(c);
    }
    // What the rules need beyond the text, from each checkout a test was picked from.
    let mut evidence: HashMap<usize, (crate::spec::Coverage, Vec<crate::recording::Recording>)> =
        HashMap::new();
    for c in &picked {
        if evidence.contains_key(&c.checkout) {
            continue;
        }
        let checkout = &read[c.checkout];
        let mut session = crate::session::Session::open(&checkout.root)?;
        evidence.insert(
            c.checkout,
            (session.coverage(), crate::recording::find(&checkout.root)),
        );
    }
    for c in picked {
        let checkout = &read[c.checkout];
        let (coverage, recordings) = &evidence[&c.checkout];
        let text = &checkout.texts[&c.test.path];
        let recorded = recordings.iter().any(|r| crate::e2e::names(r, &c.test));
        let drives = crate::e2e::Drivers::new(&checkout.root)
            .why(&c.test, recorded)
            .unwrap_or_default();
        let lines: Vec<String> = text.lines().map(str::to_string).collect();
        let (body, _) = crate::classify::slice(&lines, &c.test);
        let files =
            crate::classify::mentioned(&body, &c.test.path, &|p| checkout.tracked.contains(p));
        let short: String = checkout.commit.chars().take(7).collect();
        let file = c.test.path.rsplit('/').next().unwrap_or(&c.test.path);
        let stem = file.split('.').next().unwrap_or(file);
        let name = unique(
            dataset,
            slug(&format!(
                "{}-{short}-{stem}-{}",
                checkout.repository.replace('/', "-"),
                c.test.name
            )),
        );
        let sample = Sample {
            name: name.clone(),
            repository: checkout.repository.clone(),
            commit: checkout.commit.clone(),
            path: c.test.path.clone(),
            line: c.test.line,
            end: c.test.end.max(c.test.line),
            test: c.test.name.clone(),
            label: c.test.label.clone(),
            // Only code: a file of data the index also reads names nothing a test can call.
            names: coverage
                .reach
                .get(&(c.test.path.clone(), c.test.line))
                .into_iter()
                .flatten()
                .filter(|f| crate::testsuite::testable(f))
                .cloned()
                .collect(),
            drives,
            files,
            status: Status::Pending,
            answers: vec![None; options().len()],
            notes: String::new(),
        };
        store(dataset, &sample, text, &body, &checkout.root)?;
        dataset.samples.push(sample);
        drawn.added.push(name);
    }
    dataset.save()?;
    Ok(drawn)
}

/// `name`, or `name-2`, `name-3`... when the dataset already has a sample called that.
fn unique(dataset: &Dataset, name: String) -> String {
    let taken =
        |n: &str| dataset.samples.iter().any(|s| s.name == n) || dataset.dir.join(n).exists();
    if !taken(&name) {
        return name;
    }
    (2..)
        .map(|i| format!("{name}-{i}"))
        .find(|n| !taken(n))
        .expect("a free name")
}

/// Writes a sample's directory: the file and the test verbatim, and where they came from.
fn store(dataset: &Dataset, s: &Sample, file: &str, body: &str, root: &Path) -> Result<()> {
    let dir = dataset.dir.join(&s.name);
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    let ext = s.extension();
    std::fs::write(dir.join(format!("file.{ext}.test")), file)?;
    let mut body = body.to_string();
    body.push('\n');
    std::fs::write(dir.join(format!("test.{ext}.test")), body)?;
    std::fs::write(dir.join("README.md"), provenance(s, root))?;
    Ok(())
}

/// What a sample's README says: where it is from, and the licence it stays under.
fn provenance(s: &Sample, root: &Path) -> String {
    let licence = Repo::open(root)
        .ok()
        .and_then(|repo| {
            let names = repo.top_level(&s.commit).ok()?;
            let file = names.into_iter().find(|n| {
                let upper = n.to_uppercase();
                upper.starts_with("LICENSE")
                    || upper.starts_with("LICENCE")
                    || upper.starts_with("COPYING")
            })?;
            let text = repo.show(&s.commit, &file).ok()??;
            let first = String::from_utf8_lossy(&text)
                .lines()
                .map(str::trim)
                .find(|l| !l.is_empty())
                .unwrap_or("")
                .to_string();
            Some(format!("`{file}` at this commit, beginning “{first}”."))
        })
        .unwrap_or_else(|| {
            "**Not found.** The repository has no licence file at its top level at this commit; \
             check the repository before reusing this sample outside codereview's own tests."
                .to_string()
        });
    format!(
        "# Sample provenance\n\n\
         - **Repository:** https://github.com/{repo} (`{repo}`)\n\
         - **Commit:** `{commit}`\n\
         - **File:** `{path}`, lines {line}–{end}\n\
         - **Test:** `{test}`\n\n\
         `file.{ext}.test` is that file and `test.{ext}.test` the test's own lines, copied verbatim \
         from the repository at the commit above, to be classified by hand and to hold \
         codereview's test classification to. This content is **not** part of codereview's own \
         code and is **not** covered by its AGPL-3.0 licence: it remains under whatever licence \
         the source repository applies.\n\n\
         ## Licence\n\n{licence}\n",
        repo = s.repository,
        commit = s.commit,
        path = s.path,
        line = s.line,
        end = s.end,
        test = s.test,
        ext = s.extension(),
    )
}

// ----- agreement ---------------------------------------------------------------------------

/// How the rules' answers to one question compare with a person's, over the samples a person
/// has finished.
#[derive(Debug, Clone)]
pub struct Agreement {
    pub question: Question,
    /// Samples whose summary is the same either way, of those finished.
    pub agree: usize,
    pub finished: usize,
    /// Per option: the same answer, of those finished.
    pub options: Vec<(String, usize)>,
    /// Where they differ: the sample, the person's summary, the rules' summary and why.
    pub differ: Vec<(String, String, String, String)>,
}

/// Every question's agreement between the rules and the finished samples.
pub fn agreement(dataset: &Dataset) -> Result<Vec<Agreement>> {
    let finished: Vec<&Sample> = dataset
        .samples
        .iter()
        .filter(|s| s.status == Status::Done)
        .collect();
    let mut kinds = Vec::new();
    for s in &finished {
        kinds.push(dataset.replay(s)?);
    }
    Ok(Question::ALL
        .iter()
        .map(|&question| {
            let mut out = Agreement {
                question,
                agree: 0,
                finished: finished.len(),
                options: question
                    .options()
                    .iter()
                    .map(|o| (o.to_string(), 0))
                    .collect(),
                differ: Vec::new(),
            };
            for (s, kind) in finished.iter().zip(&kinds) {
                let said = s.said(question);
                let rules = kind.of(question);
                for (i, (person, machine)) in said.iter().zip(&rules.options).enumerate() {
                    if person.answer == machine.answer {
                        out.options[i].1 += 1;
                    }
                }
                let summary = crate::classify::summarise(question, &said);
                if summary == rules.summary {
                    out.agree += 1;
                } else {
                    out.differ.push((
                        s.name.clone(),
                        summary,
                        rules.summary.clone(),
                        rules.reason.clone(),
                    ));
                }
            }
            out
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn git(dir: &Path, args: &[&str]) {
        let out = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .unwrap();
        assert!(out.status.success(), "{args:?}: {out:?}");
    }

    /// A checkout of `owner/name` with a module and its tests, one of which reads the README.
    fn checkout(parent: &Path, name: &str) -> PathBuf {
        let dir = parent.join(name);
        std::fs::create_dir_all(dir.join("src")).unwrap();
        git(&dir, &["init", "-q"]);
        git(&dir, &["config", "user.email", "t@example.com"]);
        git(&dir, &["config", "user.name", "T"]);
        git(
            &dir,
            &[
                "remote",
                "add",
                "origin",
                &format!("git@github.com:someone/{name}.git"),
            ],
        );
        std::fs::write(dir.join("LICENSE"), "MIT License\n\nCopyright\n").unwrap();
        std::fs::write(dir.join("README.md"), "# readme\n").unwrap();
        std::fs::write(
            dir.join("src/lib.rs"),
            "pub fn parse_review(s: &str) -> usize { s.len() }\n\n\
             #[cfg(test)]\nmod tests {\n    use super::*;\n\n    #[test]\n    fn parses() {\n        \
             assert_eq!(parse_review(\"ab\"), 2);\n    }\n\n    #[test]\n    fn the_readme_says_so() {\n        \
             let text = std::fs::read_to_string(\"README.md\").unwrap();\n        \
             assert!(text.contains(\"readme\"));\n    }\n}\n",
        )
        .unwrap();
        git(&dir, &["add", "."]);
        git(&dir, &["commit", "-q", "-m", "first"]);
        dir
    }

    #[test]
    fn a_csv_round_trips_with_awkward_text() {
        let mut answers = vec![None; options().len()];
        answers[0] = Some(Answer::Yes);
        answers[1] = Some(Answer::No);
        answers[2] = Some(Answer::Inconclusive);
        let sample = Sample {
            name: "a-b".into(),
            repository: "o/n".into(),
            commit: "abc".into(),
            path: "src/a.rs".into(),
            line: 3,
            end: 9,
            test: "it(\"says, \"\"hi\"\"\")".into(),
            label: "says, \"hi\"".into(),
            names: vec!["src/a.rs".into(), "src/b.rs".into()],
            drives: String::new(),
            files: vec![],
            status: Status::Pending,
            answers,
            notes: "two\nlines".into(),
        };
        let text = write(std::slice::from_ref(&sample));
        assert!(text.starts_with("name,repository,commit,path,line,end"));
        assert!(text.contains("artifact.code,artifact.config"), "{text}");
        assert!(text.contains("output_assertions.exact_match"), "{text}");
        assert_eq!(parse(&text).unwrap(), vec![sample]);
    }

    #[test]
    fn a_draw_stores_tests_with_where_they_came_from() {
        let parent = tempfile::tempdir().unwrap();
        let root = checkout(parent.path(), "alpha");
        // Not on GitHub: passed over, and said so.
        let other = parent.path().join("elsewhere");
        std::fs::create_dir(&other).unwrap();
        git(&other, &["init", "-q"]);
        let data = tempfile::tempdir().unwrap();
        let mut dataset = Dataset::open(data.path()).unwrap();
        let how = Draw {
            count: 10,
            seed: 7,
            per_repository: 10,
        };
        let drawn = draw(&mut dataset, &[parent.path().to_path_buf()], &how).unwrap();
        assert_eq!(drawn.repositories, 1);
        assert_eq!(drawn.candidates, 2);
        assert_eq!(drawn.added.len(), 2);
        assert!(
            drawn.passed_over[0].contains("not on GitHub"),
            "{:?}",
            drawn.passed_over
        );

        let dataset = Dataset::open(data.path()).unwrap();
        let parses = dataset.samples.iter().find(|s| s.test == "parses").unwrap();
        assert_eq!(parses.repository, "someone/alpha");
        let head = Repo::open(&root).unwrap().resolve("HEAD").unwrap();
        assert_eq!(parses.commit, head);
        assert!(parses.name.starts_with("someone-alpha-"), "{}", parses.name);
        assert_eq!(parses.names, ["src/lib.rs"]);
        let dir = data.path().join(&parses.name);
        let test = std::fs::read_to_string(dir.join("test.rs.test")).unwrap();
        assert!(test.starts_with("    fn parses() {"), "{test}");
        assert!(dir.join("file.rs.test").is_file());
        let readme = std::fs::read_to_string(dir.join("README.md")).unwrap();
        assert!(
            readme.contains("https://github.com/someone/alpha"),
            "{readme}"
        );
        assert!(readme.contains("`LICENSE` at this commit, beginning “MIT License”"));
        let docs = dataset
            .samples
            .iter()
            .find(|s| s.test == "the_readme_says_so")
            .unwrap();
        assert_eq!(docs.files, ["README.md"]);

        // The rules run again on the sample alone, the checkout gone.
        drop(root);
        std::fs::remove_dir_all(parent.path().join("alpha")).unwrap();
        let kind = dataset.replay(docs).unwrap();
        assert_eq!(kind.summary(Question::Artifact), "Documentation");
        assert_eq!(
            dataset
                .replay(parses)
                .unwrap()
                .summary(Question::Interaction),
            "API"
        );

        // A second draw adds nothing it already has.
        let mut dataset = Dataset::open(data.path()).unwrap();
        checkout(parent.path(), "alpha");
        let again = draw(&mut dataset, &[parent.path().to_path_buf()], &how).unwrap();
        assert_eq!(again.added.len(), 0);
        assert_eq!(dataset.samples.len(), 2);
    }

    #[test]
    fn the_same_seed_draws_the_same_tests() {
        let parent = tempfile::tempdir().unwrap();
        for name in ["alpha", "beta", "gamma"] {
            checkout(parent.path(), name);
        }
        let names = |seed: u64| {
            let data = tempfile::tempdir().unwrap();
            let mut dataset = Dataset::open(data.path()).unwrap();
            let how = Draw {
                count: 3,
                seed,
                per_repository: 1,
            };
            let drawn = draw(&mut dataset, &[parent.path().to_path_buf()], &how).unwrap();
            let mut repos: Vec<String> = dataset
                .samples
                .iter()
                .map(|s| s.repository.clone())
                .collect();
            repos.sort();
            assert_eq!(
                repos,
                ["someone/alpha", "someone/beta", "someone/gamma"],
                "one from each, the cap being one"
            );
            drawn
                .added
                .iter()
                .map(|n| n.rsplit_once('-').map_or(n.clone(), |(_, t)| t.to_string()))
                .collect::<Vec<_>>()
        };
        assert_eq!(names(1), names(1));
    }

    #[test]
    fn agreement_counts_where_a_person_and_the_rules_differ() {
        let parent = tempfile::tempdir().unwrap();
        checkout(parent.path(), "alpha");
        let data = tempfile::tempdir().unwrap();
        let mut dataset = Dataset::open(data.path()).unwrap();
        let how = Draw {
            count: 1,
            seed: 3,
            per_repository: 5,
        };
        draw(&mut dataset, &[parent.path().to_path_buf()], &how).unwrap();
        // The person answers as the rules do, except that they call it GUI as well.
        let kind = dataset.replay(&dataset.samples[0]).unwrap();
        let mut answers = Vec::new();
        for (q, o) in options() {
            let rules = kind
                .of(q)
                .options
                .iter()
                .find(|s| s.option == o)
                .unwrap()
                .answer;
            answers.push(Some(if (q, o) == (Question::Interaction, "GUI") {
                Answer::Yes
            } else {
                rules
            }));
        }
        dataset.samples[0].answers = answers;
        dataset.samples[0].status = Status::Done;
        let agreement = agreement(&dataset).unwrap();
        let interaction = agreement
            .iter()
            .find(|a| a.question == Question::Interaction)
            .unwrap();
        assert_eq!((interaction.agree, interaction.finished), (0, 1));
        assert_eq!(
            interaction.options,
            [("API".to_string(), 1), ("GUI".to_string(), 0)]
        );
        assert_eq!(interaction.differ[0].1, "Both API and GUI");
        let artifact = agreement
            .iter()
            .find(|a| a.question == Question::Artifact)
            .unwrap();
        assert_eq!((artifact.agree, artifact.finished), (1, 1));
    }
}
