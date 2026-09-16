//! The browser front end: a local HTTP server over [`Session`] and one embedded page.
//!
//! Every request must carry the token printed at start-up, either as `?t=` on the page URL or
//! as `Authorization: Bearer` on the API, so no other site open in the same browser can read
//! the repository or write to REVIEW.md. The server binds loopback unless told otherwise.

use std::net::SocketAddr;
use std::ops::Deref;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use anyhow::{Context, Result};
use axum::extract::{FromRequestParts, Query, Request, State};
use axum::http::request::Parts;
use axum::http::{HeaderMap, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use crate::agent::{Agent, Event as AgentEvent, PermissionOption, prompts};
use crate::anchor::Anchored;
use crate::diff::{FileDiff, Op};
use crate::highlight::Segment;
use crate::notes::Note;
use crate::repo::{BlameLine, ChangedFile, Commit, FileStatus};
use crate::review::Comment;
use crate::session::{DiffTarget, FileView, Session};
use crate::symbols::{Location, Symbol};

const INDEX_HTML: &str = include_str!("web/index.html");

#[derive(Clone)]
struct AppState {
    repos: Arc<Vec<RepoState>>,
    token: Arc<String>,
}

/// One served repository: its session and its agent.
struct RepoState {
    session: Arc<Mutex<Session>>,
    hub: Arc<Mutex<AgentHub>>,
    /// What the configured agent is called, found out once: the `auto` kind probes PATH, and
    /// the page asks for the status twice a second.
    agent_label: String,
}

impl RepoState {
    fn new(session: Session) -> Self {
        Self {
            agent_label: agent_label(&session.config.agent),
            session: Arc::new(Mutex::new(session)),
            hub: Arc::new(Mutex::new(AgentHub::default())),
        }
    }
}

/// The repository a request is about: `?repo=N` (0-based, the order on the command line),
/// or the first one. Every `/api` handler takes this instead of the whole state.
struct Ctx {
    repos: Arc<Vec<RepoState>>,
    index: usize,
}

impl Deref for Ctx {
    type Target = RepoState;

    fn deref(&self) -> &RepoState {
        &self.repos[self.index]
    }
}

impl FromRequestParts<AppState> for Ctx {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> std::result::Result<Self, ApiError> {
        let index = parts
            .uri
            .query()
            .unwrap_or("")
            .split('&')
            .find_map(|kv| kv.strip_prefix("repo="))
            .map(|v| {
                v.parse::<usize>()
                    .map_err(|_| ApiError(StatusCode::BAD_REQUEST, format!("bad repo {v:?}")))
            })
            .transpose()?
            .unwrap_or(0);
        if index >= state.repos.len() {
            return Err(ApiError(
                StatusCode::NOT_FOUND,
                format!("no repository {index}; {} open", state.repos.len()),
            ));
        }
        Ok(Ctx {
            repos: state.repos.clone(),
            index,
        })
    }
}

/// The agent and everything the page has not fetched yet. The page polls; the server drains
/// the agent into `events` on every poll and hands back what is new.
#[derive(Default)]
struct AgentHub {
    agent: Option<Agent>,
    events: Vec<(u64, AgentEvent)>,
    seq: u64,
    status: String,
    name: String,
    permission: Option<(serde_json::Value, String, Vec<PermissionOption>)>,
    /// The user's prompts, echoed so a reloaded page can rebuild the transcript.
    transcript: Vec<(u64, String)>,
    /// What the backend says it is doing, when the turn started, when it last spoke.
    phase: String,
    turn_started: Option<Instant>,
    last_event: Option<Instant>,
}

impl AgentHub {
    fn drain(&mut self) {
        let Some(agent) = &mut self.agent else {
            return;
        };
        for event in agent.poll() {
            self.last_event = Some(Instant::now());
            match &event {
                AgentEvent::Status { text } => self.phase = text.clone(),
                AgentEvent::Permission {
                    request_id,
                    title,
                    options,
                } => {
                    self.permission = Some((request_id.clone(), title.clone(), options.clone()));
                    self.status = "waiting for permission".into();
                }
                AgentEvent::TurnDone { stop_reason } => {
                    self.status = if stop_reason == "end_turn" {
                        "idle".into()
                    } else {
                        format!("stopped: {stop_reason}")
                    };
                    self.phase.clear();
                    self.turn_started = None;
                }
                AgentEvent::Error { .. } => self.status = "error".into(),
                AgentEvent::Exited { .. } => {
                    self.status = "exited".into();
                    self.permission = None;
                    self.phase.clear();
                    self.turn_started = None;
                }
                _ => {}
            }
            self.seq += 1;
            self.events.push((self.seq, event));
        }
        if matches!(&self.events.last(), Some((_, AgentEvent::Exited { .. }))) {
            self.agent = None;
        }
        // Keep the buffer bounded; a page that fell that far behind reloads anyway.
        if self.events.len() > 5000 {
            let drop = self.events.len() - 5000;
            self.events.drain(..drop);
        }
    }
}

struct ApiError(StatusCode, String);

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(serde_json::json!({ "error": self.1 }))).into_response()
    }
}

impl From<anyhow::Error> for ApiError {
    fn from(e: anyhow::Error) -> Self {
        ApiError(StatusCode::BAD_REQUEST, format!("{e:#}"))
    }
}

type ApiResult<T> = std::result::Result<Json<T>, ApiError>;

/// Serves the page and the API for `sessions`, one repository each; the page switches
/// between them.
pub fn run(sessions: Vec<Session>, host: &str, port: u16, open_browser: bool) -> Result<()> {
    anyhow::ensure!(!sessions.is_empty(), "no repository to serve");
    let runtime = tokio::runtime::Runtime::new().context("tokio runtime")?;
    runtime.block_on(async {
        let token = random_token();
        let state = AppState {
            repos: Arc::new(sessions.into_iter().map(RepoState::new).collect()),
            token: Arc::new(token.clone()),
        };
        let addr: SocketAddr = format!("{host}:{port}")
            .parse()
            .with_context(|| format!("bad address {host}:{port}"))?;
        let listener = tokio::net::TcpListener::bind(addr)
            .await
            .with_context(|| format!("cannot listen on {addr}"))?;
        let local = listener.local_addr()?;
        let url = format!("http://{local}/?t={token}");
        println!("codereview web UI at {url}");
        if !local.ip().is_loopback() {
            eprintln!(
                "warning: listening on {}; anyone who can reach it can read this repository",
                local.ip()
            );
        }
        if open_browser {
            if let Err(e) = open::that(&url) {
                eprintln!("could not open a browser: {e}");
            }
        }
        axum::serve(listener, router(state))
            .with_graceful_shutdown(async {
                let _ = tokio::signal::ctrl_c().await;
            })
            .await
            .context("server")
    })
}

fn random_token() -> String {
    use rand::RngCore;
    let mut bytes = [0u8; 16];
    rand::rng().fill_bytes(&mut bytes);
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn router(state: AppState) -> Router {
    let api = Router::new()
        .route("/repos", get(get_repos))
        .route("/state", get(get_state))
        .route("/file", get(get_file))
        .route("/blame", get(get_blame))
        .route("/log", get(get_log))
        .route("/commit_files", get(get_commit_files))
        .route("/changes", get(get_changes))
        .route("/diff", get(get_diff))
        .route("/comments", get(get_comments).post(post_comment))
        .route("/comments/toggle", post(post_comment_toggle))
        .route("/comments/edit", post(post_comment_edit))
        .route("/comments/delete", post(post_comment_delete))
        .route("/notes", get(get_notes).post(post_note))
        .route("/notes/edit", post(post_note_edit))
        .route("/notes/delete", post(post_note_delete))
        .route("/refresh", post(post_refresh))
        .route("/reanchor", post(post_reanchor))
        .route("/config", post(post_config))
        .route("/themes", get(get_themes))
        .route("/symbols", get(get_symbols))
        .route("/definitions", get(get_definitions))
        .route("/references", get(get_references))
        .route("/symbol_search", get(get_symbol_search))
        .route("/identifier", get(get_identifier))
        .route("/agent/start", post(post_agent_start))
        .route("/agent/events", get(get_agent_events))
        .route("/agent/prompt", post(post_agent_prompt))
        .route("/agent/permission", post(post_agent_permission))
        .route("/agent/cancel", post(post_agent_cancel))
        .route("/agent/stop", post(post_agent_stop))
        .route_layer(middleware::from_fn_with_state(state.clone(), require_token));
    Router::new()
        .route("/", get(index))
        .nest("/api", api)
        .with_state(state)
}

async fn require_token(
    State(state): State<AppState>,
    headers: HeaderMap,
    req: Request,
    next: Next,
) -> Response {
    let ok = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .is_some_and(|t| t == state.token.as_str());
    if ok {
        next.run(req).await
    } else {
        ApiError(StatusCode::FORBIDDEN, "missing or wrong token".into()).into_response()
    }
}

#[derive(Deserialize)]
struct IndexQuery {
    t: Option<String>,
}

async fn index(State(state): State<AppState>, Query(q): Query<IndexQuery>) -> Response {
    if q.t.as_deref() != Some(state.token.as_str()) {
        return (
            StatusCode::FORBIDDEN,
            "open the URL codereview printed, token included",
        )
            .into_response();
    }
    let page = INDEX_HTML.replace("__TOKEN__", &state.token);
    Html(page).into_response()
}

/// Runs `f` with the session on a blocking thread; diffs and git calls are not async.
async fn with_session<T, F>(state: &RepoState, f: F) -> std::result::Result<T, ApiError>
where
    T: Send + 'static,
    F: FnOnce(&mut Session) -> Result<T> + Send + 'static,
{
    let session = state.session.clone();
    tokio::task::spawn_blocking(move || {
        let mut guard = session
            .lock()
            .map_err(|_| anyhow::anyhow!("session poisoned"))?;
        f(&mut guard)
    })
    .await
    .map_err(|e| ApiError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    .map_err(ApiError::from)
}

// ----- state ---------------------------------------------------------------------------------

#[derive(Serialize)]
struct RepoSummary {
    index: usize,
    name: String,
    root: String,
    branch: String,
    pending: usize,
}

/// Every served repository, in command-line order.
async fn get_repos(State(state): State<AppState>) -> ApiResult<Vec<RepoSummary>> {
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
struct StateResponse {
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

async fn get_state(state: Ctx) -> ApiResult<StateResponse> {
    let r = with_session(&state, |s| {
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
struct FileQuery {
    path: String,
    /// A revision to show the file at; empty or absent means the working tree.
    rev: Option<String>,
}

#[derive(Serialize)]
struct FileResponse {
    #[serde(flatten)]
    view: FileView,
    lines: Vec<Vec<Segment>>,
    /// Per-line change against HEAD or the index, when the file is modified.
    line_ops: Option<Vec<Op>>,
}

async fn get_file(state: Ctx, Query(q): Query<FileQuery>) -> ApiResult<FileResponse> {
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
struct PathQuery {
    path: String,
}

async fn get_blame(state: Ctx, Query(q): Query<PathQuery>) -> ApiResult<Vec<BlameLine>> {
    Ok(Json(
        with_session(&state, move |s| s.repo.blame(&q.path)).await?,
    ))
}

// ----- history -------------------------------------------------------------------------------

#[derive(Deserialize)]
struct LogQuery {
    path: Option<String>,
    #[serde(default)]
    skip: usize,
    #[serde(default = "default_limit")]
    limit: usize,
}

fn default_limit() -> usize {
    100
}

async fn get_log(state: Ctx, Query(q): Query<LogQuery>) -> ApiResult<Vec<Commit>> {
    let r = with_session(&state, move |s| match &q.path {
        Some(p) if !p.is_empty() => s.file_log(p, q.skip, q.limit),
        _ => s.log(q.skip, q.limit),
    })
    .await?;
    Ok(Json(r))
}

#[derive(Deserialize)]
struct HashQuery {
    hash: String,
}

async fn get_commit_files(state: Ctx, Query(q): Query<HashQuery>) -> ApiResult<Vec<ChangedFile>> {
    Ok(Json(
        with_session(&state, move |s| s.commit_files(&q.hash)).await?,
    ))
}

#[derive(Deserialize)]
struct ChangesQuery {
    #[serde(default)]
    staged: bool,
}

async fn get_changes(state: Ctx, Query(q): Query<ChangesQuery>) -> ApiResult<Vec<ChangedFile>> {
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
struct DiffQuery {
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
struct DiffResponse {
    #[serde(flatten)]
    diff: FileDiff,
    before_lines: Vec<Vec<Segment>>,
    after_lines: Vec<Vec<Segment>>,
    comments: Vec<Anchored>,
    target: String,
    rows: Vec<crate::diff::AlignedRow>,
}

async fn get_diff(state: Ctx, Query(q): Query<DiffQuery>) -> ApiResult<DiffResponse> {
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

async fn get_comments(state: Ctx) -> ApiResult<Vec<Anchored>> {
    Ok(Json(with_session(&state, |s| Ok(s.all_anchored())).await?))
}

#[derive(Deserialize)]
struct NewComment {
    path: String,
    /// Left out for a comment on the whole file or directory.
    line: Option<usize>,
    end_line: Option<usize>,
    text: String,
    source_line: Option<String>,
}

async fn post_comment(state: Ctx, Json(body): Json<NewComment>) -> ApiResult<Comment> {
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
struct CommentRef {
    comment: Comment,
    text: Option<String>,
}

#[derive(Serialize)]
struct Ok_ {
    ok: bool,
}

async fn post_comment_toggle(state: Ctx, Json(body): Json<CommentRef>) -> ApiResult<Ok_> {
    with_session(&state, move |s| s.toggle_comment(&body.comment)).await?;
    Ok(Json(Ok_ { ok: true }))
}

async fn post_comment_edit(state: Ctx, Json(body): Json<CommentRef>) -> ApiResult<Ok_> {
    let text = body.text.clone().unwrap_or_default();
    if text.trim().is_empty() {
        return Err(ApiError(StatusCode::BAD_REQUEST, "text required".into()));
    }
    with_session(&state, move |s| s.edit_comment(&body.comment, &text)).await?;
    Ok(Json(Ok_ { ok: true }))
}

async fn post_comment_delete(state: Ctx, Json(body): Json<CommentRef>) -> ApiResult<Ok_> {
    with_session(&state, move |s| s.delete_comment(&body.comment)).await?;
    Ok(Json(Ok_ { ok: true }))
}

// ----- notes ---------------------------------------------------------------------------------

async fn get_notes(state: Ctx) -> ApiResult<Vec<Note>> {
    Ok(Json(
        with_session(&state, |s| {
            Ok(s.notes.notes().into_iter().cloned().collect())
        })
        .await?,
    ))
}

#[derive(Deserialize)]
struct NewNote {
    path: Option<String>,
    text: String,
}

async fn post_note(state: Ctx, Json(body): Json<NewNote>) -> ApiResult<Note> {
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
struct NoteRef {
    note: Note,
    text: Option<String>,
}

async fn post_note_edit(state: Ctx, Json(body): Json<NoteRef>) -> ApiResult<Ok_> {
    let text = body.text.clone().unwrap_or_default();
    if text.trim().is_empty() {
        return Err(ApiError(StatusCode::BAD_REQUEST, "text required".into()));
    }
    with_session(&state, move |s| s.edit_note(&body.note, &text)).await?;
    Ok(Json(Ok_ { ok: true }))
}

async fn post_note_delete(state: Ctx, Json(body): Json<NoteRef>) -> ApiResult<Ok_> {
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

async fn get_themes() -> Json<Vec<crate::theme::Theme>> {
    Json(
        crate::theme::all()
            .into_iter()
            .filter(|t| !t.terminal)
            .collect(),
    )
}

#[derive(Deserialize)]
struct ConfigChange {
    theme: Option<String>,
    layout: Option<String>,
}

async fn post_config(state: Ctx, Json(body): Json<ConfigChange>) -> ApiResult<Ok_> {
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

async fn get_symbols(state: Ctx, Query(q): Query<PathQuery>) -> ApiResult<Vec<Symbol>> {
    Ok(Json(
        with_session(&state, move |s| Ok(s.symbols_in(&q.path))).await?,
    ))
}

#[derive(Deserialize)]
struct NameQuery {
    name: String,
}

async fn get_definitions(state: Ctx, Query(q): Query<NameQuery>) -> ApiResult<Vec<Symbol>> {
    Ok(Json(
        with_session(&state, move |s| Ok(s.definitions(&q.name))).await?,
    ))
}

async fn get_references(state: Ctx, Query(q): Query<NameQuery>) -> ApiResult<Vec<Location>> {
    Ok(Json(
        with_session(&state, move |s| Ok(s.references(&q.name))).await?,
    ))
}

#[derive(Deserialize)]
struct SearchQuery {
    q: String,
}

async fn get_symbol_search(state: Ctx, Query(q): Query<SearchQuery>) -> ApiResult<Vec<Symbol>> {
    Ok(Json(
        with_session(&state, move |s| Ok(s.search_symbols(&q.q))).await?,
    ))
}

#[derive(Deserialize)]
struct IdentifierQuery {
    path: String,
    /// 0-based row.
    line: usize,
    /// 0-based byte column.
    col: usize,
}

#[derive(Serialize)]
struct IdentifierResponse {
    name: Option<String>,
}

async fn get_identifier(
    state: Ctx,
    Query(q): Query<IdentifierQuery>,
) -> ApiResult<IdentifierResponse> {
    let name = with_session(&state, move |s| Ok(s.identifier_at(&q.path, q.line, q.col))).await?;
    Ok(Json(IdentifierResponse { name }))
}

// ----- agent ---------------------------------------------------------------------------------

#[derive(Serialize)]
struct AgentStatus {
    running: bool,
    name: String,
    status: String,
    command: String,
    permission: Option<AgentPermission>,
    /// What the backend is doing, how long the turn has run and how long it has been silent.
    phase: String,
    turn_ms: Option<u64>,
    quiet_ms: Option<u64>,
    /// Events with a sequence number above the `since` the page sent.
    events: Vec<(u64, AgentEvent)>,
    /// The user's own prompts, `(sequence, text)`, for rebuilding a transcript.
    transcript: Vec<(u64, String)>,
    seq: u64,
}

#[derive(Serialize)]
struct AgentPermission {
    title: String,
    options: Vec<PermissionOption>,
}

fn status_of(hub: &mut AgentHub, since: u64, command: &str) -> AgentStatus {
    hub.drain();
    AgentStatus {
        running: hub.agent.is_some(),
        name: hub.name.clone(),
        status: hub.status.clone(),
        command: command.to_string(),
        phase: hub.phase.clone(),
        turn_ms: hub.turn_started.map(|t| t.elapsed().as_millis() as u64),
        quiet_ms: hub
            .last_event
            .filter(|_| hub.turn_started.is_some())
            .map(|t| t.elapsed().as_millis() as u64),
        permission: hub
            .permission
            .as_ref()
            .map(|(_, title, options)| AgentPermission {
                title: title.clone(),
                options: options.clone(),
            }),
        events: hub
            .events
            .iter()
            .filter(|(n, _)| *n > since)
            .cloned()
            .collect(),
        transcript: hub
            .transcript
            .iter()
            .filter(|(n, _)| *n > since)
            .cloned()
            .collect(),
        seq: hub.seq,
    }
}

fn agent_command(state: &RepoState) -> String {
    state.agent_label.clone()
}

fn agent_label(config: &crate::config::AgentConfig) -> String {
    match config.kind.as_str() {
        "acp" => format!("{} {}", config.command, config.args.join(" ")),
        "claude" => config.claude_command.clone(),
        _ => {
            if std::process::Command::new(&config.claude_command)
                .arg("--version")
                .output()
                .is_ok()
            {
                config.claude_command.clone()
            } else {
                format!("{} {}", config.command, config.args.join(" "))
            }
        }
    }
}

/// Starts the agent when it is not running. Blocks for the handshake, which can take a while
/// when `npx` has to fetch an adapter.
async fn post_agent_start(state: Ctx) -> ApiResult<AgentStatus> {
    let already = state.hub.lock().map(|h| h.agent.is_some()).unwrap_or(false);
    if !already {
        let session = state.session.clone();
        let hub = state.hub.clone();
        tokio::task::spawn_blocking(move || -> Result<()> {
            let (agent_config, root) = {
                let s = session
                    .lock()
                    .map_err(|_| anyhow::anyhow!("session poisoned"))?;
                (s.config.agent.clone(), s.root().to_path_buf())
            };
            let outcome = crate::agent::spawn(&agent_config, &root);
            let mut hub = hub.lock().map_err(|_| anyhow::anyhow!("hub poisoned"))?;
            match outcome {
                Ok(agent) => {
                    hub.name = agent.name().to_string();
                    hub.agent = Some(agent);
                    hub.status = "idle".into();
                    Ok(())
                }
                Err(e) => {
                    hub.status = format!("failed to start: {e:#}");
                    Err(e)
                }
            }
        })
        .await
        .map_err(|e| ApiError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))??;
    }
    let command = agent_command(&state);
    let mut hub = state
        .hub
        .lock()
        .map_err(|_| ApiError(StatusCode::INTERNAL_SERVER_ERROR, "hub poisoned".into()))?;
    Ok(Json(status_of(&mut hub, u64::MAX, &command)))
}

#[derive(Deserialize)]
struct SinceQuery {
    #[serde(default)]
    since: u64,
}

async fn get_agent_events(state: Ctx, Query(q): Query<SinceQuery>) -> ApiResult<AgentStatus> {
    let command = agent_command(&state);
    let mut hub = state
        .hub
        .lock()
        .map_err(|_| ApiError(StatusCode::INTERNAL_SERVER_ERROR, "hub poisoned".into()))?;
    Ok(Json(status_of(&mut hub, q.since, &command)))
}

#[derive(Deserialize)]
struct ContextItem {
    uri: String,
    text: String,
}

/// Either free text, or a preset the server words: `address_comment` (with `comment`),
/// `address_all`, `review_diff` (with `path` and `target`), `about_code` (with `path`,
/// `text`, and `line`/`end_line` unless the question is about the whole path).
#[derive(Deserialize)]
struct AgentPromptRequest {
    text: Option<String>,
    preset: Option<String>,
    comment: Option<Comment>,
    path: Option<String>,
    target: Option<String>,
    line: Option<usize>,
    end_line: Option<usize>,
    #[serde(default)]
    context: Vec<ContextItem>,
}

#[derive(Serialize)]
struct AgentPromptResponse {
    text: String,
}

async fn post_agent_prompt(
    state: Ctx,
    Json(body): Json<AgentPromptRequest>,
) -> ApiResult<AgentPromptResponse> {
    let text = match body.preset.as_deref() {
        Some("address_comment") => {
            let c = body
                .comment
                .as_ref()
                .ok_or_else(|| ApiError(StatusCode::BAD_REQUEST, "comment required".into()))?;
            prompts::address_comment(&c.path, c.line_label().as_deref(), &c.text)
        }
        Some("address_all") => prompts::address_all().to_string(),
        Some("review_diff") => prompts::review_diff(
            body.path.as_deref().unwrap_or(""),
            body.target.as_deref().unwrap_or("working tree"),
        ),
        Some("about_code") => prompts::about_code(
            body.path.as_deref().unwrap_or(""),
            body.line
                .map(|line| (line, body.end_line.unwrap_or(line).max(line))),
            body.text.as_deref().unwrap_or("Explain this."),
        ),
        Some(other) => {
            return Err(ApiError(
                StatusCode::BAD_REQUEST,
                format!("unknown preset {other}"),
            ));
        }
        None => body.text.clone().unwrap_or_default(),
    };
    if text.trim().is_empty() {
        return Err(ApiError(StatusCode::BAD_REQUEST, "text required".into()));
    }
    let context: Vec<(String, String)> =
        body.context.into_iter().map(|c| (c.uri, c.text)).collect();
    let mut hub = state
        .hub
        .lock()
        .map_err(|_| ApiError(StatusCode::INTERNAL_SERVER_ERROR, "hub poisoned".into()))?;
    let Some(agent) = &mut hub.agent else {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "the agent is not running; start it first".into(),
        ));
    };
    agent.prompt(&text, &context)?;
    hub.status = "working".into();
    hub.phase = "prompt sent".into();
    hub.turn_started = Some(Instant::now());
    hub.last_event = Some(Instant::now());
    hub.seq += 1;
    let seq = hub.seq;
    hub.transcript.push((seq, text.clone()));
    Ok(Json(AgentPromptResponse { text }))
}

#[derive(Deserialize)]
struct PermissionAnswer {
    option_id: Option<String>,
}

async fn post_agent_permission(state: Ctx, Json(body): Json<PermissionAnswer>) -> ApiResult<Ok_> {
    let mut hub = state
        .hub
        .lock()
        .map_err(|_| ApiError(StatusCode::INTERNAL_SERVER_ERROR, "hub poisoned".into()))?;
    let Some((id, _, _)) = hub.permission.take() else {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "no permission request is pending".into(),
        ));
    };
    let Some(agent) = &mut hub.agent else {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "the agent is not running".into(),
        ));
    };
    agent.respond_permission(&id, body.option_id.as_deref())?;
    hub.status = "working".into();
    Ok(Json(Ok_ { ok: true }))
}

async fn post_agent_cancel(state: Ctx) -> ApiResult<Ok_> {
    let mut hub = state
        .hub
        .lock()
        .map_err(|_| ApiError(StatusCode::INTERNAL_SERVER_ERROR, "hub poisoned".into()))?;
    if let Some(agent) = &mut hub.agent {
        agent.cancel()?;
    }
    Ok(Json(Ok_ { ok: true }))
}

async fn post_agent_stop(state: Ctx) -> ApiResult<Ok_> {
    let mut hub = state
        .hub
        .lock()
        .map_err(|_| ApiError(StatusCode::INTERNAL_SERVER_ERROR, "hub poisoned".into()))?;
    hub.agent = None;
    hub.permission = None;
    hub.status = "stopped".into();
    Ok(Json(Ok_ { ok: true }))
}

// ----- maintenance ---------------------------------------------------------------------------

async fn post_refresh(state: Ctx) -> ApiResult<Ok_> {
    with_session(&state, |s| s.refresh()).await?;
    Ok(Json(Ok_ { ok: true }))
}

#[derive(Serialize)]
struct Reanchored {
    moved: usize,
}

async fn post_reanchor(state: Ctx) -> ApiResult<Reanchored> {
    let moved = with_session(&state, |s| s.reanchor()).await?;
    Ok(Json(Reanchored { moved }))
}

/// For tests: the router bound to a scratch session, with its token.
#[cfg(test)]
pub(crate) fn test_router(roots: &[&Path]) -> Result<(Router, String)> {
    let token = random_token();
    let state = AppState {
        repos: Arc::new(
            roots
                .iter()
                .map(|r| RepoState::new(crate::session::tests::scratch_session(r)))
                .collect(),
        ),
        token: Arc::new(token.clone()),
    };
    Ok((router(state), token))
}

#[cfg(not(test))]
#[allow(dead_code)]
fn _keep_path_import(_: &Path) {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::tests::scratch_repo;
    use std::io::{Read, Write};
    use std::net::TcpStream;

    /// Serves the router on a random port and returns its address.
    fn serve(roots: &[&Path]) -> (SocketAddr, String, tokio::runtime::Runtime) {
        let (router, token) = test_router(roots).unwrap();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let addr = rt.block_on(async {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
            addr
        });
        (addr, token, rt)
    }

    fn request(
        addr: SocketAddr,
        method: &str,
        path: &str,
        token: Option<&str>,
        body: Option<&str>,
    ) -> (u16, String) {
        let mut stream = TcpStream::connect(addr).unwrap();
        let mut req =
            format!("{method} {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n");
        if let Some(t) = token {
            req.push_str(&format!("Authorization: Bearer {t}\r\n"));
        }
        if let Some(b) = body {
            req.push_str(&format!(
                "Content-Type: application/json\r\nContent-Length: {}\r\n",
                b.len()
            ));
        }
        req.push_str("\r\n");
        if let Some(b) = body {
            req.push_str(b);
        }
        stream.write_all(req.as_bytes()).unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        let status: u16 = response[9..12].parse().unwrap();
        let body = response.split("\r\n\r\n").nth(1).unwrap_or("").to_string();
        (status, body)
    }

    #[test]
    fn api_round_trip() {
        let dir = scratch_repo();
        let (addr, token, _rt) = serve(&[dir.path()]);
        let t = Some(token.as_str());

        assert_eq!(request(addr, "GET", "/", None, None).0, 403);
        assert_eq!(request(addr, "GET", "/api/state", None, None).0, 403);
        let (status, page) = request(addr, "GET", &format!("/?t={token}"), None, None);
        assert_eq!(status, 200);
        assert!(page.contains(&token));

        let (status, body) = request(addr, "GET", "/api/state", t, None);
        assert_eq!(status, 200);
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["files"].as_array().unwrap().len(), 3);

        let (status, body) = request(addr, "GET", "/api/file?path=a.rs", t, None);
        assert_eq!(status, 200);
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["lines"].as_array().unwrap().len(), 3);
        assert_eq!(v["line_ops"][2], "insert");

        let (status, body) = request(
            addr,
            "POST",
            "/api/comments",
            t,
            Some(r#"{"path":"a.rs","line":2,"text":"hm"}"#),
        );
        assert_eq!(status, 200, "{body}");
        let comment: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(comment["anchor"], "fn b() {}");

        let (status, body) = request(addr, "GET", "/api/comments", t, None);
        assert_eq!(status, 200);
        let list: Vec<serde_json::Value> = serde_json::from_str(&body).unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0]["state"], "exact");

        // A comment on a whole file, and one on a directory: no line, no anchor.
        let (status, body) = request(
            addr,
            "POST",
            "/api/comments",
            t,
            Some(r#"{"path":"a.rs","text":"needs tests"}"#),
        );
        assert_eq!(status, 200, "{body}");
        let whole: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert!(whole["line"].is_null() && whole["anchor"].is_null());
        let (status, body) = request(
            addr,
            "POST",
            "/api/comments",
            t,
            Some(r#"{"path":"src","text":"too many modules"}"#),
        );
        assert_eq!(status, 200, "{body}");
        let (_, body) = request(addr, "GET", "/api/comments", t, None);
        let list: Vec<serde_json::Value> = serde_json::from_str(&body).unwrap();
        assert_eq!(list.len(), 3);
        assert!(
            list.iter()
                .any(|a| a["comment"]["path"] == "src" && a["line"].is_null())
        );
        assert_eq!(
            request(
                addr,
                "POST",
                "/api/comments/delete",
                t,
                Some(&serde_json::json!({ "comment": whole }).to_string())
            )
            .0,
            200
        );
        let (_, body) = request(addr, "GET", "/api/file?path=a.rs", t, None);
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["comments"].as_array().unwrap().len(), 1);

        let toggle = serde_json::json!({ "comment": comment }).to_string();
        assert_eq!(
            request(addr, "POST", "/api/comments/toggle", t, Some(&toggle)).0,
            200
        );
        let (_, body) = request(addr, "GET", "/api/comments", t, None);
        let list: Vec<serde_json::Value> = serde_json::from_str(&body).unwrap();
        assert_eq!(list[0]["comment"]["section"], "Completed");

        let (status, body) = request(addr, "GET", "/api/log", t, None);
        assert_eq!(status, 200);
        let log: Vec<serde_json::Value> = serde_json::from_str(&body).unwrap();
        assert_eq!(log.len(), 2);
        let hash = log[0]["hash"].as_str().unwrap().to_string();
        let (status, body) = request(
            addr,
            "GET",
            &format!("/api/commit_files?hash={hash}"),
            t,
            None,
        );
        assert_eq!(status, 200);
        assert!(body.contains("Makefile"));

        let (status, body) = request(
            addr,
            "GET",
            &format!("/api/diff?kind=commit&hash={hash}&path=a.rs&status=M"),
            t,
            None,
        );
        assert_eq!(status, 200, "{body}");
        let d: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(d["structural"], true);
        assert_eq!(d["after"]["line_ops"][1], "insert");

        let (status, body) = request(
            addr,
            "GET",
            "/api/diff?kind=working&path=a.rs&status=M",
            t,
            None,
        );
        assert_eq!(status, 200, "{body}");
        let (status, _) = request(addr, "GET", "/api/changes", t, None);
        assert_eq!(status, 200);

        let (status, _) = request(
            addr,
            "POST",
            "/api/notes",
            t,
            Some(r#"{"path":"a.rs","text":"entry"}"#),
        );
        assert_eq!(status, 200);
        let (_, body) = request(addr, "GET", "/api/notes", t, None);
        assert!(body.contains("entry"));
        let (status, _) = request(addr, "POST", "/api/reanchor", t, None);
        assert_eq!(status, 200);
        let (status, _) = request(addr, "GET", "/api/blame?path=a.rs", t, None);
        assert_eq!(status, 200);

        let (status, body) = request(addr, "POST", "/api/config", t, Some(r#"{"theme":"Nord"}"#));
        assert_eq!(status, 200, "{body}");
        let (_, body) = request(addr, "GET", "/api/state", t, None);
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["theme"]["name"], "Nord");
        assert_eq!(v["theme"]["insert_fg"], "#a3be8c");
        assert!(
            v["themes"]
                .as_array()
                .unwrap()
                .iter()
                .all(|n| n != "Terminal")
        );
        let (status, _) = request(addr, "POST", "/api/config", t, Some(r#"{"theme":"Nope"}"#));
        assert_eq!(status, 400);

        let (status, body) = request(addr, "GET", "/api/symbols?path=a.rs", t, None);
        assert_eq!(status, 200, "{body}");
        let syms: Vec<serde_json::Value> = serde_json::from_str(&body).unwrap();
        assert_eq!(syms.len(), 3);
        let (_, body) = request(addr, "GET", "/api/definitions?name=b", t, None);
        let defs: Vec<serde_json::Value> = serde_json::from_str(&body).unwrap();
        assert_eq!(defs[0]["line"], 2);
        let (_, body) = request(addr, "GET", "/api/references?name=b", t, None);
        let refs: Vec<serde_json::Value> = serde_json::from_str(&body).unwrap();
        assert_eq!(refs.len(), 1);
        let (_, body) = request(
            addr,
            "GET",
            "/api/identifier?path=a.rs&line=1&col=4",
            t,
            None,
        );
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&body).unwrap()["name"],
            "b"
        );
        let (_, body) = request(addr, "GET", "/api/symbol_search?q=c", t, None);
        assert!(body.contains("\"c\""));
    }

    #[test]
    fn several_repositories() {
        let a = scratch_repo();
        let b = scratch_repo();
        std::fs::write(
            b.path().join("REVIEW.md"),
            "# Pending\n\n- In a.rs on line 1: hi\n",
        )
        .unwrap();
        let (addr, token, _rt) = serve(&[a.path(), b.path()]);
        let t = Some(token.as_str());
        let (status, body) = request(addr, "GET", "/api/repos", t, None);
        assert_eq!(status, 200, "{body}");
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v.as_array().unwrap().len(), 2);
        assert_eq!(v[0]["index"], 0);
        assert_eq!(v[1]["pending"], 1);
        let b_root = b.path().canonicalize().unwrap().display().to_string();
        assert_eq!(v[1]["root"], b_root);
        // The selector reaches the second repository; the default is the first.
        let (_, body) = request(addr, "GET", "/api/state?repo=1", t, None);
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["root"], b_root);
        assert_eq!(v["pending"], 1);
        let (_, body) = request(addr, "GET", "/api/state", t, None);
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["pending"], 0);
        let (_, body) = request(addr, "GET", "/api/comments?repo=1", t, None);
        assert!(body.contains("\"hi\""), "{body}");
        // Out of range and malformed selectors are refused.
        assert_eq!(request(addr, "GET", "/api/state?repo=2", t, None).0, 404);
        assert_eq!(request(addr, "GET", "/api/state?repo=x", t, None).0, 400);
    }

    #[test]
    fn agent_endpoints() {
        let dir = scratch_repo();
        {
            let mut s = crate::session::tests::scratch_session(dir.path());
            s.config.agent = crate::config::AgentConfig {
                kind: "fake-acp".into(),
                ..Default::default()
            };
            s.set_layout("auto").unwrap(); // saves the agent config with it
        }
        let (addr, token, _rt) = serve(&[dir.path()]);
        let t = Some(token.as_str());
        let (status, body) = request(addr, "GET", "/api/agent/events", t, None);
        assert_eq!(status, 200);
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&body).unwrap()["running"],
            false
        );
        let (status, body) = request(
            addr,
            "POST",
            "/api/agent/prompt",
            t,
            Some(r#"{"text":"hi"}"#),
        );
        assert_eq!(status, 409, "{body}");
        let (status, body) = request(addr, "POST", "/api/agent/start", t, None);
        assert_eq!(status, 200, "{body}");
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["name"], "fake-agent");
        let (status, body) = request(
            addr,
            "POST",
            "/api/agent/prompt",
            t,
            Some(r#"{"preset":"address_all"}"#),
        );
        assert_eq!(status, 200, "{body}");
        assert!(body.contains("Pending"));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        let mut permission = None;
        while permission.is_none() && std::time::Instant::now() < deadline {
            let (_, body) = request(addr, "GET", "/api/agent/events?since=0", t, None);
            let v: serde_json::Value = serde_json::from_str(&body).unwrap();
            if !v["permission"].is_null() {
                permission = Some(
                    v["permission"]["options"][0]["option_id"]
                        .as_str()
                        .unwrap()
                        .to_string(),
                );
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let option = permission.expect("permission request");
        // Progress fields and the backend's log lines come with the events.
        let (_, body) = request(addr, "GET", "/api/agent/events?since=0", t, None);
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert!(v["turn_ms"].is_u64(), "{body}");
        assert!(v["quiet_ms"].is_u64(), "{body}");
        assert!(
            v["events"]
                .as_array()
                .unwrap()
                .iter()
                .any(|e| e[1]["event"] == "log"
                    && e[1]["text"]
                        .as_str()
                        .unwrap()
                        .starts_with("started in-process fake")),
            "{body}"
        );
        let answer = serde_json::json!({ "option_id": option }).to_string();
        assert_eq!(
            request(addr, "POST", "/api/agent/permission", t, Some(&answer)).0,
            200
        );
        let mut done = false;
        let mut seen = 0;
        while !done && std::time::Instant::now() < deadline {
            let (_, body) = request(
                addr,
                "GET",
                &format!("/api/agent/events?since={seen}"),
                t,
                None,
            );
            let v: serde_json::Value = serde_json::from_str(&body).unwrap();
            for (n, e) in v["events"]
                .as_array()
                .unwrap()
                .iter()
                .map(|p| (p[0].as_u64().unwrap(), &p[1]))
            {
                seen = seen.max(n);
                if e["event"] == "turn_done" {
                    done = true;
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(done);
        assert_eq!(request(addr, "POST", "/api/agent/stop", t, None).0, 200);
        let (_, body) = request(addr, "GET", "/api/agent/events", t, None);
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&body).unwrap()["running"],
            false
        );
    }
}
