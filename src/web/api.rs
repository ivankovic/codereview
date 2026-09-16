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

/// Every served repository, in command-line order.
pub(super) async fn get_repos(State(state): State<AppState>) -> ApiResult<Vec<RepoSummary>> {
    let repos = state.repos.clone();
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

#[derive(Serialize)]
pub(super) struct StateResponse {
    /// Whether this server runs an agent at all; the page hides every way to one when not.
    agent: bool,
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
    let agent = state.agent;
    let r = with_session(&state, move |s| {
        let status = s
            .files
            .iter()
            .filter_map(|p| s.status_letter(p).map(|c| (p.clone(), c)))
            .collect();
        Ok(StateResponse {
            agent,
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

#[derive(Serialize)]
pub(super) struct FileResponse {
    #[serde(flatten)]
    view: FileView,
    lines: Vec<Vec<Segment>>,
    /// Per-line change against HEAD or the index, when the file is modified.
    line_ops: Option<Vec<Op>>,
}

pub(super) async fn get_file(state: Ctx, Query(q): Query<FileQuery>) -> ApiResult<FileResponse> {
    let r = with_session(&state, move |s| {
        let view = match q.rev.as_deref().filter(|r| !r.is_empty()) {
            Some(rev) => s.file_view_at(rev, &q.path)?,
            None => s.file_view(&q.path)?,
        };
        let lines = crate::highlight::highlight(&view.path, &view.text, &web_syntax(s));
        let mut line_ops = None;
        if q.rev.as_deref().unwrap_or("").is_empty() {
            if let Some(entry) = s.status.get(&q.path) {
                if let Some(status) = entry.unstaged.or(entry.staged) {
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
                    if let Ok(d) = s.diff(&target, &file) {
                        if d.after.line_count() == lines.len() {
                            line_ops = Some(d.after.line_ops);
                        }
                    }
                }
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
    before_lines: Vec<Vec<Segment>>,
    after_lines: Vec<Vec<Segment>>,
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
        let before_lines = crate::highlight::highlight(&old_path, &diff.before.text, &theme);
        let after_lines = crate::highlight::highlight(&file.path, &diff.after.text, &theme);
        let comments =
            crate::anchor::anchor_all(&s.review.comments_for(&file.path), &diff.after.text);
        Ok(DiffResponse {
            target: target.label(),
            rows: diff.aligned_rows(),
            diff,
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
        s.add_comment(&body.path, lines, &body.text, body.source_line.as_deref())
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

// ----- maintenance ---------------------------------------------------------------------------

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
