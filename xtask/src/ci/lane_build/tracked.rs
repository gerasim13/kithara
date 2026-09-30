//! What the checkout's git index says it holds.

use std::{
    collections::{BTreeMap, BTreeSet},
    ops::Bound,
    path::Path,
    process::Command,
};

use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};

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

/// The content a watched path names: a tracked file's blobs, or for a
/// directory a digest of every tracked path and blob under it, so a file
/// appearing, going or changing renames it. `None` when git tracks nothing
/// there.
pub(super) fn oid(tracked: &Sources, path: &str) -> Option<String> {
    if let Some(blobs) = tracked.get(path) {
        return Some(
            blobs
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>()
                .join(","),
        );
    }
    let prefix = if path.is_empty() {
        String::new()
    } else {
        format!("{path}/")
    };
    let mut digest = Sha256::new();
    let mut any = false;
    for (entry, blobs) in tracked
        .range::<str, _>((Bound::Included(prefix.as_str()), Bound::Unbounded))
        .take_while(|(entry, _)| entry.starts_with(&prefix))
    {
        for blob in blobs {
            digest.update(entry.as_bytes());
            digest.update(b"\0");
            digest.update(blob.as_bytes());
            digest.update(b"\0");
        }
        any = true;
    }
    any.then(|| hex::encode(digest.finalize()))
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

    #[test]
    fn a_file_is_named_by_its_blob() {
        let tracked = sources(&[("src/lib.rs", "aaa"), ("src/lib.rs", "bbb")]);

        assert_eq!(oid(&tracked, "src/lib.rs").as_deref(), Some("aaa,bbb"));
    }

    /// A directory's name changes when a file under it changes, appears or
    /// goes, and never when a sibling that merely shares its prefix does.
    #[test]
    fn a_directory_is_named_by_everything_under_it() {
        let before = sources(&[("src/a.rs", "1"), ("srcx.rs", "2")]);
        let changed = sources(&[("src/a.rs", "3"), ("srcx.rs", "2")]);
        let added = sources(&[("src/a.rs", "1"), ("src/b.rs", "4"), ("srcx.rs", "2")]);
        let sibling = sources(&[("src/a.rs", "1"), ("srcx.rs", "5")]);

        let name = oid(&before, "src");
        assert!(name.is_some());
        assert_ne!(oid(&changed, "src"), name);
        assert_ne!(
            oid(&added, "src"),
            name,
            "a file appearing or going renames it"
        );
        assert_eq!(oid(&sibling, "src"), name);
        assert_ne!(
            oid(&sibling, ""),
            oid(&before, ""),
            "the root names everything"
        );
        assert_eq!(oid(&before, "assets"), None);
    }
}
