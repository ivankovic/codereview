/*  This file is part of codereview.
 *
 *  Copyright (C) 2026 Marko Ivankovic
 *
 *  This program is free software: you can redistribute it and/or modify
 *  it under the terms of the GNU Affero General Public License as published
 *  by the Free Software Foundation, version 3 of the License.
 *
 *  This program is distributed in the hope that it will be useful,
 *  but WITHOUT ANY WARRANTY; without even the implied warranty of
 *  MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
 *  GNU Affero General Public License for more details.
 *
 *  You should have received a copy of the GNU Affero General Public License
 *  along with this program.  If not, see <https://www.gnu.org/licenses/>.
 */

//! Pull requests from the forge a repository lives on, through the forge's own command-line
//! tool, the way git is reached through `git`. GitHub is `gh`; another forge is another
//! implementation of [`Forge`]. Nothing is cached: every question is asked again, so the
//! server keeps no state of its own. A forge's comments are read, never written.

use std::path::PathBuf;
use std::process::Command;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::repo::Repo;
use crate::spec::Stretch;

/// One pull request, as the front ends show it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PullRequest {
    pub number: u64,
    pub title: String,
    pub author: String,
    /// `open`, `closed` or `merged`.
    pub state: String,
    pub draft: bool,
    /// The branch it goes into, and the commit that branch was at when last asked.
    pub base_ref: String,
    pub base_hash: String,
    /// The branch it comes from, and its tip.
    pub head_ref: String,
    pub head_hash: String,
    /// Where it is on the forge.
    pub url: String,
    /// When it last changed, as the forge writes it (ISO 8601).
    pub updated: String,
}

/// A comment on a pull request: on a line of a file when `path` is set, on the whole pull
/// request otherwise.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForgeComment {
    pub author: String,
    pub body: String,
    pub path: Option<String>,
    /// The line in the pull request's head the comment is on, where the forge still has one.
    pub line: Option<u32>,
    pub created: String,
    pub url: String,
}

/// What the tool asks of a forge.
pub trait Forge: Send + Sync {
    /// The forge's name, for a heading: "GitHub".
    fn name(&self) -> &str;
    /// The open pull requests, most recently changed first.
    fn pull_requests(&self) -> Result<Vec<PullRequest>>;
    /// One pull request, open or not.
    fn pull_request(&self, number: u64) -> Result<PullRequest>;
    /// Every comment on one pull request, oldest first.
    fn comments(&self, number: u64) -> Result<Vec<ForgeComment>>;
    /// The refspecs that fetch a pull request's commits from `origin`.
    fn refspecs(&self, pr: &PullRequest) -> Vec<String>;
}

/// The forge `repo`'s `origin` is on, when the tool knows it. Whether its command-line tool
/// is installed and signed in is found out by asking, and said in the error of the first
/// question.
pub fn for_repo(repo: &Repo) -> Option<Box<dyn Forge>> {
    let name = repo.github_name()?;
    Some(Box::new(GitHub::new(gh_program(), name)))
}

/// The `gh` to run: `CODEREVIEW_GH` when set, which the tests point at a stand-in, else `gh`
/// from the path.
fn gh_program() -> PathBuf {
    std::env::var_os("CODEREVIEW_GH")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("gh"))
}

/// A pull request as a review range, its commits fetched first when the repository does not
/// have them, so that every view of a stretch works on it.
pub fn review_range(repo: &Repo, forge: &dyn Forge, pr: &PullRequest) -> Result<Stretch> {
    if !repo.has_commit(&pr.head_hash) || !repo.has_commit(&pr.base_hash) {
        let specs = forge.refspecs(pr);
        let specs: Vec<&str> = specs.iter().map(String::as_str).collect();
        repo.fetch_objects(&specs)
            .with_context(|| format!("cannot fetch pull request #{}", pr.number))?;
    }
    Ok(Stretch::Range {
        base: pr.base_hash.clone(),
        head: pr.head_hash.clone(),
        pull: Some(pr.number),
        label: None,
    })
}

/// GitHub, through `gh api`, which answers in the REST API's own JSON and uses whatever
/// account `gh` is signed in to.
pub struct GitHub {
    program: PathBuf,
    /// `owner/name`.
    repository: String,
}

impl GitHub {
    pub fn new(program: PathBuf, repository: String) -> Self {
        Self {
            program,
            repository,
        }
    }

    fn api(&self, path: &str) -> Result<Value> {
        let output = Command::new(&self.program)
            .args(["api", "--paginate", "--slurp", path])
            .env("GH_PROMPT_DISABLED", "1")
            .env("NO_COLOR", "1")
            .output()
            .with_context(|| format!("cannot run {}", self.program.display()))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!(
                "gh api {path} failed: {}",
                stderr.trim().lines().next().unwrap_or("no message")
            );
        }
        let pages: Value = serde_json::from_slice(&output.stdout)
            .context("gh answered with something not JSON")?;
        // `--slurp` wraps every page in an array; a list endpoint's pages are arrays too.
        Ok(match pages {
            Value::Array(pages) if pages.iter().all(Value::is_array) => Value::Array(
                pages
                    .into_iter()
                    .flat_map(|p| match p {
                        Value::Array(items) => items,
                        _ => Vec::new(),
                    })
                    .collect(),
            ),
            Value::Array(mut pages) if pages.len() == 1 => pages.remove(0),
            other => other,
        })
    }
}

impl Forge for GitHub {
    fn name(&self) -> &str {
        "GitHub"
    }

    fn pull_requests(&self) -> Result<Vec<PullRequest>> {
        let path = format!(
            "repos/{}/pulls?state=open&sort=updated&direction=desc&per_page=100",
            self.repository
        );
        let list = self.api(&path)?;
        list.as_array()
            .context("gh did not answer with a list of pull requests")?
            .iter()
            .map(parse_pull)
            .collect()
    }

    fn pull_request(&self, number: u64) -> Result<PullRequest> {
        parse_pull(&self.api(&format!("repos/{}/pulls/{number}", self.repository))?)
    }

    fn comments(&self, number: u64) -> Result<Vec<ForgeComment>> {
        let on_lines = self.api(&format!(
            "repos/{}/pulls/{number}/comments?per_page=100",
            self.repository
        ))?;
        let on_whole = self.api(&format!(
            "repos/{}/issues/{number}/comments?per_page=100",
            self.repository
        ))?;
        let mut comments: Vec<ForgeComment> = [on_lines, on_whole]
            .iter()
            .filter_map(Value::as_array)
            .flatten()
            .map(parse_comment)
            .collect();
        comments.sort_by(|a, b| a.created.cmp(&b.created));
        Ok(comments)
    }

    fn refspecs(&self, pr: &PullRequest) -> Vec<String> {
        vec![
            format!("pull/{}/head", pr.number),
            format!("refs/heads/{}", pr.base_ref),
        ]
    }
}

fn text(value: &Value, path: &[&str]) -> String {
    path.iter()
        .try_fold(value, |v, key| v.get(key))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// One pull request from the REST API's JSON.
pub fn parse_pull(value: &Value) -> Result<PullRequest> {
    let number = value
        .get("number")
        .and_then(Value::as_u64)
        .context("a pull request without a number")?;
    let merged = value.get("merged_at").is_some_and(|m| !m.is_null());
    let state = if merged {
        "merged".to_string()
    } else {
        text(value, &["state"])
    };
    let pr = PullRequest {
        number,
        title: text(value, &["title"]),
        author: text(value, &["user", "login"]),
        state,
        draft: value.get("draft").and_then(Value::as_bool).unwrap_or(false),
        base_ref: text(value, &["base", "ref"]),
        base_hash: text(value, &["base", "sha"]),
        head_ref: text(value, &["head", "ref"]),
        head_hash: text(value, &["head", "sha"]),
        url: text(value, &["html_url"]),
        updated: text(value, &["updated_at"]),
    };
    let hex = |h: &str| !h.is_empty() && h.chars().all(|c| c.is_ascii_hexdigit());
    if !hex(&pr.base_hash) || !hex(&pr.head_hash) {
        bail!("pull request #{number} has no commits to compare");
    }
    Ok(pr)
}

/// One comment from the REST API's JSON, on a line or on the whole pull request.
pub fn parse_comment(value: &Value) -> ForgeComment {
    let path = value
        .get("path")
        .and_then(Value::as_str)
        .map(str::to_string);
    let line = value
        .get("line")
        .and_then(Value::as_u64)
        .and_then(|l| u32::try_from(l).ok());
    ForgeComment {
        author: text(value, &["user", "login"]),
        body: text(value, &["body"]),
        path,
        line,
        created: text(value, &["created_at"]),
        url: text(value, &["html_url"]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn pull(number: u64) -> Value {
        json!({
            "number": number,
            "title": "Comments survive a rename",
            "user": {"login": "ana"},
            "state": "open",
            "draft": false,
            "merged_at": null,
            "base": {"ref": "main", "sha": "aaaaaaa"},
            "head": {"ref": "rename", "sha": "bbbbbbb"},
            "html_url": format!("https://github.com/o/n/pull/{number}"),
            "updated_at": "2026-11-20T10:00:00Z",
        })
    }

    #[test]
    fn a_pull_request_is_read_from_the_rest_api() {
        let pr = parse_pull(&pull(7)).unwrap();
        assert_eq!(pr.number, 7);
        assert_eq!(pr.author, "ana");
        assert_eq!(
            (pr.base_ref.as_str(), pr.head_ref.as_str()),
            ("main", "rename")
        );
        assert_eq!(pr.state, "open");
    }

    #[test]
    fn a_merged_pull_request_says_so() {
        let mut value = pull(7);
        value["state"] = json!("closed");
        value["merged_at"] = json!("2026-11-21T00:00:00Z");
        assert_eq!(parse_pull(&value).unwrap().state, "merged");
    }

    #[test]
    fn a_pull_request_without_commits_is_refused() {
        let mut value = pull(7);
        value["head"]["sha"] = json!("--upload-pack=x");
        assert!(parse_pull(&value).is_err());
    }

    #[test]
    fn a_comment_on_a_line_keeps_its_place() {
        let c = parse_comment(&json!({
            "user": {"login": "ana"}, "body": "why?", "path": "src/lib.rs", "line": 12,
            "created_at": "2026-11-20T10:00:00Z", "html_url": "u",
        }));
        assert_eq!((c.path.as_deref(), c.line), (Some("src/lib.rs"), Some(12)));
        let whole = parse_comment(&json!({"user": {"login": "bo"}, "body": "LGTM"}));
        assert_eq!((whole.path, whole.line), (None, None));
    }

    /// A stand-in for `gh` that answers every `api` call from a file named after the endpoint.
    #[cfg(unix)]
    fn fake_gh(dir: &std::path::Path) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let script = dir.join("gh");
        std::fs::write(
            &script,
            "#!/bin/sh\n# gh api --paginate --slurp PATH\n\
             key=$(printf '%s' \"$4\" | sed 's/?.*//; s|/|_|g')\n\
             cat \"$(dirname \"$0\")/$key.json\"\n",
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        script
    }

    #[cfg(unix)]
    #[test]
    fn pull_requests_and_their_comments_come_from_gh() {
        let dir = tempfile::tempdir().unwrap();
        let gh = fake_gh(dir.path());
        let write = |name: &str, value: Value| {
            std::fs::write(dir.path().join(name), value.to_string()).unwrap();
        };
        write("repos_o_n_pulls.json", json!([[pull(7), pull(9)]]));
        write("repos_o_n_pulls_7.json", json!([pull(7)]));
        write(
            "repos_o_n_pulls_7_comments.json",
            json!([[{"user": {"login": "bo"}, "body": "later", "path": "a.rs", "line": 3,
                     "created_at": "2026-11-20T12:00:00Z"}]]),
        );
        write(
            "repos_o_n_issues_7_comments.json",
            json!([[{"user": {"login": "ana"}, "body": "first",
                     "created_at": "2026-11-20T11:00:00Z"}]]),
        );
        let forge = GitHub::new(gh, "o/n".into());
        let list = forge.pull_requests().unwrap();
        assert_eq!(list.iter().map(|p| p.number).collect::<Vec<_>>(), [7, 9]);
        assert_eq!(
            forge.pull_request(7).unwrap().title,
            "Comments survive a rename"
        );
        let comments = forge.comments(7).unwrap();
        assert_eq!(
            comments.iter().map(|c| c.body.as_str()).collect::<Vec<_>>(),
            ["first", "later"]
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_failing_gh_says_what_it_said() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let gh = dir.path().join("gh");
        std::fs::write(&gh, "#!/bin/sh\necho 'gh auth login first' >&2\nexit 4\n").unwrap();
        std::fs::set_permissions(&gh, std::fs::Permissions::from_mode(0o755)).unwrap();
        let err = GitHub::new(gh, "o/n".into()).pull_requests().unwrap_err();
        assert!(err.to_string().contains("gh auth login first"), "{err}");
    }

    /// A forge that only knows where one pull request's commits are.
    struct Fetching(Vec<String>);

    impl Forge for Fetching {
        fn name(&self) -> &str {
            "test"
        }
        fn pull_requests(&self) -> Result<Vec<PullRequest>> {
            Ok(Vec::new())
        }
        fn pull_request(&self, _: u64) -> Result<PullRequest> {
            bail!("none")
        }
        fn comments(&self, _: u64) -> Result<Vec<ForgeComment>> {
            Ok(Vec::new())
        }
        fn refspecs(&self, _: &PullRequest) -> Vec<String> {
            self.0.clone()
        }
    }

    #[test]
    fn a_pull_request_the_clone_lacks_is_fetched_without_moving_a_branch() {
        let dir = tempfile::tempdir().unwrap();
        let run = |cwd: &std::path::Path, args: &[&str]| -> String {
            let out = Command::new("git")
                .current_dir(cwd)
                .args(args)
                .output()
                .unwrap();
            assert!(out.status.success(), "{args:?}: {out:?}");
            String::from_utf8_lossy(&out.stdout).trim().to_string()
        };
        let origin = dir.path().join("origin");
        std::fs::create_dir(&origin).unwrap();
        run(&origin, &["init", "-q", "-b", "main"]);
        run(&origin, &["config", "user.name", "T"]);
        run(&origin, &["config", "user.email", "t@example.com"]);
        std::fs::write(origin.join("a.txt"), "a\n").unwrap();
        run(&origin, &["add", "."]);
        run(&origin, &["commit", "-q", "-m", "first"]);
        run(dir.path(), &["clone", "-q", "origin", "clone"]);
        let clone = dir.path().join("clone");
        run(&origin, &["switch", "-q", "-c", "feature"]);
        std::fs::write(origin.join("a.txt"), "b\n").unwrap();
        run(&origin, &["commit", "-q", "-am", "change"]);
        let base = run(&origin, &["rev-parse", "main"]);
        let head = run(&origin, &["rev-parse", "feature"]);

        let repo = Repo::open(&clone).unwrap();
        assert!(!repo.has_commit(&head));
        let refs_before = run(&clone, &["for-each-ref"]);
        let mut value = pull(7);
        value["base"]["sha"] = serde_json::json!(base);
        value["head"]["sha"] = serde_json::json!(head);
        let pr = parse_pull(&value).unwrap();
        let forge = Fetching(vec!["refs/heads/feature".into()]);
        let range = review_range(&repo, &forge, &pr).unwrap();
        assert!(repo.has_commit(&head));
        assert!(repo.fetch_objects(&["feature:refs/heads/x"]).is_err());
        assert_eq!(run(&clone, &["for-each-ref"]), refs_before, "no ref moved");
        assert_eq!(
            range,
            Stretch::Range {
                base,
                head,
                pull: Some(7),
                label: None
            }
        );
    }
}
