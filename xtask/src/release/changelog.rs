use std::{fs, path::Path, process::Command};

use anyhow::{Context, Result, bail};
use cargo_metadata::semver::Version;
use kithara_devtools::common::tools::ToolsConfig;

use super::{git::git, tag::tag_of};
use crate::config::ChangelogConfig;

/// One git-cliff render: the commits in `range`, named `tag` when no release
/// tag closes the range.
#[derive(Debug, PartialEq, Eq)]
struct Section {
    range: String,
    tag: Option<String>,
}

/// Render the changelog at `head` and write it to the configured output.
pub(super) fn write(
    root: &Path,
    cfg: &ChangelogConfig,
    tools: &ToolsConfig,
    version: Option<&str>,
) -> Result<()> {
    let text = render(root, cfg, tools, "HEAD", version)?;
    let output = root.join(&cfg.output);
    fs::write(&output, text).with_context(|| format!("writing {}", output.display()))
}

/// `CHANGELOG.md` at `head`: one git-cliff render per release range, joined
/// newest first. A release tag sits on the stamp commit the tag step makes on
/// top of the built commit, outside the branch history, so one range over that
/// history never meets it and folds every release into a single section.
/// `version` names the commits after the last release tag as that release.
pub(super) fn render(
    root: &Path,
    cfg: &ChangelogConfig,
    tools: &ToolsConfig,
    head: &str,
    version: Option<&str>,
) -> Result<String> {
    let listing = git(root, &["tag", "--list", "v[0-9]*", "--contains", &cfg.base])?;
    let tags = release_tags(&listing, &cfg.base)?;
    if let Some(version) = version {
        admit(version, &cfg.base, &tags)?;
    }
    let mut sections = sections(&cfg.base, &tags, head, version);
    if version.is_none()
        && sections.len() > 1
        && git(root, &["rev-list", "--count", &sections[0].range])? == "0"
    {
        sections.remove(0);
    }
    let count = sections.len();
    let mut out = String::new();
    for (index, section) in sections.iter().enumerate() {
        out.push_str(&cliff(root, cfg, tools, section, strip(index, count))?);
    }
    Ok(out)
}

/// Release tags after `base`, oldest first.
fn release_tags(listing: &str, base: &str) -> Result<Vec<String>> {
    let base_version = tag_version(base)?;
    let mut tags = Vec::new();
    for tag in listing.lines().map(str::trim).filter(|tag| !tag.is_empty()) {
        let version = tag_version(tag)?;
        if version > base_version {
            tags.push((version, tag.to_string()));
        }
    }
    tags.sort();
    Ok(tags.into_iter().map(|(_, tag)| tag).collect())
}

fn tag_version(tag: &str) -> Result<Version> {
    let version = tag
        .strip_prefix('v')
        .with_context(|| format!("release tag {tag} does not start with v"))?;
    Version::parse(version).with_context(|| format!("release tag {tag} is not a semantic version"))
}

/// A named release follows every tagged one and is not tagged itself.
fn admit(version: &str, base: &str, tags: &[String]) -> Result<()> {
    let tag = tag_of(version);
    let last = tags.last().map_or(base, String::as_str);
    if tag_version(&tag)? <= tag_version(last)? {
        bail!("{tag} does not follow the last release tag {last}");
    }
    Ok(())
}

/// Ranges from `base` through every tag to `head`, newest first.
fn sections(base: &str, tags: &[String], head: &str, version: Option<&str>) -> Vec<Section> {
    let mut sections = Vec::with_capacity(tags.len() + 1);
    let mut previous = base;
    for tag in tags {
        sections.push(Section {
            range: format!("{previous}..{tag}"),
            tag: None,
        });
        previous = tag;
    }
    sections.push(Section {
        range: format!("{previous}..{head}"),
        tag: version.map(tag_of),
    });
    sections.reverse();
    sections
}

/// The newest render carries the header and the oldest the footer, so the
/// joined renders read as one git-cliff render of the whole history.
const fn strip(index: usize, count: usize) -> Option<&'static str> {
    match (index == 0, index + 1 == count) {
        (true, true) => None,
        (true, false) => Some("footer"),
        (false, true) => Some("header"),
        (false, false) => Some("all"),
    }
}

fn cliff(
    root: &Path,
    cfg: &ChangelogConfig,
    tools: &ToolsConfig,
    section: &Section,
    strip: Option<&str>,
) -> Result<String> {
    let program = tools.program("git-cliff");
    let mut command = Command::new(program);
    command.current_dir(root).arg("--config").arg(&cfg.config);
    if let Some(strip) = strip {
        command.args(["--strip", strip]);
    }
    if let Some(tag) = &section.tag {
        command.args(["--tag", tag]);
    }
    let output = command
        .arg(&section.range)
        .output()
        .with_context(|| format!("running {program} {}", section.range))?;
    if !output.status.success() {
        bail!(
            "{program} {} failed: {}",
            section.range,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    String::from_utf8(output.stdout).context("git-cliff output was not UTF-8")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn section(range: &str, tag: Option<&str>) -> Section {
        Section {
            range: range.to_string(),
            tag: tag.map(str::to_string),
        }
    }

    #[test]
    fn release_tags_follow_the_base_in_version_order() {
        let listing = "v0.0.1-alpha4\nv0.0.1-alpha6\nv0.0.1-alpha5\nv0.0.1\n";

        let tags = release_tags(listing, "v0.0.1-alpha4").unwrap();

        assert_eq!(tags, ["v0.0.1-alpha5", "v0.0.1-alpha6", "v0.0.1"]);
    }

    #[test]
    fn a_release_tag_must_be_a_version() {
        let error = release_tags("v0.0.1-alpha5\nvnext\n", "v0.0.1-alpha4").unwrap_err();

        assert!(error.to_string().contains("vnext"), "{error}");
    }

    #[test]
    fn every_release_range_renders_on_its_own() {
        let tags = ["v0.0.1-alpha5".to_string(), "v0.0.1-alpha6".to_string()];

        let sections = sections("v0.0.1-alpha4", &tags, "HEAD", Some("0.0.1-alpha7"));

        assert_eq!(
            sections,
            [
                section("v0.0.1-alpha6..HEAD", Some("v0.0.1-alpha7")),
                section("v0.0.1-alpha5..v0.0.1-alpha6", None),
                section("v0.0.1-alpha4..v0.0.1-alpha5", None),
            ]
        );
    }

    #[test]
    fn the_joined_renders_keep_one_header_and_one_footer() {
        assert_eq!(strip(0, 1), None);
        assert_eq!(
            (0..3).map(|index| strip(index, 3)).collect::<Vec<_>>(),
            [Some("footer"), Some("all"), Some("header")]
        );
    }

    #[test]
    fn a_named_release_follows_the_last_tag() {
        let tags = ["v0.0.1-alpha5".to_string()];

        assert!(admit("0.0.1-alpha6", "v0.0.1-alpha4", &tags).is_ok());
        assert!(admit("0.0.1-alpha5", "v0.0.1-alpha4", &tags).is_err());
        assert!(admit("0.0.1-alpha3", "v0.0.1-alpha4", &[]).is_err());
    }
}
