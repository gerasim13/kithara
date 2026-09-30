//! What the checkout's git index says it holds.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
    process::Command,
};

use anyhow::{Context, Result, bail};

/// Git blob ids per tracked path.
pub(super) type Sources = BTreeMap<String, BTreeSet<String>>;

/// The checkout's tracked files. A hook or a caller may point git at another
/// repository's index through its environment, so the listing drops those
/// variables and names this checkout alone.
pub(super) fn list(project_root: &Path) -> Result<Sources> {
    let output = Command::new("git")
        .current_dir(project_root)
        .env_remove("GIT_DIR")
        .env_remove("GIT_INDEX_FILE")
        .env_remove("GIT_WORK_TREE")
        .args(["ls-files", "--stage", "-z"])
        .output()
        .context("listing the checkout's tracked files")?;
    if !output.status.success() {
        bail!(
            "git ls-files failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(parse_stage(&String::from_utf8_lossy(&output.stdout)))
}

/// `git ls-files --stage -z` entries are `<mode> <blob> <stage>\t<path>`.
/// Symlinks and submodules carry no content cargo reads through them.
fn parse_stage(listed: &str) -> Sources {
    let mut sources = Sources::new();
    for entry in listed.split('\0') {
        let Some((meta, path)) = entry.split_once('\t') else {
            continue;
        };
        let mut meta = meta.split(' ');
        let (Some(mode), Some(blob)) = (meta.next(), meta.next()) else {
            continue;
        };
        if mode == "120000" || mode == "160000" || path.contains('\n') {
            continue;
        }
        sources
            .entry(path.to_owned())
            .or_default()
            .insert(blob.to_owned());
    }
    sources
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ci::lane_build::fixture::sources;

    #[test]
    fn stage_listing_skips_links_and_submodules() {
        let listed = "100644 aaa 0\tsrc/lib.rs\0120000 bbb 0\tlink\0160000 ccc 0\tvendor\0";

        assert_eq!(parse_stage(listed), sources(&[("src/lib.rs", "aaa")]));
    }
}
