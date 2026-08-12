use std::{fs, time::Duration};

use anyhow::{Context, Result, bail};
use base64::{Engine, engine::general_purpose::STANDARD};
use reqwest::{
    Method, Url,
    blocking::{Client, RequestBuilder},
};
use serde_json::{Value, json};

use super::command::BridgeConfig;

enum Payload<'a> {
    Json(&'a Value),
    Form(&'a [(String, String)]),
}

struct Api {
    client: Client,
}

impl Api {
    fn new() -> Result<Self> {
        let builder = Client::builder()
            .timeout(Duration::from_secs(60))
            .user_agent("kithara-git-bridge/1");
        Ok(Self {
            client: builder.build().context("building bridge HTTP client")?,
        })
    }

    fn request(
        &self,
        method: &Method,
        url: &Url,
        authorization: (&str, &str),
        payload: Option<&Payload<'_>>,
    ) -> Result<Value> {
        let mut request = self
            .client
            .request(method.clone(), url.clone())
            .header(authorization.0, authorization.1);
        request = match payload {
            Some(Payload::Json(body)) => request.json(body),
            Some(Payload::Form(form)) => request.form(form),
            None => request,
        };
        response_json(request, method, url)
    }
}

fn response_json(request: RequestBuilder, method: &Method, url: &Url) -> Result<Value> {
    let response = request
        .send()
        .with_context(|| format!("{method} {url} failed"))?;
    let status = response.status();
    let body = response
        .text()
        .with_context(|| format!("reading {method} {url} response"))?;
    if !status.is_success() {
        let detail = body.chars().take(4_096).collect::<String>();
        bail!("{method} {url} failed with HTTP {status}: {detail}");
    }
    if body.trim().is_empty() {
        Ok(Value::Null)
    } else {
        serde_json::from_str(&body).with_context(|| format!("parsing {method} {url} response"))
    }
}

pub(super) struct Github {
    config: BridgeConfig,
    api: Api,
    token: String,
}

/// The name the verification wears on a pull request. Commit statuses are keyed
/// by it, so a later report replaces the earlier one.
const STATUS_CONTEXT: &str = "kithara/gitlab-verification";

impl Github {
    pub(super) fn new(config: &BridgeConfig) -> Result<Self> {
        let token = read_secret(&config.github_token_file, "GitHub token")?;
        Ok(Self {
            config: config.clone(),
            api: Api::new()?,
            token,
        })
    }

    pub(super) fn head(&self) -> Result<String> {
        let response = self.request(
            &Method::GET,
            &format!(
                "/repos/{}/git/ref/heads/{}",
                self.config.github_repo, self.config.branch
            ),
            None,
        )?;
        string_field(&response, &["object", "sha"], "GitHub branch SHA")
    }

    pub(super) fn merged_pull_request(&self, sha: &str) -> Result<Option<u64>> {
        let response = self.request(
            &Method::GET,
            &format!("/repos/{}/commits/{sha}/pulls", self.config.github_repo),
            None,
        )?;
        let pulls = response
            .as_array()
            .context("GitHub commit pulls response was not an array")?;
        Ok(pulls.iter().find_map(|pull| {
            let merged = pull.get("merged_at").is_some_and(|value| !value.is_null());
            let branch = pull
                .pointer("/base/ref")
                .and_then(Value::as_str)
                .is_some_and(|branch| branch == self.config.branch);
            (merged && branch)
                .then(|| pull.get("number").and_then(Value::as_u64))
                .flatten()
        }))
    }

    /// Check runs are a GitHub App API, and this bridge authenticates with a
    /// token. A commit status draws the same mark on the pull request and is
    /// keyed by its context rather than by an id, so nothing has to be carried
    /// between the two calls.
    pub(super) fn report_status(&self, sha: &str, state: &str, description: &str) -> Result<()> {
        self.request(
            &Method::POST,
            &format!("/repos/{}/statuses/{sha}", self.config.github_repo),
            Some(&json!({
                "state": state,
                "context": STATUS_CONTEXT,
                "description": status_description(description),
            })),
        )?;
        Ok(())
    }

    pub(super) fn git_header(&self) -> String {
        let basic = STANDARD.encode(format!("x-access-token:{}", self.token));
        format!("Authorization: Basic {basic}")
    }

    fn request(&self, method: &Method, path: &str, body: Option<&Value>) -> Result<Value> {
        let url = Url::parse(&format!("https://api.github.com{path}"))
            .with_context(|| format!("building GitHub API URL for {path}"))?;
        let payload = body.map(Payload::Json);
        self.api.request(
            method,
            &url,
            ("Authorization", &format!("Bearer {}", self.token)),
            payload.as_ref(),
        )
    }
}

pub(super) struct Gitlab {
    config: BridgeConfig,
    api: Api,
    token: String,
}

impl Gitlab {
    pub(super) fn new(config: &BridgeConfig) -> Result<Self> {
        let token = read_secret(&config.gitlab_token_file, "GitLab token")?;
        Ok(Self {
            config: config.clone(),
            api: Api::new()?,
            token,
        })
    }

    pub(super) fn head(&self) -> Result<String> {
        let response = self.request(
            &Method::GET,
            &format!("repository/branches/{}", self.config.branch),
            None,
        )?;
        string_field(&response, &["commit", "id"], "GitLab branch SHA")
    }

    pub(super) fn create_pipeline(&self, reference: &str, github_sha: &str) -> Result<u64> {
        let form = vec![
            ("ref".into(), reference.into()),
            ("variables[0][key]".into(), "KITHARA_QUARANTINE_SHA".into()),
            ("variables[0][value]".into(), github_sha.into()),
        ];
        let payload = Payload::Form(&form);
        let response = self.request(&Method::POST, "pipeline", Some(&payload))?;
        response["id"]
            .as_u64()
            .context("GitLab pipeline response has no numeric id")
    }

    /// The status of the work, which is not the status of the pipeline that was
    /// asked for: this project's pipelines are parents whose lanes run in a
    /// child, and a parent reports `success` over a child that was cancelled.
    /// A parent that has succeeded therefore has to be asked about its child
    /// before its answer means anything.
    pub(super) fn pipeline_status(&self, pipeline_id: u64) -> Result<String> {
        let response = self.request(&Method::GET, &format!("pipelines/{pipeline_id}"), None)?;
        let status = response["status"]
            .as_str()
            .map(str::to_string)
            .context("GitLab pipeline response has no status")?;
        if status != "success" {
            return Ok(status);
        }
        let bridges = self.request(
            &Method::GET,
            &format!("pipelines/{pipeline_id}/bridges"),
            None,
        )?;
        Ok(blocking_downstream_status(&bridges)?.unwrap_or(status))
    }

    pub(super) fn ensure_issue(&self, title: &str, description: &str) -> Result<()> {
        let mut url = self.project_url("issues")?;
        url.query_pairs_mut()
            .append_pair("state", "opened")
            .append_pair("search", title);
        let issues = self
            .api
            .request(&Method::GET, &url, ("PRIVATE-TOKEN", &self.token), None)?;
        if issues
            .as_array()
            .into_iter()
            .flatten()
            .any(|issue| issue["title"].as_str() == Some(title))
        {
            return Ok(());
        }
        let form = vec![
            ("title".into(), title.into()),
            ("description".into(), description.into()),
            ("labels".into(), "ci-sync,incident".into()),
        ];
        let payload = Payload::Form(&form);
        self.request(&Method::POST, "issues", Some(&payload))?;
        Ok(())
    }

    pub(super) fn git_header(&self) -> String {
        let basic = STANDARD.encode(format!("{}:{}", self.config.gitlab_username, self.token));
        format!("Authorization: Basic {basic}")
    }

    fn request(&self, method: &Method, path: &str, payload: Option<&Payload<'_>>) -> Result<Value> {
        let url = self.project_url(path)?;
        self.api
            .request(method, &url, ("PRIVATE-TOKEN", &self.token), payload)
    }

    fn project_url(&self, path: &str) -> Result<Url> {
        Url::parse(&format!(
            "{}/api/v4/projects/{}/{}",
            self.config.gitlab_origin(),
            self.config.gitlab_project_id,
            path.trim_start_matches('/')
        ))
        .with_context(|| format!("building GitLab project API URL for {path}"))
    }
}

fn read_secret(path: &std::path::Path, label: &str) -> Result<String> {
    let secret = fs::read_to_string(path)
        .with_context(|| format!("reading {label} {}", path.display()))?
        .trim()
        .to_string();
    if secret.is_empty() {
        bail!("{label} file is empty");
    }
    Ok(secret)
}

/// GitHub rejects a commit status whose description exceeds 140 characters, and
/// a rejection detail naming several control paths goes well past that.
fn status_description(detail: &str) -> String {
    const LIMIT: usize = 140;
    let single_line = detail.split_whitespace().collect::<Vec<_>>().join(" ");
    if single_line.chars().count() <= LIMIT {
        return single_line;
    }
    single_line
        .chars()
        .take(LIMIT - 1)
        .chain(std::iter::once('…'))
        .collect()
}

fn string_field(value: &Value, path: &[&str], label: &str) -> Result<String> {
    let mut current = value;
    for part in path {
        current = current
            .get(part)
            .with_context(|| format!("{label} response is missing {part}"))?;
    }
    current
        .as_str()
        .map(str::to_string)
        .with_context(|| format!("{label} is not a string"))
}

/// Every job that proves anything lives in the child pipeline the dispatch
/// stage triggers. A cancelled child under a `success` parent is a state this
/// project has produced, so the parent's own status is not evidence that the
/// tests ran.
fn blocking_downstream_status(value: &Value) -> Result<Option<String>> {
    let bridges = value
        .as_array()
        .context("GitLab bridges response was not an array")?;
    for bridge in bridges {
        match bridge["downstream_pipeline"]["status"].as_str() {
            Some("success") => {}
            Some(status) => return Ok(Some(status.to_owned())),
            None => return Ok(Some("missing".to_owned())),
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_status_description_survives_a_multi_line_rejection() {
        let detail = "The merged GitHub pull request changes CI control files.\n\n- `xtask/`";
        assert_eq!(
            status_description(detail),
            "The merged GitHub pull request changes CI control files. - `xtask/`"
        );
    }

    #[test]
    fn a_status_description_stays_within_what_github_accepts() {
        let described = status_description(&"path ".repeat(80));
        assert_eq!(described.chars().count(), 140);
        assert!(described.ends_with('…'));
    }

    #[test]
    fn a_cancelled_child_blocks_a_green_parent() {
        assert_eq!(
            blocking_downstream_status(&json!([{"downstream_pipeline": {"status": "canceled"}}]))
                .unwrap(),
            Some("canceled".into())
        );
    }

    #[test]
    fn a_dispatch_job_without_a_child_proves_nothing() {
        assert_eq!(
            blocking_downstream_status(&json!([{"downstream_pipeline": null}])).unwrap(),
            Some("missing".into())
        );
    }

    #[test]
    fn every_green_child_leaves_the_parent_status_standing() {
        assert_eq!(
            blocking_downstream_status(&json!([
                {"downstream_pipeline": {"status": "success"}},
                {"downstream_pipeline": {"status": "success"}}
            ]))
            .unwrap(),
            None
        );
    }
}
