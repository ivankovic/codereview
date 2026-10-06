//! The browser front end: a local HTTP server over [`Session`] and one embedded page.
//!
//! Every request must carry the token printed at start-up, either as `?t=` on the page URL or
//! as `Authorization: Bearer` on the API, so no other site open in the same browser can read
//! the repository or write to REVIEW.md. The server binds loopback unless told otherwise.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::ops::Deref;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use axum::extract::{DefaultBodyLimit, FromRequestParts, Query, Request, State};
use axum::http::request::Parts;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Form, Json, Router};
use serde::{Deserialize, Serialize};

use crate::agent::{Agent, Event as AgentEvent, PermissionOption, prompts};
use crate::analysis::{Figures, Run as CheckRun, Staleness};
use crate::anchor::Anchored;
use crate::config::Check;
use crate::diff::{FileDiff, Op};
use crate::highlight::Segment;
use crate::notes::Note;
use crate::repo::{BlameLine, ChangedFile, Commit, FileStatus};
use crate::review::Comment;
use crate::session::{DiffTarget, FileView, Session};
use crate::symbols::{Location, Symbol};
use crate::transcript::{Entry, Transcript};

mod agent;
mod api;
mod auth;
#[cfg(test)]
mod perf;
#[cfg(test)]
mod tests;

pub use auth::hash_password;
use auth::{Auth, COOKIE, SESSION_MAX_AGE, Sessions, constant_eq};

const INDEX_HTML: &str = include_str!("web/index.html");
const LOGIN_HTML: &str = include_str!("web/login.html");

/// The page carries its own script and styles and fetches nothing else, so everything but
/// same-origin requests can be refused. `form-action` is for the sign-in form, and `blob:`
/// images and videos are the repository's own pictures and the recordings its tests left,
/// fetched with the API token and shown from memory.
const CSP: &str = "default-src 'none'; connect-src 'self'; script-src 'unsafe-inline'; \
                   style-src 'unsafe-inline'; img-src 'self' data: blob:; media-src blob:; \
                   manifest-src 'self'; \
                   frame-ancestors 'none'; base-uri 'none'; form-action 'self'";

/// What a phone needs to put the page on its home screen: the app's name and icons, and
/// nothing about any repository. Served to anyone, like the sign-in page, since a browser
/// fetches a manifest without the session cookie.
const MANIFEST_JSON: &str = include_str!("web/manifest.json");
const ICON_SVG: &str = include_str!("web/icon.svg");
const ICON_PNG: &[u8] = include_bytes!("web/icon.png");

/// How many API requests may be working at once. Each one can take a repository's lock and
/// do git work on a blocking thread, so letting them pile up without bound only moves the
/// queue somewhere less visible. Sixteen is far more than a few browsers need, whose
/// busiest habit is one agent poll every half second, and few enough that slow requests
/// cannot fill the blocking pool.
const MAX_IN_FLIGHT: usize = 16;

/// The largest request body. The page sends comments, notes and agent prompts, the last of
/// which can carry a selection; none of that is a megabyte.
const MAX_BODY: usize = 1024 * 1024;

#[derive(Clone)]
struct AppState {
    repos: Arc<Repos>,
    /// What a browser must show to be let in: a password, or a token in the first URL.
    auth: Arc<Auth>,
    /// Everyone signed in, and how sign-in has been going.
    sessions: Arc<Mutex<Sessions>>,
    /// Mark the session cookie `Secure`: set when the page is published over HTTPS.
    secure_cookie: bool,
    /// One permit per request the API is working on.
    in_flight: Arc<tokio::sync::Semaphore>,
    /// Whether the thread that builds every repository's symbol index at start is still at
    /// it; see [`warm_and_keep`].
    warming: Arc<std::sync::atomic::AtomicBool>,
}

impl AppState {
    fn sessions(&self) -> std::sync::MutexGuard<'_, Sessions> {
        // A lock this short is only poisoned by a panic while holding it, which would mean
        // a bug in the few lines below each `sessions()` call.
        self.sessions.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// Every served repository, in the order they were opened: the command line's, then each one
/// that appeared later in a directory of checkouts. The list only grows, since a page names a
/// repository by its place in it and an address must keep naming the same one.
struct Repos(std::sync::RwLock<Vec<Arc<RepoState>>>);

impl Repos {
    fn new(sessions: impl IntoIterator<Item = Session>) -> Self {
        Self(std::sync::RwLock::new(
            sessions
                .into_iter()
                .map(|s| Arc::new(RepoState::new(s)))
                .collect(),
        ))
    }

    fn list(&self) -> std::sync::RwLockReadGuard<'_, Vec<Arc<RepoState>>> {
        // Nothing panics while holding this lock but a push, which cannot.
        self.0.read().unwrap_or_else(|e| e.into_inner())
    }

    fn get(&self, index: usize) -> Option<Arc<RepoState>> {
        self.list().get(index).cloned()
    }

    /// A copy of the list as it stands, to walk without holding the lock.
    fn all(&self) -> Vec<Arc<RepoState>> {
        self.list().clone()
    }

    fn len(&self) -> usize {
        self.list().len()
    }

    fn push(&self, session: Session) {
        let repo = Arc::new(RepoState::new(session));
        self.0.write().unwrap_or_else(|e| e.into_inner()).push(repo);
    }
}

/// One served repository: its session, its agent, and the checks that have been run in it.
struct RepoState {
    session: Arc<Mutex<Session>>,
    hub: Arc<Mutex<AgentHub>>,
    /// The last run of each check, by name, in flight or finished. Held for as long as the
    /// server is, and no longer: what outlives a run is the artefact it wrote.
    checks: Arc<Mutex<HashMap<String, crate::analysis::Running>>>,
    /// What the configured agent is called, found out once: the `auto` kind probes PATH, and
    /// the page asks for the status twice a second.
    agent_label: String,
    /// When a request last named this repository: `OVERVIEW.md` is kept for the ones someone
    /// is looking at, not for everything served.
    touched: Mutex<Instant>,
}

impl RepoState {
    fn new(session: Session) -> Self {
        Self {
            agent_label: session.config.agent.label(),
            session: Arc::new(Mutex::new(session)),
            hub: Arc::new(Mutex::new(AgentHub::default())),
            checks: Arc::new(Mutex::new(HashMap::new())),
            // Not looked at yet: the keep loop works only on repositories someone opens,
            // rather than on all of them at once after every restart, holding each one's lock
            // while a page that just opened waits behind it.
            touched: Mutex::new(
                Instant::now()
                    .checked_sub(LOOKED_AT)
                    .unwrap_or_else(Instant::now),
            ),
        }
    }

    /// What the checks last did here, for `OVERVIEW.md`.
    fn check_facts(&self) -> Vec<crate::facts::CheckFact> {
        let runs = self.checks.lock().unwrap_or_else(|e| e.into_inner());
        let mut facts: Vec<_> = runs
            .values()
            .filter_map(|r| {
                let run = r.lock().unwrap_or_else(|e| e.into_inner());
                crate::facts::CheckFact::from_run(&run)
            })
            .collect();
        facts.sort_by(|a, b| a.name.cmp(&b.name));
        facts
    }
}

/// The repository a request is about: `?repo=N` (0-based, the order on the command line),
/// or the first one. Every `/api` handler takes this instead of the whole state.
struct Ctx {
    repo: Arc<RepoState>,
    /// Whether the server's own thread is still building indexes: a request that finds none
    /// then waits for that rather than building a second one under the lock.
    warming: Arc<std::sync::atomic::AtomicBool>,
}

impl Deref for Ctx {
    type Target = RepoState;

    fn deref(&self) -> &RepoState {
        &self.repo
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
        let Some(repo) = state.repos.get(index) else {
            return Err(ApiError(
                StatusCode::NOT_FOUND,
                format!("no repository {index}; {} open", state.repos.len()),
            ));
        };
        *repo.touched.lock().unwrap_or_else(|e| e.into_inner()) = Instant::now();
        Ok(Ctx {
            repo,
            warming: state.warming.clone(),
        })
    }
}

/// The agent and what it has said. The page polls; every poll drains the agent into the
/// transcript and hands back the entries that changed.
struct AgentHub {
    agent: Option<Agent>,
    /// What has been said, built exactly as the terminal builds it.
    transcript: Transcript,
    name: String,
    /// Set while a start is under way, so two browsers cannot start two agents.
    starting: bool,
}

impl Default for AgentHub {
    fn default() -> Self {
        Self {
            agent: None,
            transcript: Transcript::new(),
            name: String::new(),
            starting: false,
        }
    }
}

impl AgentHub {
    fn drain(&mut self) {
        let Some(agent) = &mut self.agent else {
            return;
        };
        let events = agent.poll();
        let gone = events
            .iter()
            .any(|e| matches!(e, AgentEvent::Exited { .. }));
        for event in events {
            self.transcript.apply(event);
        }
        if gone {
            self.agent = None;
        }
        self.trim();
    }

    /// Keeps what the hub holds bounded. One entry can carry a whole file, so the bound is
    /// bytes; a page that has fallen behind is told to take the transcript again.
    fn trim(&mut self) {
        const MAX_BYTES: usize = 4 * 1024 * 1024;
        self.transcript.trim_to(MAX_BYTES);
    }
}

struct ApiError(StatusCode, String);

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(serde_json::json!({ "error": self.1 }))).into_response()
    }
}

/// Not a bad request, but a request the server is in no state to carry out: no agent
/// running, no permission pending, an answer to a request that is no longer the one
/// waiting. Answered with 409 wherever it is raised.
#[derive(Debug)]
struct Conflict(String);

impl std::fmt::Display for Conflict {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Conflict {}

impl From<anyhow::Error> for ApiError {
    fn from(e: anyhow::Error) -> Self {
        // Anywhere in the chain: a conflict with context added over it is still a
        // conflict, and `downcast_ref` alone only looks at the outermost error.
        if let Some(conflict) = e.chain().find_map(|c| c.downcast_ref::<Conflict>()) {
            return ApiError(StatusCode::CONFLICT, conflict.0.clone());
        }
        // The whole chain holds git command lines, raw git stderr and absolute paths. The
        // operator can see it in the log; the browser is told only what went wrong.
        eprintln!("api error: {e:#}");
        // Something that failed on this machine is this machine's fault, and a client that
        // retries the same request should be told so rather than told to change it.
        let ours = e.chain().any(|c| {
            c.downcast_ref::<std::io::Error>().is_some_and(|io| {
                !matches!(
                    io.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::InvalidInput
                )
            })
        });
        let status = if ours {
            StatusCode::INTERNAL_SERVER_ERROR
        } else {
            StatusCode::BAD_REQUEST
        };
        ApiError(status, format!("{e}"))
    }
}

type ApiResult<T> = std::result::Result<Json<T>, ApiError>;

/// What a handler answers with when there is nothing to say but "done".
#[derive(Serialize)]
pub(crate) struct Ok_ {
    pub(crate) ok: bool,
}

/// Serves the page and the API for `sessions`, one repository each; the page switches
/// between them.
/// How to serve: where to listen, whether to open a browser, and the address the page is
/// reached at from outside.
pub struct Serve {
    pub host: String,
    pub port: u16,
    pub open_browser: bool,
    /// What a reverse proxy publishes, `https://review.example.com`: the URL to print, and
    /// an HTTPS one also marks the session cookie `Secure`.
    pub public_url: Option<String>,
}

/// `checkouts` are the directories of checkouts among the paths served: a repository that
/// appears in one later is served from then on.
pub fn run(
    sessions: Vec<Session>,
    checkouts: crate::session::Checkouts,
    serve: Serve,
) -> Result<()> {
    anyhow::ensure!(!sessions.is_empty(), "no repository to serve");
    let public = match serve.public_url.as_deref() {
        Some(url) => {
            let url = url.trim_end_matches('/');
            anyhow::ensure!(
                url.starts_with("http://") || url.starts_with("https://"),
                "--public-url must start with http:// or https://, got {url:?}"
            );
            Some(url.to_string())
        }
        None => None,
    };
    let auth = Auth::from_env()?;
    let loopback_only = serve
        .host
        .parse::<std::net::IpAddr>()
        .is_ok_and(|ip| ip.is_loopback());
    anyhow::ensure!(
        auth.is_password() || loopback_only,
        "listening on {} would put this on the network with only a token; set \
         CODEREVIEW_PASSWORD_HASH (see `codereview hash-password`), or bind a loopback address \
         and put a reverse proxy in front",
        serve.host
    );
    if public.is_some() && !auth.is_password() {
        eprintln!(
            "warning: published at a URL but signed in with a token; anyone who learns it is in. \
             Set CODEREVIEW_PASSWORD_HASH, see `codereview hash-password`"
        );
    }
    let runtime = tokio::runtime::Runtime::new().context("tokio runtime")?;
    let served = runtime.block_on(async {
        let first_url = match auth.token() {
            Some(token) => format!("/?t={token}"),
            None => "/".to_string(),
        };
        let state = AppState {
            repos: Arc::new(Repos::new(sessions)),
            auth: Arc::new(auth),
            sessions: Arc::new(Mutex::new(Sessions::default())),
            secure_cookie: public.as_deref().is_some_and(|u| u.starts_with("https://")),
            in_flight: Arc::new(tokio::sync::Semaphore::new(MAX_IN_FLIGHT)),
            warming: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        };
        let addr: SocketAddr = format!("{}:{}", serve.host, serve.port)
            .parse()
            .with_context(|| format!("bad address {}:{}", serve.host, serve.port))?;
        let listener = tokio::net::TcpListener::bind(addr)
            .await
            .with_context(|| format!("cannot listen on {addr}"))?;
        let local = listener.local_addr()?;
        let url = format!("http://{local}{first_url}");
        match &public {
            Some(public) => {
                println!("codereview web UI at {public}{first_url}");
                println!("listening on {local}");
            }
            None => println!("codereview web UI at {url}"),
        }
        if state.auth.is_password() {
            println!("sign in with the password behind CODEREVIEW_PASSWORD_HASH");
        }
        if serve.open_browser {
            if let Err(e) = open::that(&url) {
                eprintln!("could not open a browser: {e}");
            }
        }
        watch_repositories(state.clone(), !checkouts.is_empty());
        warm_and_keep(state.clone(), checkouts);
        axum::serve(listener, router(state))
            .with_graceful_shutdown(shutdown_signal())
            .await
            .context("server")
    });
    // The threads that watch and keep the repositories loop for as long as the process lives,
    // and dropping a runtime waits for its blocking threads: without this a stop signal ends
    // the server and then hangs until the service manager kills it.
    runtime.shutdown_background();
    served
}

/// How long after its last request a repository is still someone's, for keeping its
/// `OVERVIEW.md`: a page polls every two seconds while it is open.
const LOOKED_AT: Duration = Duration::from_secs(120);

/// What no request should wait for, on one thread of their own. First, every served
/// repository's symbol index is built, one repository at a time, and handed to its session,
/// so that the first look at an overview finds it ready rather than paying seconds for it;
/// the page shows `OVERVIEW.md` meanwhile. Then, every few seconds, the directories of
/// checkouts are read again and a repository that has appeared in one is served and indexed,
/// and `OVERVIEW.md` is kept for the repositories someone has looked at lately: written once
/// one has held still for ten seconds and says something new.
fn warm_and_keep(state: AppState, mut checkouts: crate::session::Checkouts) {
    use std::sync::atomic::Ordering;
    state.warming.store(true, Ordering::Relaxed);
    tokio::task::spawn_blocking(move || {
        warm(&state.repos.all());
        state.warming.store(false, Ordering::Relaxed);
        loop {
            std::thread::sleep(TICK);
            let found = checkouts.scan(|m| eprintln!("{m}"));
            if !found.is_empty() {
                let before = state.repos.len();
                for session in found {
                    println!("serving {}, new in its directory", session.root().display());
                    state.repos.push(session);
                }
                // Not marked as warming: a request for a new repository builds its own index
                // rather than waiting on this thread to get to it.
                warm(&state.repos.all()[before..]);
            }
            for repo in state.repos.all() {
                let looked_at = repo
                    .touched
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .elapsed()
                    < LOOKED_AT;
                if !looked_at {
                    continue;
                }
                let checks = repo.check_facts();
                let mut s = repo.session.lock().unwrap_or_else(|e| e.into_inner());
                if let Err(e) = s.keep_facts(Some(checks)) {
                    eprintln!("cannot keep {}: {e:#}", crate::facts::FILE_NAME);
                }
            }
        }
    });
}

/// Builds the symbol index of each of `repos` that has none, one at a time, and hands it to
/// its session.
fn warm(repos: &[Arc<RepoState>]) {
    for repo in repos {
        let (root, files) = {
            let s = repo.session.lock().unwrap_or_else(|e| e.into_inner());
            if s.has_index() {
                continue;
            }
            (s.root().to_path_buf(), s.files.clone())
        };
        let index = crate::symbols::Index::build(&root, &files);
        repo.session
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .install_index(index);
    }
}

/// How often each repository is looked at, and how long it must then hold still before a
/// check runs. A save in an editor is usually followed by another; a build started on the
/// first one is a build thrown away.
const TICK: Duration = Duration::from_secs(5);
const SETTLE: Duration = Duration::from_secs(10);
/// However much a repository is worked on, nothing of it is built more often than this.
const MIN_BETWEEN: Duration = Duration::from_secs(120);

/// Watches every served repository and runs the checks that named it.
///
/// Nothing here belongs to a browser: this is the difference between a page that shows what
/// the last run said and a machine that has already run it by the time anyone looks. A
/// repository is only watched when a check in the user's own configuration names it, so the
/// default for a repository that was cloned rather than written is that nothing runs in it.
///
/// One run at a time across the whole server, whatever it is serving: fifteen repositories
/// building at once is a machine nobody can use, and the second build would be waiting on the
/// first for the same lock anyway.
///
/// Whether a repository is watched is decided once, when it is first seen: at start, or when
/// it appears in a directory of checkouts, which `may_grow` says can happen.
fn watch_repositories(state: AppState, may_grow: bool) {
    fn is_watched(repo: &RepoState) -> bool {
        let s = repo.session.lock().unwrap_or_else(|e| e.into_inner());
        let root = s.root().to_path_buf();
        s.config
            .analysis
            .iter()
            .any(|c| crate::analysis::watches(c, &root))
    }
    let repos = state.repos.all();
    let mut watched: Vec<bool> = repos.iter().map(|r| is_watched(r)).collect();
    // Nothing to watch, nothing to spawn: the usual case, and it should cost nothing.
    let count = watched.iter().filter(|w| **w).count();
    if count == 0 && !may_grow {
        return;
    }
    if count > 0 {
        println!(
            "watching {count} of {} repositories for checks to run",
            repos.len()
        );
    }
    tokio::task::spawn_blocking(move || {
        // Per repository: the fingerprint last seen, when it last moved, and when each check
        // was last started here. All of it dies with the process, like every other run.
        let mut seen: HashMap<usize, (u64, Instant)> = HashMap::new();
        // When each check last started here, and what the repository looked like then.
        let mut ran: HashMap<(usize, String), (Instant, u64)> = HashMap::new();
        loop {
            std::thread::sleep(TICK);
            let repos = state.repos.all();
            for repo in &repos[watched.len()..] {
                let watch = is_watched(repo);
                if watch {
                    let name = repo
                        .session
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .name();
                    println!("watching {name} for checks to run");
                }
                watched.push(watch);
            }
            for (i, repo) in repos.iter().enumerate() {
                if !watched[i] {
                    continue;
                }
                let (fingerprint, root, checks) = {
                    let s = repo.session.lock().unwrap_or_else(|e| e.into_inner());
                    let root = s.root().to_path_buf();
                    match s.fingerprint(None) {
                        Ok(f) => (f, root, s.config.analysis.clone()),
                        // A repository that cannot be read right now — mid-rebase, mid-clone —
                        // is not a reason to give up watching it.
                        Err(_) => continue,
                    }
                };
                let now = Instant::now();
                let (last, changed_at) = *seen.get(&i).unwrap_or(&(fingerprint, now));
                if last != fingerprint {
                    seen.insert(i, (fingerprint, now));
                    continue;
                }
                seen.insert(i, (fingerprint, changed_at));
                // Still holding the value it settled on, and settled long enough ago.
                if now.duration_since(changed_at) < SETTLE {
                    continue;
                }
                for check in checks.iter().filter(|c| crate::analysis::watches(c, &root)) {
                    let key = (i, check.name.clone());
                    let before = ran.get(&key).copied();
                    if !due(
                        now.duration_since(changed_at),
                        before.map(|(at, _)| now.duration_since(at)),
                        before.map(|(_, f)| f),
                        fingerprint,
                    ) {
                        continue;
                    }
                    // One at a time across the whole server, not merely within one
                    // repository: fifteen builds at once is a machine nobody can use.
                    if anything_running(&state) {
                        continue;
                    }
                    let mut runs = repo.checks.lock().unwrap_or_else(|e| e.into_inner());
                    match crate::analysis::start(check, &root) {
                        Ok(run) => {
                            runs.insert(check.name.clone(), run);
                            ran.insert(key, (now, fingerprint));
                        }
                        Err(e) => eprintln!("cannot start {}: {e:#}", check.name),
                    }
                }
            }
        }
    });
}

/// Whether a watched check is due to run.
///
/// Three things have to hold, and the middle one is the one worth stating: the repository has
/// held still long enough to be worth building, it has *changed since this check last ran in
/// it*, and enough time has passed since that run. Without the middle test the floor between
/// runs becomes a schedule, and a repository nobody has touched for a month is rebuilt every
/// two minutes for ever.
fn due(
    settled_for: Duration,
    since_last_run: Option<Duration>,
    ran_at: Option<u64>,
    fingerprint: u64,
) -> bool {
    settled_for >= SETTLE
        && ran_at != Some(fingerprint)
        && since_last_run.is_none_or(|d| d >= MIN_BETWEEN)
}

/// Whether any check is running anywhere on this server.
fn anything_running(state: &AppState) -> bool {
    state.repos.all().iter().any(|r| {
        r.checks
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .any(|run| run.lock().unwrap_or_else(|e| e.into_inner()).running)
    })
}

/// Ctrl-C, and SIGTERM as well, which is how a service manager asks a server to stop.
async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        match signal(SignalKind::terminate()) {
            Ok(mut term) => {
                tokio::select! {
                    _ = ctrl_c => {}
                    _ = term.recv() => {}
                }
            }
            Err(_) => ctrl_c.await,
        }
    }
    #[cfg(not(unix))]
    ctrl_c.await;
}

/// The session cookie's value, when the request carries one.
fn cookie_token(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(header::COOKIE)?
        .to_str()
        .ok()?
        .split(';')
        .find_map(|pair| {
            let (name, value) = pair.split_once('=')?;
            (name.trim() == COOKIE).then_some(value.trim())
        })
}

fn router(state: AppState) -> Router {
    let api = Router::new()
        .route("/repos", get(api::get_repos))
        .route("/state", get(api::get_state))
        .route("/overview", get(api::get_overview))
        .route("/leans_on", get(api::get_leans_on))
        .route("/tests", get(api::get_tests))
        .route("/spec", get(api::get_spec))
        .route("/e2e", get(api::get_e2e))
        .route("/recording", get(api::get_recording))
        .route("/cast", get(api::get_cast))
        .route("/tests_of", get(api::get_tests_of))
        .route("/file", get(api::get_file))
        .route("/blame", get(api::get_blame))
        .route("/image", get(api::get_image))
        .route("/log", get(api::get_log))
        .route("/commit_files", get(api::get_commit_files))
        .route("/changes", get(api::get_changes))
        .route("/diff", get(api::get_diff))
        .route("/comments", get(api::get_comments).post(api::post_comment))
        .route("/comments/toggle", post(api::post_comment_toggle))
        .route("/comments/edit", post(api::post_comment_edit))
        .route("/comments/delete", post(api::post_comment_delete))
        .route("/notes", get(api::get_notes).post(api::post_note))
        .route("/notes/edit", post(api::post_note_edit))
        .route("/notes/delete", post(api::post_note_delete))
        .route("/commit", post(api::post_commit))
        .route("/analysis", get(api::get_analysis))
        .route("/analysis/run", post(api::post_analysis_run))
        .route("/version", get(api::get_version))
        .route("/refresh", post(api::post_refresh))
        .route("/reanchor", post(api::post_reanchor))
        .route("/config", post(api::post_config))
        .route("/themes", get(api::get_themes))
        .route("/symbols", get(api::get_symbols))
        .route("/definitions", get(api::get_definitions))
        .route("/references", get(api::get_references))
        .route("/symbol_search", get(api::get_symbol_search))
        .route("/identifier", get(api::get_identifier))
        .route("/agent/start", post(agent::post_agent_start))
        .route("/agent/events", get(agent::get_agent_events))
        .route("/agent/prompt", post(agent::post_agent_prompt))
        .route("/agent/permission", post(agent::post_agent_permission))
        .route("/agent/cancel", post(agent::post_agent_cancel))
        .route("/agent/stop", post(agent::post_agent_stop))
        .route("/agent/clear", post(agent::post_agent_clear))
        .route("/logout", post(post_logout))
        .route_layer(middleware::from_fn_with_state(state.clone(), require_token))
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            limit_in_flight,
        ))
        .layer(DefaultBodyLimit::max(MAX_BODY));
    Router::new()
        .route("/", get(index))
        .route("/login", post(post_login))
        .route("/manifest.json", get(manifest))
        .route("/icon.svg", get(icon_svg))
        .route("/icon.png", get(icon_png))
        .nest("/api", api)
        .layer(middleware::from_fn(security_headers))
        .with_state(state)
}

async fn require_token(
    State(state): State<AppState>,
    headers: HeaderMap,
    req: Request,
    next: Next,
) -> Response {
    // The API takes the session's token in a header, never the cookie: a page on another
    // site cannot set that header without a preflight this server never answers, so it
    // cannot act as the signed-in browser.
    let ok = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .is_some_and(|t| state.sessions().accepts(t));
    if ok {
        next.run(req).await
    } else {
        ApiError(StatusCode::FORBIDDEN, "missing or wrong token".into()).into_response()
    }
}

/// Refuses a request outright when the API already has all it can work on, rather than
/// queueing it behind work that is already slow. Counted before the token is checked, so an
/// unauthenticated flood cannot push real requests out of the way either.
async fn limit_in_flight(State(state): State<AppState>, req: Request, next: Next) -> Response {
    let Ok(_permit) = state.in_flight.clone().try_acquire_owned() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            [(header::RETRY_AFTER, "1")],
            "the server is busy; try again",
        )
            .into_response();
    };
    next.run(req).await
}

/// Nothing the server sends may be stored by a proxy or a browser, embedded in a frame, or
/// used as a referrer; the page itself carries the token.
async fn security_headers(req: Request, next: Next) -> Response {
    let mut response = next.run(req).await;
    let headers = response.headers_mut();
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    if let Ok(csp) = HeaderValue::from_str(CSP) {
        headers.insert(header::CONTENT_SECURITY_POLICY, csp);
    }
    response
}

#[derive(Deserialize)]
struct IndexQuery {
    t: Option<String>,
}

/// The page, for a browser that is signed in. A cookie says so; `?t=` earns one on a token
/// run, and the sign-in form earns one on a password run. The redirect afterwards keeps the
/// token out of the address bar, the browser's history, and the log of every later request.
async fn index(
    State(state): State<AppState>,
    Query(q): Query<IndexQuery>,
    headers: HeaderMap,
) -> Response {
    if let Some(api_token) = signed_in(&state, &headers) {
        return Html(page_for(&api_token)).into_response();
    }
    if let (Some(given), Some(want)) = (q.t.as_deref(), state.auth.token()) {
        if constant_eq(given, want) {
            return start_session(&state);
        }
    }
    if state.auth.is_password() {
        return (StatusCode::OK, Html(login_page(None))).into_response();
    }
    (
        StatusCode::FORBIDDEN,
        "open the URL codereview printed, token included",
    )
        .into_response()
}

async fn manifest() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "application/manifest+json")],
        MANIFEST_JSON,
    )
}

async fn icon_svg() -> impl IntoResponse {
    ([(header::CONTENT_TYPE, "image/svg+xml")], ICON_SVG)
}

async fn icon_png() -> impl IntoResponse {
    ([(header::CONTENT_TYPE, "image/png")], ICON_PNG)
}

/// The API token of the session this request's cookie names, if it names a live one.
fn signed_in(state: &AppState, headers: &HeaderMap) -> Option<String> {
    let cookie = cookie_token(headers)?.to_string();
    state.sessions().api_token_for(&cookie)
}

/// Opens a session and sends the browser to the page with the cookie for it.
fn start_session(state: &AppState) -> Response {
    let (id, _) = state.sessions().open();
    let cookie = session_cookie(&id, state.secure_cookie);
    let mut response = Redirect::to("/").into_response();
    if let Ok(value) = HeaderValue::from_str(&cookie) {
        response.headers_mut().insert(header::SET_COOKIE, value);
    }
    response
}

#[derive(Deserialize)]
struct LoginForm {
    password: String,
}

/// The sign-in form's target. Wrong passwords are counted, and once there have been a few
/// the answer is refused for a while, so that the password cannot be worked out by asking
/// repeatedly.
async fn post_login(State(state): State<AppState>, Form(form): Form<LoginForm>) -> Response {
    if !state.auth.is_password() {
        return (StatusCode::NOT_FOUND, "this server has no password").into_response();
    }
    if let Some(wait) = state.sessions().locked_for() {
        let seconds = wait.as_secs() + 1;
        return (
            StatusCode::TOO_MANY_REQUESTS,
            [(header::RETRY_AFTER, seconds.to_string())],
            Html(login_page(Some(&format!(
                "Too many attempts. Try again in {seconds} seconds."
            )))),
        )
            .into_response();
    }
    if !state.auth.password_matches(&form.password) {
        state.sessions().note_failure();
        return (
            StatusCode::UNAUTHORIZED,
            Html(login_page(Some("Wrong password."))),
        )
            .into_response();
    }
    state.sessions().note_success();
    start_session(&state)
}

/// Ends this browser's session. The page asks for it with the session's own token, so no
/// other site can sign anybody out.
async fn post_logout(State(state): State<AppState>, headers: HeaderMap) -> Json<Ok_> {
    if let Some(token) = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
    {
        state.sessions().close(token);
    }
    Json(Ok_ { ok: true })
}

/// The page, carrying this session's API token. The token is put in as a JSON string, so
/// whatever it holds stays one string literal and cannot become script.
fn page_for(api_token: &str) -> String {
    let literal = serde_json::to_string(api_token).unwrap_or_else(|_| "\"\"".into());
    INDEX_HTML.replace("__TOKEN__", &literal)
}

fn login_page(message: Option<&str>) -> String {
    let block = match message {
        Some(text) => format!("<p class=\"error\">{}</p>", escape(text)),
        None => String::new(),
    };
    LOGIN_HTML.replace("__MESSAGE__", &block)
}

/// Enough escaping for the fixed messages the sign-in page shows.
fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// The cookie that keeps this browser signed in. `Secure` only over HTTPS, since a browser
/// throws away a `Secure` cookie that arrives over plain HTTP.
fn session_cookie(id: &str, secure: bool) -> String {
    let mut cookie =
        format!("{COOKIE}={id}; Path=/; HttpOnly; SameSite=Lax; Max-Age={SESSION_MAX_AGE}");
    if secure {
        cookie.push_str("; Secure");
    }
    cookie
}

/// Runs `f` with the agent hub on a blocking thread. Everything the hub does ends in a
/// write to a child process's pipe or a read from it, which blocks: doing that on a runtime
/// thread lets one unresponsive agent stall every request the server has.
async fn with_hub<T, F>(state: &RepoState, f: F) -> std::result::Result<T, ApiError>
where
    T: Send + 'static,
    F: FnOnce(&mut AgentHub) -> Result<T> + Send + 'static,
{
    let hub = state.hub.clone();
    tokio::task::spawn_blocking(move || {
        let mut guard = hub.lock().unwrap_or_else(|e| e.into_inner());
        f(&mut guard)
    })
    .await
    .map_err(|e| ApiError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    .map_err(ApiError::from)
}

/// Runs `f` with the session on a blocking thread; diffs and git calls are not async.
async fn with_session<T, F>(state: &RepoState, f: F) -> std::result::Result<T, ApiError>
where
    T: Send + 'static,
    F: FnOnce(&mut Session) -> Result<T> + Send + 'static,
{
    let session = state.session.clone();
    tokio::task::spawn_blocking(move || {
        // Take the lock back after a panic rather than refusing every later request for
        // this repository: the session is rebuilt from disk by a refresh, and one bad
        // request must not put a repository out of service until a restart.
        let mut guard = session.lock().unwrap_or_else(|e| e.into_inner());
        f(&mut guard)
    })
    .await
    .map_err(|e| ApiError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    .map_err(ApiError::from)
}

#[cfg(test)]
pub(crate) fn test_router(roots: &[&Path], auth: Auth) -> Router {
    router(test_state(roots, auth))
}

#[cfg(test)]
fn test_state(roots: &[&Path], auth: Auth) -> AppState {
    AppState {
        repos: Arc::new(Repos::new(
            roots
                .iter()
                .map(|r| crate::session::tests::scratch_session(r)),
        )),
        auth: Arc::new(auth),
        sessions: Arc::new(Mutex::new(Sessions::default())),
        secure_cookie: false,
        in_flight: Arc::new(tokio::sync::Semaphore::new(MAX_IN_FLIGHT)),
        // No thread builds indexes for a test server; a request that needs one builds it.
        warming: Arc::new(std::sync::atomic::AtomicBool::new(false)),
    }
}

#[cfg(not(test))]
#[allow(dead_code)]
fn _keep_path_import(_: &Path) {}
