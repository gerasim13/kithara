use crate::consts;

/// One compiler-cache object, named by the hash sccache stores it under.
///
/// sccache writes `<prefix>/<h0>/<h1>/<h2>/<hash>`, the first three characters
/// of the hash repeated as directories. Everything else in a scope's bucket -
/// the startup probe, the snapshot layers, the evictor's own marker - is not
/// an entry, so it is counted against the budget and never evicted.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, derive_more::From, derive_more::Into,
)]
pub(super) struct Entry([u8; 32]);

impl Entry {
    pub(super) fn parse(object: &str) -> Option<Self> {
        let path = object
            .strip_prefix(consts::SCCACHE_PREFIX)?
            .strip_prefix('/')?;
        let mut parts = path.split('/');
        let shards = [parts.next()?, parts.next()?, parts.next()?];
        let hash = parts.next()?;
        if parts.next().is_some()
            || hash.len() != 64
            || !hash
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            || shards
                .iter()
                .zip(hash.as_bytes())
                .any(|(shard, character)| shard.as_bytes() != [*character])
        {
            return None;
        }
        let mut bytes = [0; 32];
        hex::decode_to_slice(hash, &mut bytes).ok()?;
        Some(Self(bytes))
    }

    pub(super) fn object(&self) -> String {
        let hash = hex::encode(self.0);
        format!(
            "{}/{}/{}/{}/{hash}",
            consts::SCCACHE_PREFIX,
            &hash[..1],
            &hash[1..2],
            &hash[2..3]
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::consts::ENTRY_HASH;

    #[test]
    fn an_entry_reads_back_as_the_object_sccache_wrote() {
        let object = format!("sccache/a/b/c/{ENTRY_HASH}");

        let entry = Entry::parse(&object).expect("sccache's own layout");

        assert_eq!(entry.object(), object);
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
            ".evict/recount".to_owned(),
        ] {
            assert_eq!(Entry::parse(&object), None, "{object}");
        }
    }
}
