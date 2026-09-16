//! The agent behind the browser UI: one per repository, the transcript it is building, and
//! the routes that drive it. Everything here runs on a blocking thread, because talking to an
//! agent means writing to a pipe that may not be read for a while.

use super::*;

// ----- agent ---------------------------------------------------------------------------------

#[derive(Serialize)]
pub(super) struct AgentStatus {
    running: bool,
    name: String,
    status: String,
    command: String,
    /// What the turn is doing and what it has cost, already worded; the page shows them.
    progress: Option<String>,
    usage: Option<String>,
    permission: Option<AgentPermission>,
    /// Transcript entries written or changed since the `since` the page sent, each with the
    /// position it belongs at. Entries are not append-only: a tool call is filled in as it
    /// runs.
    entries: Vec<(usize, Entry)>,
    /// What to send as `since` next time, and how many entries there are in all: after a
    /// trim the transcript is shorter, and the page has to drop what is no longer there.
    version: u64,
    count: usize,
}

#[derive(Serialize)]
pub(super) struct AgentPermission {
    /// What the request is about, in one line.
    title: String,
    /// Everything it would approve, in full.
    details: Option<String>,
    options: Vec<PermissionOption>,
    /// Which request this is, so an answer cannot be applied to a later one.
    request_id: String,
}

fn status_of(hub: &mut AgentHub, since: u64, command: &str) -> AgentStatus {
    hub.drain();
    let busy = hub.agent.as_ref().is_some_and(Agent::busy);
    AgentStatus {
        running: hub.agent.is_some(),
        name: hub.name.clone(),
        status: hub.transcript.status.clone(),
        command: command.to_string(),
        progress: hub.transcript.progress(busy || hub.starting, None),
        usage: hub.transcript.usage(),
        permission: hub.transcript.permission.as_ref().map(|p| AgentPermission {
            title: p.title.clone(),
            details: p.details.clone(),
            options: p.options.clone(),
            request_id: p.id.as_str().unwrap_or_default().to_string(),
        }),
        entries: hub
            .transcript
            .since(since)
            .into_iter()
            .map(|(i, e)| (i, e.clone()))
            .collect(),
        version: hub.transcript.version(),
        count: hub.transcript.entries().len(),
    }
}

fn agent_command(state: &RepoState) -> String {
    state.agent_label.clone()
}

/// Starts the agent when it is not running. Blocks for the handshake, which can take a while
/// when `npx` has to fetch an adapter.
pub(super) async fn post_agent_start(state: Ctx) -> ApiResult<AgentStatus> {
    let command = agent_command(&state);
    let session = state.session.clone();
    // Claiming the start under the lock keeps two browsers from starting two agents, where
    // the second would drop and kill the first.
    let mine = with_hub(&state, |hub| {
        let free = hub.agent.is_none() && !hub.starting;
        if free {
            hub.starting = true;
            hub.transcript.status = "starting".into();
        }
        Ok(free)
    })
    .await?;
    if mine {
        let hub = state.hub.clone();
        let outcome = tokio::task::spawn_blocking(move || -> Result<()> {
            let (agent_config, root) = {
                let s = session.lock().unwrap_or_else(|e| e.into_inner());
                (s.config.agent.clone(), s.root().to_path_buf())
            };
            let started = crate::agent::spawn(&agent_config, &root);
            let mut hub = hub.lock().unwrap_or_else(|e| e.into_inner());
            hub.starting = false;
            match started {
                Ok(agent) => {
                    hub.name = agent.name().to_string();
                    hub.agent = Some(agent);
                    hub.transcript.status = "idle".into();
                    Ok(())
                }
                Err(e) => {
                    hub.transcript.status = format!("failed to start: {e:#}");
                    Err(e)
                }
            }
        })
        .await
        .map_err(|e| ApiError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        outcome?;
    }
    let status = with_hub(&state, move |hub| Ok(status_of(hub, u64::MAX, &command))).await?;
    Ok(Json(status))
}

#[derive(Deserialize)]
pub(super) struct SinceQuery {
    #[serde(default)]
    since: u64,
}

pub(super) async fn get_agent_events(
    state: Ctx,
    Query(q): Query<SinceQuery>,
) -> ApiResult<AgentStatus> {
    let command = agent_command(&state);
    let status = with_hub(&state, move |hub| Ok(status_of(hub, q.since, &command))).await?;
    Ok(Json(status))
}

#[derive(Deserialize)]
pub(super) struct ContextItem {
    uri: String,
    text: String,
}

/// Either free text, or a preset the server words: `address_comment` (with `comment`),
/// `address_all`, `review_diff` (with `path` and `target`), `about_code` (with `path`,
/// `text`, and `line`/`end_line` unless the question is about the whole path).
#[derive(Deserialize)]
pub(super) struct AgentPromptRequest {
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
pub(super) struct AgentPromptResponse {
    text: String,
}

pub(super) async fn post_agent_prompt(
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
    let sent = text.clone();
    with_hub(&state, move |hub| {
        let Some(agent) = &mut hub.agent else {
            return Err(Conflict("the agent is not running; start it first".into()).into());
        };
        agent.prompt(&sent, &context)?;
        // The prompt is part of the conversation, written down the same way the agent's
        // words are, so a page that reloads sees it in place.
        hub.transcript.apply(AgentEvent::Text {
            role: crate::agent::Role::User,
            text: sent,
        });
        hub.transcript.begin_turn();
        hub.trim();
        Ok(())
    })
    .await?;
    Ok(Json(AgentPromptResponse { text }))
}

#[derive(Deserialize)]
pub(super) struct PermissionAnswer {
    option_id: Option<String>,
    /// Which request is being answered. A page that has not seen the current request, or
    /// that answers a stale one, must not have its answer applied to another.
    request_id: Option<String>,
}

pub(super) async fn post_agent_permission(
    state: Ctx,
    Json(body): Json<PermissionAnswer>,
) -> ApiResult<Ok_> {
    with_hub(&state, move |hub| {
        let Some(pending) = hub.transcript.permission.as_ref() else {
            return Err(Conflict("no permission request is pending".into()).into());
        };
        if let Some(answering) = &body.request_id
            && pending.id.as_str() != Some(answering.as_str())
        {
            return Err(Conflict(format!(
                "that answer is for another request; {} is the one waiting",
                pending.id.as_str().unwrap_or("another")
            ))
            .into());
        }
        // An option the request never offered is not an answer; the backend treats anything
        // it does not recognise as a refusal, which is the safe reading.
        let chosen = body
            .option_id
            .as_deref()
            .filter(|id| pending.options.iter().any(|o| o.option_id == *id));
        let id = pending.id.clone();
        let Some(agent) = &mut hub.agent else {
            return Err(Conflict("the agent is not running".into()).into());
        };
        // Only once the answer is away is the request no longer waiting: a failed write
        // would otherwise leave the agent waiting for an answer nobody can give again.
        agent.respond_permission(&id, chosen)?;
        hub.transcript.permission = None;
        hub.transcript.status = "working".into();
        Ok(())
    })
    .await?;
    Ok(Json(Ok_ { ok: true }))
}

pub(super) async fn post_agent_cancel(state: Ctx) -> ApiResult<Ok_> {
    with_hub(&state, |hub| {
        if let Some(agent) = &mut hub.agent {
            agent.cancel()?;
        }
        Ok(())
    })
    .await?;
    Ok(Json(Ok_ { ok: true }))
}

/// Empties the transcript. The page asks rather than forgetting on its own, so that the
/// two ends agree on what there is; a reload would otherwise bring it all back.
pub(super) async fn post_agent_clear(state: Ctx) -> ApiResult<Ok_> {
    with_hub(&state, |hub| {
        hub.transcript.clear();
        Ok(())
    })
    .await?;
    Ok(Json(Ok_ { ok: true }))
}

pub(super) async fn post_agent_stop(state: Ctx) -> ApiResult<Ok_> {
    // Dropping the agent kills its process, which waits for it: another blocking call.
    with_hub(&state, |hub| {
        hub.agent = None;
        hub.transcript.permission = None;
        hub.transcript.status = "stopped".into();
        Ok(())
    })
    .await?;
    Ok(Json(Ok_ { ok: true }))
}
