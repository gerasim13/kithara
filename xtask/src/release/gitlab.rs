use std::{
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

use super::{
    artifact::{file_name, sha256},
    git::token,
};
use crate::config::ReleaseConfig;

/// Mirror the release under `tag`, which the tag step pushed: every asset goes
/// to the generic package registry, and the release links to it. A re-run
/// keeps what is already there with the same bytes.
pub(super) fn publish(
    cfg: &ReleaseConfig,
    tag: &str,
    title: &str,
    notes: &str,
    assets: &[PathBuf],
) -> Result<()> {
    let api = Api::new(cfg)?;
    let (code, body) = api.get(&format!("repository/tags/{tag}"))?;
    if code != 200 {
        bail!("gitlab does not carry the tag {tag} (HTTP {code}): {body}");
    }
    for asset in assets {
        upload_asset(&api, cfg, tag, asset)?;
    }
    let links = links(cfg, tag, &names(assets)?);

    let (code, body) = api.get(&format!("releases/{tag}"))?;
    match code {
        200 => {
            println!("[gitlab] updating release {tag}...");
            let payload = json!({ "name": title, "description": notes });
            let (code, body) = api.put(&format!("releases/{tag}"), Some(&payload.to_string()))?;
            if code != 200 {
                bail!("gitlab release update failed (HTTP {code}): {body}");
            }
            sync_links(&api, tag, &links)
        }
        // A release that does not exist answers 403 as well as 404.
        403 | 404 => {
            println!("[gitlab] creating release {tag}...");
            create_release(&api, tag, title, notes, &links)
        }
        other => bail!("gitlab release lookup failed (HTTP {other}): {body}"),
    }
}

/// The `GitLab` side replaces the same three things the GitHub side does: the
/// release, the tag under it, and the package version the release links to.
pub(super) fn replace_nightly(
    cfg: &ReleaseConfig,
    tag: &str,
    title: &str,
    sha: &str,
    notes: &str,
    assets: &[PathBuf],
) -> Result<()> {
    let api = Api::new(cfg)?;
    retract_with(&api, tag)?;
    let (code, body) = api.delete(&format!("repository/tags/{tag}"))?;
    match code {
        200 | 204 | 403 | 404 => {}
        other => bail!("gitlab tag delete failed (HTTP {other}): {body}"),
    }

    let (code, body) = api.post(&format!("repository/tags?tag_name={tag}&ref={sha}"), None)?;
    if code != 201 {
        bail!("gitlab tag create failed (HTTP {code}): {body}");
    }
    for asset in assets {
        let name = file_name(asset)?;
        println!("[gitlab] uploading {name} to package registry...");
        let (code, body) = api.upload(&package_path(cfg, tag, &name), asset)?;
        if code != 200 && code != 201 {
            bail!("gitlab upload of {name} failed (HTTP {code}): {body}");
        }
    }
    create_release(&api, tag, title, notes, &links(cfg, tag, &names(assets)?))
}

/// Take down the release under `tag` and the package version it links to,
/// leaving the tag.
pub(super) fn retract(cfg: &ReleaseConfig, tag: &str) -> Result<()> {
    retract_with(&Api::new(cfg)?, tag)
}

fn retract_with(api: &Api, tag: &str) -> Result<()> {
    // Ask before removing. `GitLab` answers a delete for a release that does
    // not exist with 403 rather than 404 — it evaluates the permission against
    // nothing — and a first run has nothing to remove. Reading that as a
    // permission failure stopped the channel on the one run where there was
    // provably no problem.
    let (code, body) = api.get(&format!("releases/{tag}"))?;
    match code {
        200 => {
            let (code, body) = api.delete(&format!("releases/{tag}"))?;
            match code {
                200 | 204 => println!("[gitlab] removed release {tag}"),
                // The same 403-for-nothing-to-delete as the lookup above.
                403 | 404 => {}
                other => bail!("gitlab release delete failed (HTTP {other}): {body}"),
            }
        }
        403 | 404 => {}
        other => bail!("gitlab release lookup failed (HTTP {other}): {body}"),
    }
    if let Some(id) = api.package_id(tag)? {
        let (code, body) = api.delete(&format!("packages/{id}"))?;
        match code {
            200 | 204 | 404 => println!("[gitlab] removed package {tag}"),
            other => bail!("gitlab package delete failed (HTTP {other}): {body}"),
        }
    }
    Ok(())
}

fn create_release(api: &Api, tag: &str, title: &str, notes: &str, links: &[Link]) -> Result<()> {
    let payload = json!({
        "tag_name": tag,
        "name": title,
        "description": notes,
        "assets": { "links": links.iter().map(Link::as_json).collect::<Vec<_>>() },
    });
    let (code, body) = api.post("releases", Some(&payload.to_string()))?;
    if code != 201 {
        bail!("gitlab release create failed (HTTP {code}): {body}");
    }
    Ok(())
}

fn upload_asset(api: &Api, cfg: &ReleaseConfig, tag: &str, file: &Path) -> Result<()> {
    let name = file_name(file)?;
    let checksum = sha256(file)?;
    match api.package_file_sha(tag, &name)? {
        Some(existing) if existing == checksum => {
            println!("[gitlab] package asset {name} already uploaded");
            return Ok(());
        }
        Some(existing) => bail!(
            "gitlab package asset {name} has sha256 {existing}, expected {checksum}; \
             delete the broken package file and re-run"
        ),
        None => {}
    }

    println!("[gitlab] uploading {name} to package registry...");
    let (code, body) = api.upload(&package_path(cfg, tag, &name), file)?;
    if code != 200 && code != 201 {
        bail!("gitlab upload of {name} failed (HTTP {code}): {body}");
    }
    Ok(())
}

fn names(assets: &[PathBuf]) -> Result<Vec<String>> {
    assets.iter().map(|asset| file_name(asset)).collect()
}

#[derive(Debug)]
struct Link {
    name: String,
    url: String,
}

impl Link {
    fn as_json(&self) -> Value {
        json!({
            "name": &self.name,
            "url": &self.url,
            "link_type": "package",
        })
    }
}

/// A release link for each asset, pointing at its package registry file.
fn links(cfg: &ReleaseConfig, tag: &str, names: &[String]) -> Vec<Link> {
    names
        .iter()
        .map(|name| Link {
            name: name.clone(),
            url: format!(
                "https://{}/api/v4/projects/{}/{}",
                cfg.gitlab_host,
                cfg.gitlab_project.replace('/', "%2F"),
                package_path(cfg, tag, name)
            ),
        })
        .collect()
}

fn sync_links(api: &Api, tag: &str, links: &[Link]) -> Result<()> {
    let (code, body) = api.get(&format!("releases/{tag}/assets/links"))?;
    if code != 200 {
        bail!("gitlab release links lookup failed (HTTP {code}): {body}");
    }
    let existing: Value = serde_json::from_str(&body).context("parse release links json")?;
    for link in links {
        let existing_link = existing
            .as_array()
            .into_iter()
            .flatten()
            .find(|item| item["name"].as_str() == Some(link.name.as_str()));
        match existing_link {
            Some(item) if item["url"].as_str() == Some(link.url.as_str()) => {
                println!(
                    "[gitlab] release link {} already points to package",
                    link.name
                );
            }
            Some(item) => {
                let id = item["id"]
                    .as_i64()
                    .with_context(|| format!("gitlab release link {} missing id", link.name))?;
                println!("[gitlab] updating release link {}...", link.name);
                let (code, body) = api.put(
                    &format!("releases/{tag}/assets/links/{id}"),
                    Some(&link.as_json().to_string()),
                )?;
                if code != 200 {
                    bail!(
                        "gitlab release link update for {} failed (HTTP {code}): {body}",
                        link.name
                    );
                }
            }
            None => {
                println!("[gitlab] adding release link {}...", link.name);
                let (code, body) = api.post(
                    &format!("releases/{tag}/assets/links"),
                    Some(&link.as_json().to_string()),
                )?;
                if code != 201 {
                    bail!(
                        "gitlab release link create for {} failed (HTTP {code}): {body}",
                        link.name
                    );
                }
            }
        }
    }
    Ok(())
}

fn package_path(cfg: &ReleaseConfig, tag: &str, name: &str) -> String {
    format!("packages/generic/{}/{tag}/{name}", cfg.gitlab_package)
}

fn http_timeout_secs(cfg: &ReleaseConfig) -> Result<u64> {
    cfg.http_timeout_secs.context(
        "ext.release.http_timeout_secs is not set; fill in the [ext.release] section of .config/xtask.toml",
    )
}

fn upload_timeout_secs(cfg: &ReleaseConfig) -> Result<u64> {
    cfg.upload_timeout_secs.context(
        "ext.release.upload_timeout_secs is not set; fill in the [ext.release] section of .config/xtask.toml",
    )
}

/// The project's REST API, authenticated with `GITLAB_TOKEN`.
struct Api {
    base: String,
    noproxy: String,
    token: String,
    http_timeout_secs: u64,
    upload_timeout_secs: u64,
}

impl Api {
    fn new(cfg: &ReleaseConfig) -> Result<Self> {
        Ok(Self {
            base: format!(
                "https://{}/api/v4/projects/{}",
                cfg.gitlab_host,
                cfg.gitlab_project.replace('/', "%2F")
            ),
            noproxy: cfg.gitlab_host.clone(),
            token: token("GITLAB_TOKEN")?,
            http_timeout_secs: http_timeout_secs(cfg)?,
            upload_timeout_secs: upload_timeout_secs(cfg)?,
        })
    }

    fn get(&self, path: &str) -> Result<(u16, String)> {
        self.curl(&[], path)
    }

    fn post(&self, path: &str, body: Option<&str>) -> Result<(u16, String)> {
        self.send("POST", path, body)
    }

    fn put(&self, path: &str, body: Option<&str>) -> Result<(u16, String)> {
        self.send("PUT", path, body)
    }

    fn send(&self, method: &str, path: &str, body: Option<&str>) -> Result<(u16, String)> {
        let mut extra = vec!["--request", method];
        if let Some(json) = body {
            extra.extend(["--header", "Content-Type: application/json", "--data", json]);
        }
        self.curl(&extra, path)
    }

    fn delete(&self, path: &str) -> Result<(u16, String)> {
        self.curl(&["--request", "DELETE"], path)
    }

    fn upload(&self, path: &str, file: &Path) -> Result<(u16, String)> {
        let file_arg = file.display().to_string();
        let timeout = self.upload_timeout_secs.to_string();
        self.curl(&["--upload-file", &file_arg, "--max-time", &timeout], path)
    }

    /// Internal hosts are not reachable through corporate/sandbox proxies,
    /// so the `GitLab` host is always taken off-proxy.
    fn curl(&self, extra: &[&str], path: &str) -> Result<(u16, String)> {
        let url = format!("{}/{path}", self.base);
        let timeout = self.http_timeout_secs.to_string();
        let private_token = format!("PRIVATE-TOKEN: {}", self.token);
        let output = Command::new("curl")
            .args([
                "-sS",
                "--max-time",
                &timeout,
                "--noproxy",
                &self.noproxy,
                "--header",
                &private_token,
                "-w",
                "\n%{http_code}",
            ])
            .args(extra)
            .arg(&url)
            .output()
            .with_context(|| format!("run curl {url}"))?;
        if !output.status.success() {
            bail!(
                "curl {url} failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        let stdout = String::from_utf8_lossy(&output.stdout);
        let (body, code) = stdout
            .rsplit_once('\n')
            .with_context(|| format!("curl {url}: missing status line"))?;
        let code: u16 = code.trim().parse().context("parse http status")?;
        Ok((code, body.to_string()))
    }

    /// Registry id of the generic package holding `tag`, or None when nothing
    /// has been published under it yet.
    fn package_id(&self, tag: &str) -> Result<Option<i64>> {
        let (code, body) = self.get(&format!("packages?package_version={tag}"))?;
        if code != 200 {
            bail!("gitlab package lookup failed (HTTP {code}): {body}");
        }
        let packages: Value = serde_json::from_str(&body).context("parse packages json")?;
        Ok(packages
            .as_array()
            .into_iter()
            .flatten()
            .find(|p| p["version"].as_str() == Some(tag))
            .and_then(|p| p["id"].as_i64()))
    }

    /// sha256 of the asset file in the generic registry for `tag`,
    /// or None when the package or file does not exist yet.
    fn package_file_sha(&self, tag: &str, name: &str) -> Result<Option<String>> {
        let Some(pkg_id) = self.package_id(tag)? else {
            return Ok(None);
        };

        let (code, body) = self.get(&format!("packages/{pkg_id}/package_files"))?;
        if code != 200 {
            bail!("gitlab package_files lookup failed (HTTP {code}): {body}");
        }
        let files: Value = serde_json::from_str(&body).context("parse package_files json")?;
        let sha = files
            .as_array()
            .into_iter()
            .flatten()
            .filter(|f| f["file_name"].as_str() == Some(name))
            .filter_map(|f| f["file_sha256"].as_str())
            .next_back()
            .map(str::to_string);
        Ok(sha)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::config::DocsChannel;

    #[test]
    fn gitlab_release_links_cover_every_published_asset() {
        let cfg = ReleaseConfig {
            gitlab_host: "gitlab.zvq.me".into(),
            gitlab_project: "disrupt/kithara".into(),
            gitlab_package: "kithara".into(),
            core_asset: "KitharaFFIInternal.xcframework.zip".into(),
            merged_asset: "Kithara.xcframework.zip".into(),
            wasm_asset: "kithara-wasm-pages.zip".into(),
            platform_assets: vec!["kithara.aar".into()],
            docs: BTreeMap::from([(
                "apple".to_string(),
                DocsChannel {
                    asset: "Kithara-docs.zip".into(),
                    ..DocsChannel::default()
                },
            )]),
            ..ReleaseConfig::default()
        };
        let names: Vec<_> = cfg.assets().map(str::to_string).collect();

        let links = links(&cfg, "v0.0.2", &names);

        assert_eq!(
            links
                .iter()
                .map(|link| link.name.as_str())
                .collect::<Vec<_>>(),
            [
                "KitharaFFIInternal.xcframework.zip",
                "Kithara.xcframework.zip",
                "kithara-wasm-pages.zip",
                "kithara.aar",
                "Kithara-docs.zip",
            ]
        );
        assert!(
            links.iter().all(|link| link.url.starts_with(
                "https://gitlab.zvq.me/api/v4/projects/disrupt%2Fkithara/packages/generic/kithara/v0.0.2/"
            ) && link.url.ends_with(&link.name)),
            "{links:#?}"
        );
    }

    #[test]
    fn release_http_timeout_uses_config() {
        let cfg = ReleaseConfig {
            http_timeout_secs: Some(60),
            upload_timeout_secs: Some(600),
            ..ReleaseConfig::default()
        };

        assert_eq!(http_timeout_secs(&cfg).unwrap(), 60);
        assert_eq!(upload_timeout_secs(&cfg).unwrap(), 600);
    }

    #[test]
    fn release_http_timeout_requires_config() {
        let cfg = ReleaseConfig {
            upload_timeout_secs: Some(600),
            ..ReleaseConfig::default()
        };

        let error = http_timeout_secs(&cfg).unwrap_err();
        assert!(error.to_string().contains("http_timeout_secs"), "{error}");
    }

    #[test]
    fn release_upload_timeout_requires_config() {
        let cfg = ReleaseConfig {
            http_timeout_secs: Some(60),
            ..ReleaseConfig::default()
        };

        let error = upload_timeout_secs(&cfg).unwrap_err();
        assert!(error.to_string().contains("upload_timeout_secs"), "{error}");
    }
}
