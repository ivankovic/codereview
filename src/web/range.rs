//! The review range, for the page: what can be reviewed — a worktree as it is on disk, a
//! branch, a tag, a forge's pull request — what to compare it with, and one range's files and
//! comments. The choices are `targets`', the same the terminal UI offers. A forge is a network
//! away, so it is asked without holding the session's lock, which every other request for the
//! repository waits on.

use super::*;
use crate::forge::{ForgeComment, PullRequest};
use crate::repo::Repo;
use crate::spec::Stretch;
use crate::targets::{Base, Head, Heads};

#[derive(Serialize)]
pub(super) struct Choices {
    #[serde(flatten)]
    heads: Heads,
    /// Whether `origin` is on a forge the tool can ask for pull requests.
    forge: Option<String>,
}

/// What can be reviewed, other than the stretches the page knows and the forge's pull
/// requests, which `/pulls` asks for.
pub(super) async fn get_choices(state: Ctx) -> ApiResult<Choices> {
    let r = with_session(&state, |s| {
        let repo = &s.repo;
        Ok(Choices {
            heads: crate::targets::heads(repo)?,
            forge: crate::forge::for_repo(repo).map(|f| f.name().to_string()),
        })
    })
    .await?;
    Ok(Json(r))
}

/// What a head can be compared with, the likeliest first.
pub(super) async fn post_bases(state: Ctx, Json(head): Json<Head>) -> ApiResult<Vec<Base>> {
    let r = with_session(&state, move |s| crate::targets::bases(&s.repo, &head)).await?;
    Ok(Json(r))
}

#[derive(Deserialize)]
pub(super) struct Pick {
    head: Head,
    base: String,
}

#[derive(Serialize)]
pub(super) struct Picked {
    base: String,
    /// The head's revision: a worktree's is the snapshot of it taken now.
    head: String,
    /// What to call it, where its revision says nothing.
    label: Option<String>,
    title: String,
}

/// A head and a base, made a review range: a worktree taken as it is on disk now.
pub(super) async fn post_pick(state: Ctx, Json(p): Json<Pick>) -> ApiResult<Picked> {
    let r = with_session(&state, move |s| {
        let stretch = crate::targets::range(&s.repo, &p.head, &p.base)?;
        let title = stretch.title();
        let Stretch::Range {
            base, head, label, ..
        } = stretch
        else {
            anyhow::bail!("not a range");
        };
        Ok(Picked {
            base,
            head,
            label,
            title,
        })
    })
    .await?;
    Ok(Json(r))
}

/// The repository's root, read under the lock, for work that must not hold it.
async fn root_of(state: &Ctx) -> std::result::Result<std::path::PathBuf, ApiError> {
    with_session(state, |s| Ok(s.root().to_path_buf())).await
}

/// Runs `f` on a blocking thread against the repository at `root`, without the session.
async fn off_the_lock<T, F>(root: std::path::PathBuf, f: F) -> std::result::Result<T, ApiError>
where
    T: Send + 'static,
    F: FnOnce(&Repo) -> Result<T> + Send + 'static,
{
    tokio::task::spawn_blocking(move || f(&Repo::open(&root)?))
        .await
        .map_err(|e| ApiError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .map_err(|e| ApiError(StatusCode::BAD_GATEWAY, format!("{e:#}")))
}

/// The forge's open pull requests, most recently changed first.
pub(super) async fn get_pulls(state: Ctx) -> ApiResult<Vec<PullRequest>> {
    let root = root_of(&state).await?;
    let pulls = off_the_lock(root, |repo| {
        crate::forge::for_repo(repo)
            .context("origin is not on GitHub, so there are none to ask for")?
            .pull_requests()
    })
    .await?;
    Ok(Json(pulls))
}

#[derive(Deserialize)]
pub(super) struct PullQuery {
    number: u64,
}

#[derive(Serialize)]
pub(super) struct Pull {
    pull: PullRequest,
    /// The two sides to review it as: its base's commit and its head's, both here now.
    base: String,
    head: String,
    comments: Vec<ForgeComment>,
    /// Why the comments could not be read, when they could not.
    comments_error: Option<String>,
}

/// One pull request, made ready to review: its commits fetched when the repository lacks
/// them, and its comments, read and never written.
pub(super) async fn post_pull(state: Ctx, Json(q): Json<PullQuery>) -> ApiResult<Pull> {
    let root = root_of(&state).await?;
    let pull = off_the_lock(root, move |repo| {
        let forge = crate::forge::for_repo(repo).context("origin is not on GitHub")?;
        let pr = forge.pull_request(q.number)?;
        let Stretch::Range { base, head, .. } =
            crate::forge::review_range(repo, forge.as_ref(), &pr)?
        else {
            anyhow::bail!("pull request #{} is not a range", q.number);
        };
        let (comments, comments_error) = match forge.comments(pr.number) {
            Ok(c) => (c, None),
            Err(e) => (Vec::new(), Some(format!("{e:#}"))),
        };
        Ok(Pull {
            pull: pr,
            base,
            head,
            comments,
            comments_error,
        })
    })
    .await?;
    Ok(Json(pull))
}

#[derive(Deserialize)]
pub(super) struct RangeQuery {
    base: String,
    head: String,
    #[serde(default)]
    pull: Option<u64>,
    /// What the head is called, where its revision says nothing.
    #[serde(default)]
    label: Option<String>,
}

#[derive(Serialize)]
pub(super) struct RangeFiles {
    /// What it covers, for a heading.
    label: String,
    /// Where its diffs start: the commit the two sides last had in common.
    from: String,
    /// The head's commit.
    to: String,
    files: Vec<ChangedFile>,
}

/// The files a pull request or a comparison changes, from where its two sides parted.
pub(super) async fn get_range(state: Ctx, Query(q): Query<RangeQuery>) -> ApiResult<RangeFiles> {
    let r = with_session(&state, move |s| {
        let stretch = Stretch::Range {
            base: q.base,
            head: q.head,
            pull: q.pull,
            label: q.label,
        };
        let span = crate::spec::span(s, &stretch)?;
        let to = span.to.clone().unwrap_or_default();
        let files = s.repo.diff_files(&span.base, &to)?;
        Ok(RangeFiles {
            label: span.label,
            from: span.base,
            to,
            files,
        })
    })
    .await?;
    Ok(Json(r))
}
