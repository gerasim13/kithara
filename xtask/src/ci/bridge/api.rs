use std::{
    fs,
    io::Write,
    process::{Command, Stdio},
    time::Duration,
};

use anyhow::{Context, Result, bail};
use base64::{
    Engine,
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
};
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
}

impl Github {
    pub(super) fn new(config: &BridgeConfig) -> Result<Self> {
        Ok(Self {
            config: config.clone(),
            api: Api::new()?,
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

    pub(super) fn create_check(&self, sha: &str) -> Result<u64> {
        let response = self.request(
            &Method::POST,
            &format!("/repos/{}/check-runs", self.config.github_repo),
            Some(&json!({
                "name": "Kithara / GitLab verification",
                "head_sha": sha,
                "status": "in_progress",
                "started_at": utc_timestamp()?,
            })),
        )?;
        response["id"]
            .as_u64()
            .context("GitHub check response has no numeric id")
    }

    pub(super) fn finish_check(
        &self,
        check_run_id: u64,
        conclusion: &str,
        summary: &str,
    ) -> Result<()> {
        self.request(
            &Method::PATCH,
            &format!(
                "/repos/{}/check-runs/{check_run_id}",
                self.config.github_repo
            ),
            Some(&json!({
                "status": "completed",
                "conclusion": conclusion,
                "completed_at": utc_timestamp()?,
                "output": {
                    "title": "GitLab verification",
                    "summary": summary,
                },
            })),
        )?;
        Ok(())
    }

    pub(super) fn git_header(&self) -> Result<String> {
        let basic = STANDARD.encode(format!("x-access-token:{}", self.app_token()?));
        Ok(format!("Authorization: Basic {basic}"))
    }

    fn request(&self, method: &Method, path: &str, body: Option<&Value>) -> Result<Value> {
        let token = self.app_token()?;
        let url = Url::parse(&format!("https://api.github.com{path}"))
            .with_context(|| format!("building GitHub API URL for {path}"))?;
        let payload = body.map(Payload::Json);
        self.api.request(
            method,
            &url,
            ("Authorization", &format!("Bearer {token}")),
            payload.as_ref(),
        )
    }

    fn app_token(&self) -> Result<String> {
        let now = unix_time()?;
        let header = URL_SAFE_NO_PAD.encode(br#"{"alg":"RS256","typ":"JWT"}"#);
        let payload = URL_SAFE_NO_PAD.encode(
            serde_json::to_vec(&json!({
                "iat": now.saturating_sub(30),
                "exp": now + 540,
                "iss": self.config.github_app_id,
            }))
            .context("serializing GitHub App JWT")?,
        );
        let signing_input = format!("{header}.{payload}");
        let signature = openssl_sign(&self.config.github_private_key, signing_input.as_bytes())?;
        let jwt = format!("{signing_input}.{}", URL_SAFE_NO_PAD.encode(signature));
        let url = Url::parse(&format!(
            "https://api.github.com/app/installations/{}/access_tokens",
            self.config.github_installation_id
        ))
        .context("building GitHub installation token URL")?;
        let body = json!({});
        let payload = Payload::Json(&body);
        let response = self.api.request(
            &Method::POST,
            &url,
            ("Authorization", &format!("Bearer {jwt}")),
            Some(&payload),
        )?;
        response["token"]
            .as_str()
            .map(str::to_string)
            .context("GitHub installation token response has no token")
    }
}

pub(super) struct Gitlab {
    config: BridgeConfig,
    api: Api,
    token: String,
}

impl Gitlab {
    pub(super) fn new(config: &BridgeConfig) -> Result<Self> {
        let token = fs::read_to_string(&config.gitlab_token_file)
            .with_context(|| {
                format!(
                    "reading GitLab token {}",
                    config.gitlab_token_file.display()
                )
            })?
            .trim()
            .to_string();
        if token.is_empty() {
            bail!("GitLab token file is empty");
        }
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

    pub(super) fn pipeline_status(&self, pipeline_id: u64) -> Result<String> {
        let response = self.request(&Method::GET, &format!("pipelines/{pipeline_id}"), None)?;
        response["status"]
            .as_str()
            .map(str::to_string)
            .context("GitLab pipeline response has no status")
    }

    pub(super) fn latest_push_pipeline_status(&self, sha: &str) -> Result<Option<String>> {
        let mut url = self.project_url("pipelines")?;
        url.query_pairs_mut()
            .append_pair("sha", sha)
            .append_pair("ref", &self.config.branch)
            .append_pair("source", "push")
            .append_pair("order_by", "id")
            .append_pair("sort", "desc")
            .append_pair("per_page", "1");
        let pipelines =
            self.api
                .request(&Method::GET, &url, ("PRIVATE-TOKEN", &self.token), None)?;
        pipeline_status_from_list(&pipelines)
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

fn openssl_sign(key: &std::path::Path, payload: &[u8]) -> Result<Vec<u8>> {
    let mut child = Command::new("openssl")
        .args(["dgst", "-sha256", "-sign"])
        .arg(key)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("starting OpenSSL for GitHub App JWT")?;
    child
        .stdin
        .take()
        .context("OpenSSL stdin was not piped")?
        .write_all(payload)
        .context("writing GitHub App JWT to OpenSSL")?;
    let output = child
        .wait_with_output()
        .context("waiting for OpenSSL JWT signature")?;
    if !output.status.success() {
        bail!(
            "OpenSSL JWT signing failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(output.stdout)
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

fn pipeline_status_from_list(value: &Value) -> Result<Option<String>> {
    let pipelines = value
        .as_array()
        .context("GitLab pipelines response was not an array")?;
    pipelines
        .first()
        .map(|pipeline| {
            pipeline["status"]
                .as_str()
                .map(str::to_string)
                .context("GitLab pipeline response has no status")
        })
        .transpose()
}

fn unix_time() -> Result<u64> {
    Ok(std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .context("system clock is before Unix epoch")?
        .as_secs())
}

fn utc_timestamp() -> Result<String> {
    let output = Command::new("date")
        .args(["-u", "+%Y-%m-%dT%H:%M:%SZ"])
        .output()
        .context("running date for API timestamp")?;
    if !output.status.success() {
        bail!(
            "date failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    String::from_utf8(output.stdout)
        .context("date timestamp was not UTF-8")
        .map(|value| value.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn latest_pipeline_status_is_optional_but_typed() {
        assert_eq!(
            pipeline_status_from_list(&json!([{"status": "success"}])).unwrap(),
            Some("success".into())
        );
        assert_eq!(pipeline_status_from_list(&json!([])).unwrap(), None);
        assert!(pipeline_status_from_list(&json!([{}])).is_err());
    }
}
