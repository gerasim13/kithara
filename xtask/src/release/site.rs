use std::{
    fmt::Write as _,
    fs,
    path::Path,
    process::{Command, Stdio},
};

use anyhow::{Context, Result, bail};
use kithara_devtools::common::tools::ToolsConfig;

use super::{
    git::{Remote, committer, github_remote, output, token},
    tag::tag_of,
};
use crate::config::ReleaseConfig;

/// Where a documentation channel sits on the Pages site, as `DocC` needs it to
/// rewrite its links: the site is served under the repository name.
pub(crate) fn hosting_base(cfg: &ReleaseConfig, channel: &str) -> Result<String> {
    let (_, name) = repo_parts(&cfg.github_repo)?;
    Ok(format!("{name}/{}", cfg.docs_channel(channel)?.pages_path))
}

/// The address GitHub serves the repository's Pages site at.
pub(super) fn pages_url(repo: &str) -> Result<String> {
    let (owner, name) = repo_parts(repo)?;
    Ok(format!(
        "https://{}.github.io/{name}/",
        owner.to_ascii_lowercase()
    ))
}

fn repo_parts(repo: &str) -> Result<(&str, &str)> {
    github_remote(repo)?;
    repo.split_once('/')
        .with_context(|| format!("invalid GitHub repository name: {repo:?}"))
}

/// Lay the site out in `out`: the player at the root and every documentation
/// set under its own path, unpacked from the release artifacts, with the
/// release section `latest` added to the player's page.
pub(super) fn assemble(
    cfg: &ReleaseConfig,
    tools: &ToolsConfig,
    artifacts: &Path,
    latest: &str,
    out: &Path,
) -> Result<()> {
    let unpacked = out.join(".unpacked");
    let parts = [(&cfg.wasm_asset, &cfg.wasm_dist, "")].into_iter().chain(
        cfg.docs
            .values()
            .map(|docs| (&docs.asset, &docs.archive, docs.pages_path.as_str())),
    );
    for (number, (asset, source, target)) in parts.enumerate() {
        let into = unpacked.join(number.to_string());
        fs::create_dir_all(&into).with_context(|| format!("creating {}", into.display()))?;
        let status = Command::new(tools.program("unzip"))
            .arg("-q")
            .arg(artifacts.join(asset))
            .arg("-d")
            .arg(&into)
            .stdin(Stdio::null())
            .status()
            .with_context(|| format!("running unzip for {asset}"))?;
        if !status.success() {
            bail!("unzip {asset} failed ({status})");
        }
        // The release jobs zip a directory by its name, so the archive holds
        // that one directory.
        let top = Path::new(source)
            .file_name()
            .with_context(|| format!("{source} names no directory"))?;
        let from = into.join(top);
        if !from.is_dir() {
            bail!("{asset} does not hold {}/", top.to_string_lossy());
        }
        move_entries(&from, &out.join(target))?;
    }
    fs::remove_dir_all(&unpacked).with_context(|| format!("removing {}", unpacked.display()))?;
    let page = out.join("index.html");
    let player = fs::read_to_string(&page)
        .with_context(|| format!("{} holds no index.html", cfg.wasm_asset))?;
    fs::write(&page, with_latest(&player, latest)?).context("writing index.html")?;
    fs::write(out.join(".nojekyll"), "").context("writing .nojekyll")
}

/// Move what `from` holds into `to`, refusing to replace anything already
/// there.
fn move_entries(from: &Path, to: &Path) -> Result<()> {
    fs::create_dir_all(to).with_context(|| format!("creating {}", to.display()))?;
    for entry in fs::read_dir(from).with_context(|| format!("reading {}", from.display()))? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if target.exists() {
            bail!("{} is laid out twice", target.display());
        }
        fs::rename(entry.path(), &target).with_context(|| {
            format!("moving {} to {}", entry.path().display(), target.display())
        })?;
    }
    Ok(())
}

/// The player's page with the release section at its end and the section's
/// style in its head.
fn with_latest(player: &str, latest: &str) -> Result<String> {
    let page = insert_before(player, "</head>", &format!("<style>{STYLE}</style>\n"))?;
    insert_before(&page, "</body>", latest)
}

fn insert_before(page: &str, tag: &str, content: &str) -> Result<String> {
    let mut found = page.match_indices(tag);
    let (Some((at, _)), None) = (found.next(), found.next()) else {
        bail!("the player page has to close {tag} exactly once");
    };
    Ok(format!("{}{content}{}", &page[..at], &page[at..]))
}

/// Replace the Pages branch with one commit holding the site in `out`.
pub(super) fn deploy(cfg: &ReleaseConfig, out: &Path, version: &str) -> Result<()> {
    let remote = Remote::github(&cfg.github_repo, Some(&token("GH_TOKEN")?))?;
    let branch = &cfg.pages_branch;
    output(committer(out).args(["init", "-q", "-b", branch]), None)?;
    output(committer(out).args(["add", "--all"]), None)?;
    output(
        committer(out).args([
            "commit",
            "-q",
            "--no-gpg-sign",
            "-m",
            &format!("Deploy {} {version}", cfg.title),
        ]),
        None,
    )?;
    println!("[pages] pushing the site to {branch}...");
    output(
        remote.git(out)?.args([
            "push",
            "--force",
            &remote.url,
            &format!("HEAD:refs/heads/{branch}"),
        ]),
        None,
    )
    .map(drop)
}

/// The release section added to the player's page: the documentation of every
/// platform, the release archives with their checksums, and every crate on
/// crates.io and docs.rs.
pub(super) fn section(
    cfg: &ReleaseConfig,
    version: &str,
    crates: &[String],
    artifacts: &[(String, String)],
) -> String {
    let repo = format!("https://github.com/{}", cfg.github_repo);
    let tag = tag_of(version);
    let release = format!("{repo}/releases/tag/{tag}");
    let changelog = format!("{repo}/blob/{tag}/CHANGELOG.md");
    let version = escape(version);
    let mut html = String::new();

    let _ = write!(
        html,
        r#"<section id="latest" class="latest">
<div class="latest-head"><h2>Latest release <span>{version}</span></h2>
<nav><a href="{release}">Release notes</a><a href="{changelog}">Changelog</a><a href="{repo}">GitHub</a></nav></div>
<div class="card">
<h3>Documentation</h3>
<div class="latest-docs">
"#
    );
    for docs in cfg.docs.values() {
        let _ = writeln!(
            html,
            r#"<a href="{}/{}">{}<span>API reference</span></a>"#,
            escape(&docs.pages_path),
            escape(&docs.entry),
            escape(&docs.label),
        );
    }
    html.push_str(
        "</div>\n</div>\n<div class=\"card\">\n<h3>Downloads</h3>\n\
         <div class=\"latest-table\"><table>\n\
         <thead><tr><th>Archive</th><th>Contents</th><th>SHA-256</th></tr></thead>\n<tbody>\n",
    );
    for (name, checksum) in artifacts {
        let _ = writeln!(
            html,
            r#"<tr><td><a href="{repo}/releases/download/{tag}/{name}">{name}</a></td><td>{}</td><td><code title="{checksum}">{checksum}</code></td></tr>"#,
            escape(&contents(cfg, name)),
            name = escape(name),
            checksum = escape(checksum),
        );
    }
    let _ = write!(
        html,
        "</tbody>\n</table></div>\n</div>\n<div class=\"card\">\n\
         <h3>Crates <span class=\"latest-count\">{}</span></h3>\n<ul class=\"latest-crates\">\n",
        crates.len()
    );
    for name in crates {
        let _ = writeln!(
            html,
            r#"<li><span>{name}</span><a href="https://crates.io/crates/{name}/{version}">crates.io</a><a href="https://docs.rs/{name}/{version}">docs.rs</a></li>"#,
            name = escape(name),
        );
    }
    let _ = write!(
        html,
        "</ul>\n</div>\n<p class=\"latest-foot\">Built from \
         <a href=\"{repo}/tree/{tag}\">{tag}</a>.</p>\n</section>\n",
        tag = escape(&tag),
    );
    html
}

/// What an archive carries, by the role the release configuration gives it.
fn contents(cfg: &ReleaseConfig, name: &str) -> String {
    if name == cfg.core_asset {
        return "Swift Package binary target".into();
    }
    if name == cfg.merged_asset {
        return "XCFramework with the Swift layer".into();
    }
    if name == cfg.wasm_asset {
        return "Web demo bundle".into();
    }
    cfg.docs
        .values()
        .find(|docs| docs.asset == name)
        .map_or_else(
            || "Platform library".into(),
            |docs| format!("{} documentation", docs.label),
        )
}

fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for character in text.chars() {
        match character {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            other => out.push(other),
        }
    }
    out
}

/// Painted with the player page's own palette and cards, so the section reads
/// as part of the page it is added to.
const STYLE: &str = "
.latest{margin-top:28px}
.latest-head{display:flex;flex-wrap:wrap;align-items:baseline;gap:6px 18px;margin-bottom:12px}
.latest h2{color:var(--accent);font-size:22px}
.latest h2 span{color:var(--text-muted);font-weight:500}
.latest nav{display:flex;flex-wrap:wrap;gap:16px;font-size:13px}
.latest h3{margin-bottom:10px;color:var(--text-main);font-size:13px}
.latest a{color:var(--accent-strong);text-decoration:none}
.latest a:hover{text-decoration:underline}
.latest-docs{display:grid;grid-template-columns:repeat(auto-fit,minmax(160px,1fr));gap:8px}
.latest-docs a{display:flex;flex-direction:column;gap:2px;padding:10px 12px;border:1px solid var(--line);border-radius:8px;background:var(--bg-dark);color:var(--text-main);font-weight:600}
.latest-docs a:hover{border-color:var(--accent);text-decoration:none}
.latest-docs span{color:var(--text-muted);font-size:12px;font-weight:400}
.latest-table{overflow-x:auto}
.latest table{width:100%;border-collapse:collapse;font-size:13px}
.latest th,.latest td{padding:8px 10px;border-bottom:1px solid var(--line);text-align:left;vertical-align:top}
.latest tr:last-child td{border-bottom:0}
.latest th{color:var(--text-muted);font-size:12px}
.latest td:first-child{white-space:nowrap}
.latest td:nth-child(2){color:var(--text-muted)}
.latest code{display:inline-block;max-width:18ch;overflow:hidden;text-overflow:ellipsis;white-space:nowrap;vertical-align:bottom;color:var(--text-muted);font:12px/1.6 monospace}
.latest-count{padding:1px 8px;border-radius:999px;background:color-mix(in srgb,var(--accent) 18%,transparent);color:var(--accent-strong);font-size:12px}
.latest-crates{display:grid;grid-template-columns:repeat(auto-fill,minmax(250px,1fr));gap:6px;list-style:none}
.latest-crates li{display:flex;align-items:center;gap:10px;padding:7px 10px;border:1px solid var(--line);border-radius:6px;background:var(--bg-dark);font-size:12px}
.latest-crates span{flex:1;min-width:0;overflow:hidden;text-overflow:ellipsis;white-space:nowrap;color:var(--text-main);font:13px/1.6 monospace}
.latest-foot{margin-top:4px;color:var(--text-muted);font-size:12px}
";

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::config::DocsChannel;

    fn config() -> ReleaseConfig {
        ReleaseConfig {
            title: "Kithara".into(),
            github_repo: "zvuk/kithara".into(),
            gitlab_host: "gitlab.internal".into(),
            core_asset: "KitharaFFIInternal.xcframework.zip".into(),
            wasm_asset: "kithara-wasm-pages.zip".into(),
            docs: BTreeMap::from([(
                "apple".to_string(),
                DocsChannel {
                    asset: "Kithara-docs.zip".into(),
                    label: "iOS".into(),
                    pages_path: "docs/ios".into(),
                    entry: "documentation/".into(),
                    ..DocsChannel::default()
                },
            )]),
            ..ReleaseConfig::default()
        }
    }

    #[test]
    fn the_site_is_served_under_the_repository_name() {
        let cfg = config();

        assert_eq!(
            pages_url("Zvuk/kithara").unwrap(),
            "https://zvuk.github.io/kithara/"
        );
        assert_eq!(hosting_base(&cfg, "apple").unwrap(), "kithara/docs/ios");
        assert!(hosting_base(&cfg, "missing").is_err());
    }

    #[test]
    fn the_release_section_links_every_release_surface() {
        let section = section(
            &config(),
            "0.0.2",
            &["kithara".into(), "kithara-net".into()],
            &[("KitharaFFIInternal.xcframework.zip".into(), "abc".into())],
        );

        for link in [
            r#"href="docs/ios/documentation/""#,
            "https://github.com/zvuk/kithara/releases/tag/v0.0.2",
            "https://github.com/zvuk/kithara/releases/download/v0.0.2/KitharaFFIInternal.xcframework.zip",
            "https://crates.io/crates/kithara-net/0.0.2",
            "https://docs.rs/kithara-net/0.0.2",
        ] {
            assert!(section.contains(link), "{link} missing from the section");
        }
        assert!(section.contains("Swift Package binary target"));
        assert!(section.contains(r#"<span class="latest-count">2</span>"#));
        assert!(!section.contains("gitlab.internal"));
    }

    /// The section lands inside the player's own page and paints with the
    /// palette that page declares.
    #[test]
    fn the_release_section_joins_the_player_page() {
        let player = fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../crates/kithara-ffi/index.html"),
        )
        .expect("the player page");

        let page = with_latest(&player, "<section id=\"latest\"></section>\n").unwrap();
        let style = page.find("<style>\n.latest{").expect("section style");
        let section = page.find("<section id=\"latest\">").expect("section");
        assert!(style < page.find("</head>").unwrap());
        assert!(section < page.find("</body>").unwrap());
        assert!(page.contains(r#"id="playlist""#));

        for name in STYLE
            .split("var(--")
            .skip(1)
            .filter_map(|rest| rest.split_once(')'))
            .map(|(name, _)| name)
        {
            assert!(
                player.contains(&format!("--{name}:")),
                "the player page declares no --{name}"
            );
        }
        assert!(with_latest("<html><body></body></html>", "").is_err());
    }

    #[test]
    fn page_text_is_escaped() {
        assert_eq!(
            escape(r#"<a href="x">&'"#),
            "&lt;a href=&quot;x&quot;&gt;&amp;&#39;"
        );
    }
}
