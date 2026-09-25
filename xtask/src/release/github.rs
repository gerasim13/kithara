use std::{
    path::{Path, PathBuf},
    process::{Command, Output},
};

use anyhow::{Context, Result, bail};
use serde_json::Value;

use super::artifact::{file_name, sha256};

/// Create the release under `tag`, which the tag step pushed, or amend the one
/// a previous run created, and attach every asset. An asset already attached
/// with the same bytes stays; one with other bytes stops the run.
pub(super) fn publish(
    repo: &str,
    tag: &str,
    title: &str,
    notes: &str,
    assets: &[PathBuf],
) -> Result<()> {
    if view(repo, tag)? {
        println!("[github] updating release {tag}...");
        run_step(
            gh().args([
                "release", "edit", tag, "--repo", repo, "--title", title, "--notes", notes,
            ]),
            "gh release edit",
        )?;
    } else {
        println!("[github] creating release {tag}...");
        run_step(
            gh().args([
                "release",
                "create",
                tag,
                "--repo",
                repo,
                "--verify-tag",
                "--title",
                title,
                "--notes",
                notes,
            ]),
            "gh release create",
        )?;
    }
    for asset in assets {
        upload_asset(repo, tag, asset)?;
    }
    Ok(())
}

/// `gh` amends a release in place but will not take new bytes for an asset
/// name it already holds, so the release goes away with its tag and comes back
/// pointing at today's commit.
pub(super) fn replace_nightly(
    repo: &str,
    tag: &str,
    title: &str,
    sha: &str,
    notes: &str,
    assets: &[PathBuf],
) -> Result<()> {
    // The commit has to be in the mirror before the old release is taken down.
    // GitHub mirrors the default branch alone, and the release that goes away
    // first would leave the channel carrying nothing at all if the commit that
    // replaces it turned out not to be there.
    let target = gh()
        .args(["api", &format!("repos/{repo}/commits/{sha}")])
        .output()
        .context("run gh api commits")?;
    if !target.status.success() {
        bail!(
            "github mirror {repo} does not carry {sha}; the nightly channel publishes what the \
             mirrored branch builds, and nothing was replaced.\n{}",
            output_text(&target).trim()
        );
    }

    if view(repo, tag)? {
        println!("[github] replacing nightly release {tag}...");
        run_step(
            gh().args([
                "release",
                "delete",
                tag,
                "--repo",
                repo,
                "--cleanup-tag",
                "--yes",
            ]),
            "gh release delete",
        )?;
    } else {
        println!("[github] creating nightly release {tag}...");
    }

    let mut create = gh();
    create.args([
        "release",
        "create",
        tag,
        "--repo",
        repo,
        "--target",
        sha,
        "--title",
        title,
        "--notes",
        notes,
        "--prerelease",
    ]);
    create.args(assets);
    run_step(&mut create, "gh release create")?;

    for asset in assets {
        verify_asset(repo, tag, &file_name(asset)?, &sha256(asset)?)?;
    }
    Ok(())
}

/// Whether a release exists under `tag`. Only an answer that the release is
/// missing counts as a no: an authentication failure stops the run.
fn view(repo: &str, tag: &str) -> Result<bool> {
    let view = gh()
        .args(["release", "view", tag, "--repo", repo])
        .output()
        .context("run gh release view")?;
    if view.status.success() {
        return Ok(true);
    }
    let text = output_text(&view);
    if !release_missing(&text) {
        bail!("gh release view failed: {}", text.trim());
    }
    Ok(false)
}

fn upload_asset(repo: &str, tag: &str, file: &Path) -> Result<()> {
    let name = file_name(file)?;
    let checksum = sha256(file)?;
    match asset_sha256(repo, tag, &name)? {
        Some(existing) if existing == checksum => {
            println!("[github] asset {name} already uploaded with matching checksum");
            return Ok(());
        }
        Some(existing) => {
            bail!("github asset {name} has sha256 {existing}, expected {checksum}");
        }
        None => {}
    }

    println!("[github] uploading {name}...");
    run_step(
        gh().args(["release", "upload", tag, "--repo", repo])
            .arg(file),
        "gh release upload",
    )?;
    verify_asset(repo, tag, &name, &checksum)
}

fn verify_asset(repo: &str, tag: &str, name: &str, expected: &str) -> Result<()> {
    let actual = asset_sha256(repo, tag, name)?
        .with_context(|| format!("github asset {name} is missing after upload"))?;
    if actual != expected {
        bail!("github asset {name} has sha256 {actual}, expected {expected}");
    }
    Ok(())
}

fn asset_sha256(repo: &str, tag: &str, name: &str) -> Result<Option<String>> {
    let release = gh_json(&["api", &format!("repos/{repo}/releases/tags/{tag}")])?;
    let Some(asset) = release["assets"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|asset| asset["name"].as_str() == Some(name))
    else {
        return Ok(None);
    };

    if let Some(digest) = asset["digest"].as_str()
        && let Some(checksum) = digest.strip_prefix("sha256:")
    {
        return Ok(Some(checksum.to_string()));
    }

    let download = tempfile::tempdir().context("creating a release asset download directory")?;
    run_step(
        gh().args([
            "release",
            "download",
            tag,
            "--repo",
            repo,
            "--pattern",
            name,
            "--dir",
        ])
        .arg(download.path()),
        "gh release download",
    )?;
    sha256(&download.path().join(name)).map(Some)
}

fn gh() -> Command {
    Command::new("gh")
}

fn gh_json(args: &[&str]) -> Result<Value> {
    let output = gh()
        .args(args)
        .output()
        .with_context(|| format!("run gh {args:?}"))?;
    if !output.status.success() {
        bail!(
            "gh {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    serde_json::from_slice(&output.stdout).context("parse gh json output")
}

fn run_step(command: &mut Command, description: &str) -> Result<()> {
    let status = command
        .status()
        .with_context(|| format!("failed to run {description}"))?;
    if !status.success() {
        bail!(
            "{description} failed (exit code: {})",
            status.code().unwrap_or(-1)
        );
    }
    Ok(())
}

fn release_missing(text: &str) -> bool {
    let text = text.to_ascii_lowercase();
    text.contains("release not found")
        || (text.contains("not found") && text.contains("404"))
        || text.contains("http 404")
}

fn output_text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gh_release_missing_does_not_hide_auth_failures() {
        assert!(release_missing("HTTP 404: release not found"));
        assert!(!release_missing(
            "Get https://api.github.com/repos/zvuk/kithara/releases/tags/v0.0.1-alpha3: Forbidden"
        ));
        assert!(!release_missing(
            "The token in keyring is invalid. To re-authenticate, run: gh auth refresh"
        ));
    }
}
