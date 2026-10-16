use crate::consts;

/// One compiler-cache entry, named by the hash its compiler cache stores it
/// under.
///
/// sccache writes `<prefix>/<h0>/<h1>/<h2>/<hash>`, the first three characters
/// of the hash repeated as directories. kache writes one compile as
/// `<prefix>/v3/manifests/<crate>/<hash>.json` and
/// `<prefix>/v3/packs/<crate>/<hash>.tar.zst`, one entry in two objects.
/// Everything else in a scope's bucket - the startup probe, kache's build
/// index and predictions, the snapshot layers - is not an entry, so it is
/// counted against the budget and never evicted.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, derive_more::From, derive_more::Into,
)]
pub(super) struct Entry([u8; 32]);

impl Entry {
    pub(super) fn parse(object: &str) -> Option<Self> {
        let (prefix, path) = object.split_once('/')?;
        let hash = match prefix {
            consts::SCCACHE_PREFIX => sccache_hash(path)?,
            consts::KACHE_PREFIX => kache_hash(path)?,
            _ => return None,
        };
        if hash.len() != 64
            || !hash
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return None;
        }
        let mut bytes = [0; 32];
        hex::decode_to_slice(hash, &mut bytes).ok()?;
        Some(Self(bytes))
    }
}

fn sccache_hash(path: &str) -> Option<&str> {
    let mut parts = path.split('/');
    let shards = [parts.next()?, parts.next()?, parts.next()?];
    let hash = parts.next()?;
    let sharded = parts.next().is_none()
        && shards
            .iter()
            .zip(hash.as_bytes())
            .all(|(shard, character)| shard.as_bytes() == [*character]);
    sharded.then_some(hash)
}

fn kache_hash(path: &str) -> Option<&str> {
    let mut parts = path.split('/');
    let (version, kind, _crate, file) =
        (parts.next()?, parts.next()?, parts.next()?, parts.next()?);
    if version != "v3" || parts.next().is_some() {
        return None;
    }
    match kind {
        "manifests" => file.strip_suffix(".json"),
        "packs" => file.strip_suffix(".tar.zst"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::consts::ENTRY_HASH;

    #[test]
    fn an_entry_is_named_by_the_hash_sccache_stored_it_under() {
        let entry =
            Entry::parse(&format!("sccache/a/b/c/{ENTRY_HASH}")).expect("sccache's own layout");

        assert_eq!(hex::encode(<[u8; 32]>::from(entry)), ENTRY_HASH);
    }

    /// kache stores one compile as a manifest and a pack under the same key,
    /// and neither is of use without the other.
    #[test]
    fn a_kache_manifest_and_its_pack_are_one_entry() {
        let manifest = Entry::parse(&format!("kache/v3/manifests/serde/{ENTRY_HASH}.json"));
        let pack = Entry::parse(&format!("kache/v3/packs/serde/{ENTRY_HASH}.tar.zst"));

        assert!(manifest.is_some());
        assert_eq!(manifest, pack);
        assert_eq!(
            manifest,
            Entry::parse(&format!("sccache/a/b/c/{ENTRY_HASH}"))
        );
    }

    /// kache's build index and predictions are rewritten in place by every
    /// build that reads them, so they are never anyone's victim.
    #[test]
    fn only_what_kache_stores_per_compile_is_an_entry() {
        for object in [
            "kache/_manifests/id/75448a2dcb98e888/aarch64-apple-darwin/debug.json".to_owned(),
            format!("kache/v3/predictions/{ENTRY_HASH}.json"),
            format!("kache/v3/manifests/{ENTRY_HASH}.json"),
            format!("kache/v3/manifests/serde/{ENTRY_HASH}.tar.zst"),
            format!("kache/v3/packs/serde/{ENTRY_HASH}.json"),
            format!("kache/v3/manifests/serde/extra/{ENTRY_HASH}.json"),
            "kache/v3/manifests/serde/notes.txt".to_owned(),
            format!("elsewhere/v3/manifests/serde/{ENTRY_HASH}.json"),
        ] {
            assert_eq!(Entry::parse(&object), None, "{object}");
        }
    }

    /// The startup probe sits directly under the prefix and the snapshot
    /// layers beside it. Read as entries, they would be evicted, and a lane
    /// whose probe is gone starts its compiler cache read-only.
    #[test]
    fn only_what_sccache_writes_is_an_entry() {
        for object in [
            "sccache/.sccache_check".to_owned(),
            format!("sccache/a/b/{ENTRY_HASH}"),
            format!("sccache/a/b/d/{ENTRY_HASH}"),
            format!("sccache/a/b/c/{}", ENTRY_HASH.to_ascii_uppercase()),
            format!("sccache/a/b/c/{ENTRY_HASH}/more"),
            format!("sccache/a/b/c/{}", &ENTRY_HASH[..63]),
            format!("elsewhere/a/b/c/{ENTRY_HASH}"),
            format!("target-snapshots/{ENTRY_HASH}/{ENTRY_HASH}.tar"),
        ] {
            assert_eq!(Entry::parse(&object), None, "{object}");
        }
    }
}
