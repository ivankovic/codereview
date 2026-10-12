//! The JSON API the page talks to: everything about a repository except signing in and the
//! agent, which have modules of their own. Every handler takes a [`Ctx`], the repository the
//! request named, and does its work on a blocking thread, since git and the diff engine are
//! not async.

use super::*;

// ----- state ---------------------------------------------------------------------------------

#[derive(Serialize)]
pub(super) struct RepoSummary {
    index: usize,
    name: String,
    root: String,
    branch: String,
    pending: usize,
}

/// Every served repository: in command-line order, then the ones that appeared later.
pub(super) async fn get_repos(State(state): State<AppState>) -> ApiResult<Vec<RepoSummary>> {
    let repos = state.repos.all();
    let list = tokio::task::spawn_blocking(move || {
        repos
            .iter()
            .enumerate()
            .map(|(index, r)| {
                let s = r
                    .session
                    .lock()
                    .map_err(|_| anyhow::anyhow!("session poisoned"))?;
                Ok(RepoSummary {
                    index,
                    name: s.name(),
                    root: s.root().display().to_string(),
                    branch: s.repo.head_label(),
                    pending: s.review.pending_count(),
                })
            })
            .collect::<Result<Vec<_>>>()
    })
    .await
    .map_err(|e| ApiError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))??;
    Ok(Json(list))
}

/// What a repository looks like when you open it. Costs a handful of git commands and a stat
/// per file, so it is asked for when that view is drawn.
pub(super) async fn get_overview(state: Ctx) -> ApiResult<crate::overview::Overview> {
    Ok(Json(with_session(&state, |s| s.overview()).await?))
}

#[derive(Deserialize)]
pub(super) struct LeansQuery {
    #[serde(default = "default_leaning")]
    most: usize,
}

fn default_leaning() -> usize {
    8
}

#[derive(Deserialize)]
pub(super) struct TestsQuery {
    #[serde(default = "default_leaning")]
    most: usize,
}

#[derive(Serialize)]
pub(super) struct TestsResponse {
    #[serde(flatten)]
    inventory: crate::testsuite::Inventory,
    as_of: Option<crate::session::AsOf>,
}

/// The tests the repository defines, grouped by file, and the files it leans on that no test
/// names. Needs the symbol index, like what it leans on, and answers from `OVERVIEW.md` the
/// same way while that is built: the tests then carry no line numbers.
pub(super) async fn get_tests(state: Ctx, Query(q): Query<TestsQuery>) -> ApiResult<TestsResponse> {
    let most = q.most.clamp(1, 50);
    let warming = state.warming.load(std::sync::atomic::Ordering::Relaxed);
    Ok(Json(
        with_session(&state, move |s| {
            if let Some(as_of) = s.as_of().filter(|_| warming || s.facts_file.is_some()) {
                let inventory = match &s.facts_file {
                    Some(facts) => crate::testsuite::Inventory::from_facts(facts, most),
                    None => crate::testsuite::Inventory::of(Vec::new(), Vec::new()),
                };
                return Ok(TestsResponse {
                    inventory,
                    as_of: Some(as_of),
                });
            }
            Ok(TestsResponse {
                inventory: s.tests(most),
                as_of: None,
            })
        })
        .await?,
    ))
}

#[derive(Serialize)]
pub(super) struct EndToEndResponse {
    #[serde(flatten)]
    opening: crate::e2e::Opening,
    as_of: Option<crate::session::AsOf>,
}

/// What a repository opens on: its end-to-end tests, each with the recording its last run
/// left. Needs the tests, so while the symbol index is built it answers from `OVERVIEW.md`
/// like the other test views; the recordings are read from disk either way.
pub(super) async fn get_e2e(state: Ctx) -> ApiResult<EndToEndResponse> {
    let warming = state.warming.load(std::sync::atomic::Ordering::Relaxed);
    Ok(Json(
        with_session(&state, move |s| {
            if let Some(as_of) = s.as_of().filter(|_| warming || s.facts_file.is_some()) {
                let tests = match &s.facts_file {
                    Some(facts) => crate::testsuite::Inventory::from_facts(facts, 0)
                        .files
                        .into_iter()
                        .flat_map(|f| f.tests)
                        .collect(),
                    None => Vec::new(),
                };
                return Ok(EndToEndResponse {
                    opening: crate::e2e::of(s, tests),
                    as_of: Some(as_of),
                });
            }
            Ok(EndToEndResponse {
                opening: s.end_to_end(),
                as_of: None,
            })
        })
        .await?,
    ))
}

#[derive(Deserialize)]
pub(super) struct TestQuery {
    file: String,
    name: String,
}

/// One end-to-end test as its own view shows it: the code it ran, the documentation it
/// demonstrates and the smaller tests below it. Needs the symbol index, and while the server
/// is still building it says so rather than building a second one; the page asks again.
pub(super) async fn get_e2e_test(
    state: Ctx,
    Query(q): Query<TestQuery>,
) -> ApiResult<crate::e2e::Detail> {
    if state.warming.load(std::sync::atomic::Ordering::Relaxed) {
        return Err(ApiError(
            StatusCode::SERVICE_UNAVAILABLE,
            "still reading the tests".into(),
        ));
    }
    let detail = with_session(&state, move |s| Ok(crate::e2e::detail(s, &q.file, &q.name))).await?;
    detail
        .map(Json)
        .ok_or_else(|| ApiError(StatusCode::NOT_FOUND, "no such end-to-end test".into()))
}

/// What a stretch of history did, through the end-to-end tests: what happened to the tests,
/// the changed code none of them ran, and each test before and after once the base is
/// recorded. Needs the symbol index, like a test's own view.
pub(super) async fn get_stretch(
    state: Ctx,
    Query(q): Query<SpecQuery>,
) -> ApiResult<crate::stretch::Report> {
    let stretch = stretch_of(&q)?;
    if state.warming.load(std::sync::atomic::Ordering::Relaxed) {
        return Err(ApiError(
            StatusCode::SERVICE_UNAVAILABLE,
            "still reading the tests".into(),
        ));
    }
    Ok(Json(
        with_session(&state, move |s| crate::stretch::report(s, &stretch)).await?,
    ))
}

#[derive(Deserialize)]
pub(super) struct StretchTestQuery {
    #[serde(flatten)]
    stretch: SpecQuery,
    file: String,
    name: String,
}

/// One end-to-end test before and after a stretch, step by step.
pub(super) async fn get_stretch_test(
    state: Ctx,
    Query(q): Query<StretchTestQuery>,
) -> ApiResult<crate::stretch::Comparison> {
    let stretch = stretch_of(&q.stretch)?;
    Ok(Json(
        with_session(&state, move |s| {
            crate::stretch::compare(s, &stretch, &q.file, &q.name)
        })
        .await?,
    ))
}

#[derive(Deserialize)]
pub(super) struct RecordBase {
    commit: String,
}

/// The name a recording at `commit` runs under, among the checks' runs.
fn base_run(commit: &str) -> String {
    format!("end-to-end tests at {commit}")
}

/// Records the end-to-end tests as they were at a commit: a worktree of it, and the
/// repository's end-to-end command run there. What it leaves is the worktree's recordings;
/// while it runs, its progress is a run like a check's.
pub(super) async fn post_stretch_record(
    state: Ctx,
    Json(body): Json<RecordBase>,
) -> ApiResult<crate::analysis::Run> {
    let checks = state.checks.clone();
    let run = with_session(&state, move |s| {
        let commit = s.repo.resolve(&body.commit)?;
        let name = base_run(&commit);
        let mut runs = checks.lock().unwrap_or_else(|e| e.into_inner());
        if runs
            .get(&name)
            .is_some_and(|r| r.lock().unwrap_or_else(|e| e.into_inner()).running)
        {
            anyhow::bail!(Conflict(format!(
                "the tests at {commit} are already being recorded"
            )));
        }
        let (worktree, mut check) = crate::stretch::prepare(s, &commit)?;
        check.name = name.clone();
        let started = crate::analysis::start(&check, &worktree)?;
        runs.insert(name, started.clone());
        let run = started.lock().unwrap_or_else(|e| e.into_inner()).clone();
        Ok(run)
    })
    .await?;
    Ok(Json(run))
}

/// How the recording at a commit is going, or went: `null` when none was started here.
pub(super) async fn get_stretch_record(
    state: Ctx,
    Query(q): Query<RecordBase>,
) -> ApiResult<Option<crate::analysis::Run>> {
    let checks = state.checks.clone();
    Ok(Json(
        with_session(&state, move |s| {
            let commit = s.repo.resolve(&q.commit)?;
            let runs = checks.lock().unwrap_or_else(|e| e.into_inner());
            Ok(runs
                .get(&base_run(&commit))
                .map(|r| r.lock().unwrap_or_else(|e| e.into_inner()).clone()))
        })
        .await?,
    ))
}

/// The recording `path` names, when it is one a test run left: the path is checked against
/// what was found, never opened because a request named it.
fn recording_file(s: &Session, path: &str, at: Option<&str>) -> Result<std::path::PathBuf> {
    let root = match at {
        Some(commit) => {
            if commit.len() != 40 || !commit.chars().all(|c| c.is_ascii_hexdigit()) {
                anyhow::bail!("{commit:?} is not a full commit hash");
            }
            crate::stretch::worktree_path(s, commit)?
        }
        None => s.root().to_path_buf(),
    };
    let found = crate::recording::find(&root)
        .into_iter()
        .any(|r| r.media().as_deref() == Some(path));
    if !found {
        anyhow::bail!("{path} is not a recording a test left");
    }
    Ok(root.join(path))
}

/// A recording larger than this is not served: a test's video is a few megabytes.
const MOST_RECORDING_BYTES: u64 = 256 << 20;

/// A recording's bytes, with its media type. Ranges are honoured, since that is how a video
/// element seeks.
pub(super) async fn get_recording(
    state: Ctx,
    headers: axum::http::HeaderMap,
    Query(q): Query<RecordingQuery>,
) -> Result<Response, ApiError> {
    let Some(kind) = crate::recording::media_type(&q.path) else {
        return Err(ApiError(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            format!("{} is not a recording", q.path),
        ));
    };
    let path = q.path.clone();
    let at = q.at.clone();
    let bytes = with_session(&state, move |s| {
        let file = recording_file(s, &path, at.as_deref())?;
        let size = std::fs::metadata(&file)?.len();
        if size > MOST_RECORDING_BYTES {
            anyhow::bail!("{path} is over {} MB", MOST_RECORDING_BYTES >> 20);
        }
        Ok(std::fs::read(file)?)
    })
    .await
    .map_err(|e| ApiError(StatusCode::NOT_FOUND, e.1))?;
    let total = bytes.len();
    let range = headers
        .get(header::RANGE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| byte_range(v, total));
    let common = [
        (header::CONTENT_TYPE, kind.to_string()),
        (header::ACCEPT_RANGES, "bytes".to_string()),
        (
            header::CONTENT_SECURITY_POLICY,
            "default-src 'none'".to_string(),
        ),
    ];
    Ok(match range {
        Some((start, end)) => (
            StatusCode::PARTIAL_CONTENT,
            common,
            [(
                header::CONTENT_RANGE,
                format!("bytes {start}-{end}/{total}"),
            )],
            bytes[start..=end].to_vec(),
        )
            .into_response(),
        None => (common, bytes).into_response(),
    })
}

/// The first range of a `Range: bytes=...` header, inclusive, within `total` bytes. Anything
/// this cannot serve is answered with the whole file, which the header allows.
fn byte_range(header: &str, total: usize) -> Option<(usize, usize)> {
    let spec = header.strip_prefix("bytes=")?.split(',').next()?.trim();
    let (start, end) = spec.split_once('-')?;
    let last = total.checked_sub(1)?;
    let (start, end) = match (start.trim(), end.trim()) {
        ("", n) => {
            let n: usize = n.parse().ok()?;
            (total.saturating_sub(n), last)
        }
        (a, "") => (a.parse().ok()?, last),
        (a, b) => (a.parse().ok()?, b.parse::<usize>().ok()?.min(last)),
    };
    (start <= end && end <= last).then_some((start, end))
}

/// A terminal recording played into frames of styled text, for the page to draw.
pub(super) async fn get_cast(
    state: Ctx,
    Query(q): Query<RecordingQuery>,
) -> ApiResult<crate::recording::Cast> {
    Ok(Json(
        with_session(&state, move |s| {
            if crate::recording::kind_of(&q.path) != Some(crate::recording::Kind::Terminal) {
                anyhow::bail!("{} is not a terminal recording", q.path);
            }
            let file = recording_file(s, &q.path, q.at.as_deref())?;
            crate::recording::frames(&std::fs::read_to_string(file)?)
        })
        .await?,
    ))
}

#[cfg(test)]
#[test]
fn ranges_are_read_as_a_video_element_asks() {
    assert_eq!(byte_range("bytes=0-", 10), Some((0, 9)));
    assert_eq!(byte_range("bytes=2-4", 10), Some((2, 4)));
    assert_eq!(byte_range("bytes=5-100", 10), Some((5, 9)));
    assert_eq!(byte_range("bytes=-3", 10), Some((7, 9)));
    assert_eq!(byte_range("bytes=9-2", 10), None);
    assert_eq!(byte_range("bytes=10-", 10), None);
    assert_eq!(byte_range("lines=1-2", 10), None);
    assert_eq!(byte_range("bytes=0-", 0), None);
}

#[derive(Deserialize)]
pub(super) struct SpecQuery {
    /// `week` (the default), `last` or `uncommitted`.
    #[serde(default)]
    stretch: Option<String>,
    /// One commit, against its parent: the drill-down from the log. Wins over `stretch`.
    #[serde(default)]
    commit: Option<String>,
    /// The question to group the tests by; `interaction` by default.
    #[serde(default)]
    by: Option<crate::classify::Question>,
}

/// The stretch of history a query names: one commit, or `week` (the default), `last` or
/// `uncommitted`.
fn stretch_of(q: &SpecQuery) -> Result<crate::spec::Stretch, ApiError> {
    use crate::spec::Stretch;
    Ok(match (q.commit.as_deref(), q.stretch.as_deref()) {
        (Some(hash), _) if !hash.is_empty() => Stretch::Commit(hash.to_string()),
        (_, None | Some("" | "week")) => Stretch::Week,
        (_, Some("last")) => Stretch::LastCommit,
        (_, Some("uncommitted")) => Stretch::Uncommitted,
        (_, Some(other)) => {
            return Err(ApiError(
                StatusCode::BAD_REQUEST,
                format!("no stretch called {other:?}; week, last or uncommitted"),
            ));
        }
    })
}

#[derive(Serialize)]
pub(super) struct SpecResponse {
    #[serde(flatten)]
    spec: crate::spec::Spec,
    /// Set when the tests came from `OVERVIEW.md` because the index is still being built;
    /// what the stretch did to them never does, since that is read from the files it touched.
    as_of: Option<crate::session::AsOf>,
}

/// The tests as a specification: by level, with what a stretch of history did to them. While
/// the index is built the list comes from `OVERVIEW.md`, the same way the overview's does.
pub(super) async fn get_spec(state: Ctx, Query(q): Query<SpecQuery>) -> ApiResult<SpecResponse> {
    let stretch = stretch_of(&q)?;
    let by = q.by.unwrap_or(crate::classify::Question::Interaction);
    let warming = state.warming.load(std::sync::atomic::Ordering::Relaxed);
    Ok(Json(
        with_session(&state, move |s| {
            if let Some(as_of) = s.as_of().filter(|_| warming || s.facts_file.is_some()) {
                let tests = match &s.facts_file {
                    Some(facts) => crate::testsuite::Inventory::from_facts(facts, 0)
                        .files
                        .into_iter()
                        .flat_map(|f| f.tests)
                        .collect(),
                    None => Vec::new(),
                };
                return Ok(SpecResponse {
                    spec: crate::spec::of(s, tests, None, &stretch, by)?,
                    as_of: Some(as_of),
                });
            }
            Ok(SpecResponse {
                spec: s.spec(&stretch, by)?,
                as_of: None,
            })
        })
        .await?,
    ))
}

/// The tests that name something the file defines, or that it holds: what a reader of a
/// file asks before trusting it. Needs the symbol index, and builds it if it must.
pub(super) async fn get_tests_of(
    state: Ctx,
    Query(q): Query<PathQuery>,
) -> ApiResult<Vec<crate::spec::SpecTest>> {
    Ok(Json(
        with_session(&state, move |s| Ok(s.tests_reaching(&q.path))).await?,
    ))
}

#[derive(Serialize)]
pub(super) struct LeansResponse {
    leaning: Vec<crate::symbols::Reach>,
    /// Set when this came from `OVERVIEW.md` because the index is still being built: the
    /// page says so and asks again.
    as_of: Option<crate::session::AsOf>,
}

/// Which files the rest of the tree leans on. Separate from the overview because it needs the
/// symbol index, which is seconds of parsing on a large repository. While the server's own
/// thread is still building that index, or `OVERVIEW.md` has an answer, the file's answer is
/// given, marked as of its commit — empty, and marked, when there is no file yet — and the
/// page asks again. Only with neither is the index built here and now, under the lock.
pub(super) async fn get_leans_on(
    state: Ctx,
    Query(q): Query<LeansQuery>,
) -> ApiResult<LeansResponse> {
    let most = q.most.clamp(1, 50);
    let warming = state.warming.load(std::sync::atomic::Ordering::Relaxed);
    Ok(Json(
        with_session(&state, move |s| {
            if let Some(as_of) = s.as_of().filter(|_| warming || s.facts_file.is_some()) {
                let leaning = s
                    .facts_file
                    .iter()
                    .flat_map(|f| f.leans_on.iter())
                    .take(most)
                    .map(|(path, names)| crate::symbols::Reach {
                        path: path.clone(),
                        score: 0.0,
                        names: names.clone(),
                    })
                    .collect();
                return Ok(LeansResponse {
                    leaning,
                    as_of: Some(as_of),
                });
            }
            Ok(LeansResponse {
                leaning: s.leans_on(most),
                as_of: None,
            })
        })
        .await?,
    ))
}

#[derive(Serialize)]
pub(super) struct StateResponse {
    name: String,
    root: String,
    branch: String,
    files: Vec<String>,
    /// path -> status letter
    status: Vec<(String, char)>,
    pending: usize,
    author: Option<String>,
    has_commits: bool,
    theme: crate::theme::Theme,
    /// Every scheme the page can offer: the RGB ones. The terminal scheme has no meaning in
    /// a browser; when it is configured the page falls back to Light or Dark by itself.
    themes: Vec<String>,
    layout: String,
}

pub(super) async fn get_state(state: Ctx) -> ApiResult<StateResponse> {
    let r = with_session(&state, move |s| {
        let status = s
            .files
            .iter()
            .filter_map(|p| s.status_letter(p).map(|c| (p.clone(), c)))
            .collect();
        Ok(StateResponse {
            name: s.name(),
            root: s.root().display().to_string(),
            branch: s.repo.head_label(),
            files: s.files.clone(),
            status,
            pending: s.review.pending_count(),
            author: s.author.clone(),
            has_commits: s.repo.has_commits(),
            theme: s.theme.clone(),
            themes: crate::theme::all()
                .into_iter()
                .filter(|t| !t.terminal)
                .map(|t| t.name)
                .collect(),
            layout: s.config.layout.clone(),
        })
    })
    .await?;
    Ok(Json(r))
}

// ----- files ---------------------------------------------------------------------------------

#[derive(Deserialize)]
pub(super) struct FileQuery {
    path: String,
    /// A revision to show the file at; empty or absent means the working tree.
    rev: Option<String>,
}

/// Highlighted lines for the wire: the distinct styles once, in a table, and per line a
/// list of `[style, text]` pairs with adjacent runs of one style joined. One object per
/// segment made a file of 1600 lines a megabyte of JSON, six times what this takes, and the
/// page spent its time parsing it; the page turns these back into segments on arrival.
#[derive(Serialize, Default)]
pub(super) struct Packed {
    styles: Vec<PackedStyle>,
    lines: Vec<Vec<(usize, String)>>,
}

#[derive(Serialize, Clone, PartialEq, Eq, Hash)]
struct PackedStyle {
    fg: Option<(u8, u8, u8)>,
    #[serde(skip_serializing_if = "Option::is_none")]
    ansi: Option<u8>,
    bold: bool,
    italic: bool,
}

impl Packed {
    /// Packs `lines` into this, sharing the style table with whatever is already packed.
    fn add(&mut self, lines: &[Vec<Segment>]) -> Vec<Vec<(usize, String)>> {
        let mut index: std::collections::HashMap<PackedStyle, usize> = self
            .styles
            .iter()
            .cloned()
            .enumerate()
            .map(|(i, s)| (s, i))
            .collect();
        lines
            .iter()
            .map(|line| {
                let mut out: Vec<(usize, String)> = Vec::with_capacity(line.len());
                for seg in line {
                    let style = PackedStyle {
                        fg: seg.fg,
                        ansi: seg.ansi,
                        bold: seg.bold,
                        italic: seg.italic,
                    };
                    let i = *index.entry(style.clone()).or_insert_with(|| {
                        self.styles.push(style);
                        self.styles.len() - 1
                    });
                    match out.last_mut() {
                        Some((last, text)) if *last == i => text.push_str(&seg.text),
                        _ => out.push((i, seg.text.clone())),
                    }
                }
                out
            })
            .collect()
    }

    fn of(lines: &[Vec<Segment>]) -> Packed {
        let mut packed = Packed::default();
        packed.lines = packed.add(lines);
        packed
    }
}

#[derive(Serialize)]
pub(super) struct FileResponse {
    #[serde(flatten)]
    view: FileView,
    #[serde(flatten)]
    lines: Packed,
    /// Per-line change against HEAD or the index, when the file is modified.
    line_ops: Option<Vec<Op>>,
}

pub(super) async fn get_file(state: Ctx, Query(q): Query<FileQuery>) -> ApiResult<FileResponse> {
    let r = with_session(&state, move |s| {
        let view = match q.rev.as_deref().filter(|r| !r.is_empty()) {
            Some(rev) => s.file_view_at(rev, &q.path)?,
            None => s.file_view(&q.path)?,
        };
        let lines = Packed::of(&crate::highlight::highlight(
            &view.path,
            &view.text,
            &web_syntax(s),
        ));
        let mut line_ops = None;
        if q.rev.as_deref().unwrap_or("").is_empty()
            && let Some(entry) = s.status.get(&q.path)
            && let Some(status) = entry.unstaged.or(entry.staged)
        {
            let file = ChangedFile {
                status,
                path: q.path.clone(),
                old_path: entry.old_path.clone(),
            };
            let target = if entry.unstaged.is_some() {
                DiffTarget::Working
            } else {
                DiffTarget::Staged
            };
            if let Ok(d) = s.diff(&target, &file)
                && d.after.line_count() == lines.lines.len()
            {
                line_ops = Some(d.after.line_ops);
            }
        }
        Ok(FileResponse {
            view,
            lines,
            line_ops,
        })
    })
    .await?;
    Ok(Json(r))
}

#[derive(Deserialize)]
pub(super) struct PathQuery {
    path: String,
}

#[derive(Deserialize)]
pub(super) struct RecordingQuery {
    path: String,
    /// A commit the recording was made at, in its worktree, rather than the working tree.
    #[serde(default)]
    at: Option<String>,
}

/// A picture in the working tree, as its bytes with its media type, for the page to show. Only
/// what `media::image_type` names a picture is served; anything else is not this endpoint's
/// to hand out. The page fetches it with the API token like any other request, so the bytes
/// come back to a script, which shows them from memory: an `<img>` pointing here could not
/// send the token. An SVG is served with a policy that lets it run nothing, should it ever be
/// opened on its own.
pub(super) async fn get_image(
    state: Ctx,
    Query(q): Query<PathQuery>,
) -> Result<Response, ApiError> {
    let Some(kind) = crate::media::image_type(&q.path) else {
        return Err(ApiError(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            format!("{} is not a picture", q.path),
        ));
    };
    let path = q.path.clone();
    let bytes = with_session(&state, move |s| s.repo.read_working(&path)).await?;
    let Some(bytes) = bytes else {
        return Err(ApiError(StatusCode::NOT_FOUND, format!("no {}", q.path)));
    };
    Ok((
        [
            (header::CONTENT_TYPE, kind),
            (
                header::CONTENT_SECURITY_POLICY,
                "default-src 'none'; style-src 'unsafe-inline'",
            ),
        ],
        bytes,
    )
        .into_response())
}

pub(super) async fn get_blame(state: Ctx, Query(q): Query<PathQuery>) -> ApiResult<Vec<BlameLine>> {
    Ok(Json(
        with_session(&state, move |s| s.repo.blame(&q.path)).await?,
    ))
}

// ----- history -------------------------------------------------------------------------------

#[derive(Deserialize)]
pub(super) struct LogQuery {
    path: Option<String>,
    #[serde(default)]
    skip: usize,
    #[serde(default = "default_limit")]
    limit: usize,
}

fn default_limit() -> usize {
    100
}

/// The most commits one request may ask for. Without a cap, `limit` reaches git's
/// `--max-count` and the answer is the entire history, held in memory twice.
const MAX_LOG_PAGE: usize = 1000;

pub(super) async fn get_log(state: Ctx, Query(q): Query<LogQuery>) -> ApiResult<Vec<Commit>> {
    let limit = q.limit.clamp(1, MAX_LOG_PAGE);
    let r = with_session(&state, move |s| match &q.path {
        Some(p) if !p.is_empty() => s.file_log(p, q.skip, limit),
        _ => s.log(q.skip, limit),
    })
    .await?;
    Ok(Json(r))
}

#[derive(Deserialize)]
pub(super) struct HashQuery {
    hash: String,
}

pub(super) async fn get_commit_files(
    state: Ctx,
    Query(q): Query<HashQuery>,
) -> ApiResult<Vec<ChangedFile>> {
    Ok(Json(
        with_session(&state, move |s| s.commit_files(&q.hash)).await?,
    ))
}

#[derive(Deserialize)]
pub(super) struct ChangesQuery {
    #[serde(default)]
    staged: bool,
}

pub(super) async fn get_changes(
    state: Ctx,
    Query(q): Query<ChangesQuery>,
) -> ApiResult<Vec<ChangedFile>> {
    let target = if q.staged {
        DiffTarget::Staged
    } else {
        DiffTarget::Working
    };
    Ok(Json(
        with_session(&state, move |s| s.changed_files(&target)).await?,
    ))
}

// ----- diff ----------------------------------------------------------------------------------

#[derive(Deserialize)]
pub(super) struct DiffQuery {
    /// `working`, `staged`, `commit`, or `revisions`.
    kind: String,
    hash: Option<String>,
    from: Option<String>,
    to: Option<String>,
    path: String,
    old_path: Option<String>,
    status: Option<char>,
}

#[derive(Serialize)]
pub(super) struct DiffResponse {
    #[serde(flatten)]
    diff: FileDiff,
    /// The styles of both sides' lines, packed as a file's are.
    styles: Vec<PackedStyle>,
    before_lines: Vec<Vec<(usize, String)>>,
    after_lines: Vec<Vec<(usize, String)>>,
    comments: Vec<Anchored>,
    target: String,
    rows: Vec<crate::diff::AlignedRow>,
}

pub(super) async fn get_diff(state: Ctx, Query(q): Query<DiffQuery>) -> ApiResult<DiffResponse> {
    let target = match q.kind.as_str() {
        "working" => DiffTarget::Working,
        "staged" => DiffTarget::Staged,
        "commit" => DiffTarget::Commit {
            hash: q
                .hash
                .clone()
                .ok_or_else(|| ApiError(StatusCode::BAD_REQUEST, "hash required".into()))?,
        },
        "revisions" => DiffTarget::Revisions {
            from: q.from.clone().unwrap_or_default(),
            to: q.to.clone().unwrap_or_default(),
        },
        other => {
            return Err(ApiError(
                StatusCode::BAD_REQUEST,
                format!("unknown diff kind {other}"),
            ));
        }
    };
    let r = with_session(&state, move |s| {
        let file = ChangedFile {
            status: q
                .status
                .and_then(FileStatus::from_letter)
                .unwrap_or(FileStatus::Modified),
            path: q.path.clone(),
            old_path: q.old_path.clone().filter(|p| !p.is_empty()),
        };
        let diff = s.diff(&target, &file)?;
        let old_path = file.old_path.clone().unwrap_or_else(|| file.path.clone());
        let theme = web_syntax(s);
        let mut packed = Packed::default();
        let before_lines = packed.add(&crate::highlight::highlight(
            &old_path,
            &diff.before.text,
            &theme,
        ));
        let after_lines = packed.add(&crate::highlight::highlight(
            &file.path,
            &diff.after.text,
            &theme,
        ));
        let comments = s.anchor_in(&file.path, &diff.after.text);
        Ok(DiffResponse {
            target: target.label(),
            rows: diff.aligned_rows(),
            diff,
            styles: packed.styles,
            before_lines,
            after_lines,
            comments,
        })
    })
    .await?;
    Ok(Json(r))
}

// ----- comments ------------------------------------------------------------------------------

pub(super) async fn get_comments(state: Ctx) -> ApiResult<Vec<Anchored>> {
    Ok(Json(with_session(&state, |s| Ok(s.all_anchored())).await?))
}

#[derive(Deserialize)]
pub(super) struct NewComment {
    path: String,
    /// Left out for a comment on the whole file or directory.
    line: Option<usize>,
    end_line: Option<usize>,
    text: String,
    source_line: Option<String>,
    /// The commit the line numbers count in, when they were read off a commit's diff.
    at: Option<String>,
}

pub(super) async fn post_comment(state: Ctx, Json(body): Json<NewComment>) -> ApiResult<Comment> {
    if body.text.trim().is_empty() || body.line == Some(0) {
        return Err(ApiError(
            StatusCode::BAD_REQUEST,
            "text required, and line must be 1 or more when given".into(),
        ));
    }
    let lines = body
        .line
        .map(|line| (line, body.end_line.unwrap_or(line).max(line)));
    let r = with_session(&state, move |s| {
        s.add_comment(
            &body.path,
            lines,
            &body.text,
            body.source_line.as_deref(),
            body.at.as_deref(),
        )
    })
    .await?;
    Ok(Json(r))
}

#[derive(Deserialize)]
pub(super) struct CommentRef {
    comment: Comment,
    text: Option<String>,
}

pub(super) async fn post_comment_toggle(
    state: Ctx,
    Json(body): Json<CommentRef>,
) -> ApiResult<Ok_> {
    with_session(&state, move |s| s.toggle_comment(&body.comment)).await?;
    Ok(Json(Ok_ { ok: true }))
}

pub(super) async fn post_comment_edit(state: Ctx, Json(body): Json<CommentRef>) -> ApiResult<Ok_> {
    let text = body.text.clone().unwrap_or_default();
    if text.trim().is_empty() {
        return Err(ApiError(StatusCode::BAD_REQUEST, "text required".into()));
    }
    with_session(&state, move |s| s.edit_comment(&body.comment, &text)).await?;
    Ok(Json(Ok_ { ok: true }))
}

pub(super) async fn post_comment_delete(
    state: Ctx,
    Json(body): Json<CommentRef>,
) -> ApiResult<Ok_> {
    with_session(&state, move |s| s.delete_comment(&body.comment)).await?;
    Ok(Json(Ok_ { ok: true }))
}

// ----- notes ---------------------------------------------------------------------------------

pub(super) async fn get_notes(state: Ctx) -> ApiResult<Vec<Note>> {
    Ok(Json(
        with_session(&state, |s| {
            Ok(s.notes.notes().into_iter().cloned().collect())
        })
        .await?,
    ))
}

#[derive(Deserialize)]
pub(super) struct NewNote {
    path: Option<String>,
    text: String,
}

pub(super) async fn post_note(state: Ctx, Json(body): Json<NewNote>) -> ApiResult<Note> {
    if body.text.trim().is_empty() {
        return Err(ApiError(StatusCode::BAD_REQUEST, "text required".into()));
    }
    let r = with_session(&state, move |s| {
        s.add_note(body.path.as_deref().filter(|p| !p.is_empty()), &body.text)
    })
    .await?;
    Ok(Json(r))
}

#[derive(Deserialize)]
pub(super) struct NoteRef {
    note: Note,
    text: Option<String>,
}

pub(super) async fn post_note_edit(state: Ctx, Json(body): Json<NoteRef>) -> ApiResult<Ok_> {
    let text = body.text.clone().unwrap_or_default();
    if text.trim().is_empty() {
        return Err(ApiError(StatusCode::BAD_REQUEST, "text required".into()));
    }
    with_session(&state, move |s| s.edit_note(&body.note, &text)).await?;
    Ok(Json(Ok_ { ok: true }))
}

pub(super) async fn post_note_delete(state: Ctx, Json(body): Json<NoteRef>) -> ApiResult<Ok_> {
    with_session(&state, move |s| s.delete_note(&body.note)).await?;
    Ok(Json(Ok_ { ok: true }))
}

/// The syntect theme the page highlights with. The terminal scheme's `ansi` theme yields
/// palette indices a browser cannot show, so the page gets the Dark scheme's colours instead
/// (the page itself picks Light or Dark chrome by the browser's preference in that case).
fn web_syntax(s: &Session) -> String {
    if s.theme.terminal {
        crate::theme::Theme::named("Dark").expect("built in").syntax
    } else {
        s.theme.syntax.clone()
    }
}

pub(super) async fn get_themes() -> Json<Vec<crate::theme::Theme>> {
    Json(
        crate::theme::all()
            .into_iter()
            .filter(|t| !t.terminal)
            .collect(),
    )
}

#[derive(Deserialize)]
pub(super) struct ConfigChange {
    theme: Option<String>,
    layout: Option<String>,
}

pub(super) async fn post_config(state: Ctx, Json(body): Json<ConfigChange>) -> ApiResult<Ok_> {
    with_session(&state, move |s| {
        if let Some(theme) = &body.theme {
            s.set_theme(theme)?;
        }
        if let Some(layout) = &body.layout {
            s.set_layout(layout)?;
        }
        Ok(())
    })
    .await?;
    Ok(Json(Ok_ { ok: true }))
}

// ----- symbols -------------------------------------------------------------------------------

pub(super) async fn get_symbols(state: Ctx, Query(q): Query<PathQuery>) -> ApiResult<Vec<Symbol>> {
    Ok(Json(
        with_session(&state, move |s| Ok(s.symbols_in(&q.path))).await?,
    ))
}

#[derive(Deserialize)]
pub(super) struct NameQuery {
    name: String,
}

pub(super) async fn get_definitions(
    state: Ctx,
    Query(q): Query<NameQuery>,
) -> ApiResult<Vec<Symbol>> {
    Ok(Json(
        with_session(&state, move |s| Ok(s.definitions(&q.name))).await?,
    ))
}

/// The most matches one search answers with; a one-letter query otherwise returns the
/// whole index.
const MAX_HITS: usize = 500;

pub(super) async fn get_references(
    state: Ctx,
    Query(q): Query<NameQuery>,
) -> ApiResult<Vec<Location>> {
    Ok(Json(
        with_session(&state, move |s| {
            let mut hits = s.references(&q.name);
            hits.truncate(MAX_HITS);
            Ok(hits)
        })
        .await?,
    ))
}

#[derive(Deserialize)]
pub(super) struct SearchQuery {
    q: String,
}

pub(super) async fn get_symbol_search(
    state: Ctx,
    Query(q): Query<SearchQuery>,
) -> ApiResult<Vec<Symbol>> {
    Ok(Json(
        with_session(&state, move |s| {
            let mut hits = s.search_symbols(&q.q);
            hits.truncate(MAX_HITS);
            Ok(hits)
        })
        .await?,
    ))
}

#[derive(Deserialize)]
pub(super) struct IdentifierQuery {
    path: String,
    /// 0-based row.
    line: usize,
    /// 0-based byte column.
    col: usize,
}

#[derive(Serialize)]
pub(super) struct IdentifierResponse {
    name: Option<String>,
}

pub(super) async fn get_identifier(
    state: Ctx,
    Query(q): Query<IdentifierQuery>,
) -> ApiResult<IdentifierResponse> {
    let name = with_session(&state, move |s| Ok(s.identifier_at(&q.path, q.line, q.col))).await?;
    Ok(Json(IdentifierResponse { name }))
}

// ----- analysis ------------------------------------------------------------------------------

#[derive(Serialize)]
pub(super) struct CheckStatus {
    name: String,
    /// What it runs, so the page can say what a button is about to do.
    run: String,
    /// Whether this repository is one where it may also run unasked.
    watched: bool,
    /// Found in the repository rather than configured: running it saves it first.
    suggested: bool,
    /// The last run in this server's lifetime, in flight or finished; `None` if it has not
    /// been run since the server started, which is not the same as never having run.
    last: Option<CheckRun>,
    /// What its artefact says, read now, which survives a restart because the file does; or,
    /// with no artefact, what the last run printed at the end.
    figures: Option<Figures>,
    /// How far the code has moved since the result shown: the last finished run, or else the
    /// artefact. `None` when there is no result yet.
    stale: Option<Staleness>,
}

/// The checks of the repository at `root`: the configured ones that belong to it, then the
/// ones found in it that nothing configured already runs or is called. The second flag says
/// which is which.
fn checks_of(s: &Session, root: &std::path::Path) -> Vec<(Check, bool)> {
    let configured: Vec<Check> = s
        .config
        .analysis
        .iter()
        .filter(|c| crate::analysis::applies(c, root))
        .cloned()
        .collect();
    let found: Vec<Check> = crate::analysis::detect(root)
        .into_iter()
        .filter(|d| {
            !configured
                .iter()
                .any(|c| c.run == d.run || c.name == d.name)
        })
        .collect();
    configured
        .into_iter()
        .map(|c| (c, false))
        .chain(found.into_iter().map(|c| (c, true)))
        .collect()
}

/// Every check of this repository, with whatever is known about it: what the last run did,
/// what the artefact on disk says, and how much the code has changed since. The first two are
/// independent — a figure can be there with no run behind it, left by a `make check` in a
/// terminal.
pub(super) async fn get_analysis(state: Ctx) -> ApiResult<Vec<CheckStatus>> {
    let checks = state.checks.clone();
    let list = with_session(&state, move |s| {
        let root = s.root().to_path_buf();
        let runs = checks.lock().unwrap_or_else(|e| e.into_inner());
        Ok(checks_of(s, &root)
            .into_iter()
            .map(|(c, suggested)| {
                let last = runs
                    .get(&c.name)
                    .map(|r| r.lock().unwrap_or_else(|e| e.into_inner()).clone());
                let finished = last.as_ref().filter(|r| !r.running);
                let figures = crate::analysis::figures(&c, &root).or_else(|| {
                    let run = finished?;
                    crate::analysis::figures_from_output(&run.output).map(|f| Figures {
                        as_of: Some(run.started.clone()),
                        ..f
                    })
                });
                let since = finished
                    .map(|r| r.started.clone())
                    .or_else(|| figures.as_ref().and_then(|f| f.as_of.clone()));
                let stale = since.and_then(|t| s.changed_since(&t, c.reads.as_deref()).ok());
                CheckStatus {
                    watched: crate::analysis::watches(&c, &root),
                    name: c.name,
                    run: c.run,
                    suggested,
                    last,
                    figures,
                    stale,
                }
            })
            .collect())
    })
    .await?;
    Ok(Json(list))
}

#[derive(Deserialize)]
pub(super) struct RunCheck {
    name: String,
}

/// Starts one check. Returns at once: what it is doing arrives through `/analysis`. A check
/// the repository suggested is saved to the configuration first, for this repository, so
/// that pressing Run once is how a found check becomes a configured one.
pub(super) async fn post_analysis_run(
    state: Ctx,
    Json(body): Json<RunCheck>,
) -> ApiResult<CheckRun> {
    let checks = state.checks.clone();
    let run = with_session(&state, move |s| {
        let root = s.root().to_path_buf();
        let (check, suggested) = checks_of(s, &root)
            .into_iter()
            .find(|(c, _)| c.name == body.name)
            .with_context(|| format!("no check called {:?}", body.name))?;
        if suggested {
            s.save_check(&check)?;
        }
        let mut runs = checks.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(existing) = runs.get(&check.name)
            && existing.lock().unwrap_or_else(|e| e.into_inner()).running
        {
            anyhow::bail!(Conflict(format!("{} is already running", check.name)));
        }
        let started = crate::analysis::start(&check, s.root())?;
        runs.insert(check.name.clone(), started.clone());
        let run = started.lock().unwrap_or_else(|e| e.into_inner()).clone();
        Ok(run)
    })
    .await?;
    Ok(Json(run))
}

// ----- committing ----------------------------------------------------------------------------

#[derive(Deserialize)]
pub(super) struct NewCommit {
    message: String,
    /// Stage the whole working tree first, which is what the working-tree view shows. Without
    /// it only what is already staged is committed, which is what the staged view shows.
    #[serde(default)]
    all: bool,
}

/// Commits what the reviewer has just read. Hooks run; a refusal from one comes back as the
/// error, so the page can show what the hook said.
pub(super) async fn post_commit(state: Ctx, Json(body): Json<NewCommit>) -> ApiResult<Commit> {
    if body.message.trim().is_empty() {
        return Err(ApiError(
            StatusCode::BAD_REQUEST,
            "a commit needs a message".into(),
        ));
    }
    let r = with_session(&state, move |s| s.commit(&body.message, body.all)).await?;
    Ok(Json(r))
}

// ----- maintenance ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub(super) struct VersionQuery {
    /// The file the page has open, if any: a second write to a file already counted as
    /// modified does not move the status, but it does move the file's own stamp.
    #[serde(default)]
    path: Option<String>,
}

#[derive(Serialize)]
pub(super) struct Version {
    version: String,
    /// How many repositories are served: the list grows when one appears in a directory of
    /// checkouts, and the page reads it again when this moves.
    repos: usize,
}

/// What the page polls to notice that the repository changed under it, whoever changed it.
/// The number means nothing on its own; a page compares it with the last one it saw.
pub(super) async fn get_version(
    State(app): State<AppState>,
    state: Ctx,
    Query(q): Query<VersionQuery>,
) -> ApiResult<Version> {
    let version = with_session(&state, move |s| s.fingerprint(q.path.as_deref())).await?;
    Ok(Json(Version {
        version: version.to_string(),
        repos: app.repos.len(),
    }))
}

pub(super) async fn post_refresh(state: Ctx) -> ApiResult<Ok_> {
    with_session(&state, |s| s.refresh()).await?;
    Ok(Json(Ok_ { ok: true }))
}

#[derive(Serialize)]
pub(super) struct Reanchored {
    moved: usize,
}

pub(super) async fn post_reanchor(state: Ctx) -> ApiResult<Reanchored> {
    let moved = with_session(&state, |s| s.reanchor()).await?;
    Ok(Json(Reanchored { moved }))
}
