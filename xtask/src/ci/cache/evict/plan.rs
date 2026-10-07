use std::collections::HashMap;

use super::entry::Entry;

/// What one listing found of an entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Stored {
    pub(super) size: u64,
    pub(super) written_s: u64,
}

/// Everything a scope's bucket holds, as one complete listing saw it.
#[derive(Debug, Default)]
pub(super) struct Listing {
    pub(super) entries: HashMap<Entry, Stored>,
    /// Bytes of every object that is not an entry. They count against the
    /// budget and are never evicted.
    pub(super) other_bytes: u64,
}

impl Listing {
    /// Counts one listed object: an entry by its hash, anything else by its
    /// size alone.
    pub(super) fn insert(&mut self, object: &str, stored: Stored) {
        match Entry::parse(object) {
            Some(entry) => {
                self.entries.insert(entry, stored);
            }
            None => self.other_bytes = self.other_bytes.saturating_add(stored.size),
        }
    }

    pub(super) fn used(&self) -> u64 {
        self.entries
            .values()
            .map(|stored| stored.size)
            .fold(self.other_bytes, u64::saturating_add)
    }
}

/// An entry chosen for eviction, and what it was chosen by.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Victim {
    pub(super) entry: Entry,
    pub(super) size: u64,
    /// Its last use: the later of its write and its last read.
    pub(super) used_s: u64,
    /// Whether anything read it since it was written.
    pub(super) read: bool,
}

/// The entries to evict, used longest ago first.
///
/// Nothing goes until the bucket holds four fifths of its quota, and then
/// enough goes to bring it down to thirteen twentieths: a margin wide enough
/// that a pass every half hour stays ahead of a day's writes, and narrow
/// enough that what stays is what the fleet still reads.
pub(super) fn victims(listing: &Listing, reads: &HashMap<Entry, u64>, quota: u64) -> Vec<Victim> {
    let mut used = listing.used();
    if used < quota / 5 * 4 {
        return Vec::new();
    }
    let mut candidates = listing
        .entries
        .iter()
        .map(|(&entry, stored)| {
            let read = reads.get(&entry).copied();
            Victim {
                entry,
                size: stored.size,
                used_s: read.map_or(stored.written_s, |read| read.max(stored.written_s)),
                read: read.is_some_and(|read| read > stored.written_s),
            }
        })
        .collect::<Vec<_>>();
    candidates.sort_unstable_by_key(|victim| (victim.used_s, victim.entry));
    let target = quota / 20 * 13;
    let mut chosen = Vec::new();
    for victim in candidates {
        if used <= target {
            break;
        }
        used = used.saturating_sub(victim.size);
        chosen.push(victim);
    }
    chosen
}

/// The victims nothing has read since they were chosen.
pub(super) fn still_unused(victims: &[Victim], reads: &HashMap<Entry, u64>) -> Vec<Victim> {
    victims
        .iter()
        .filter(|victim| {
            reads
                .get(&victim.entry)
                .is_none_or(|&read| read <= victim.used_s)
        })
        .copied()
        .collect()
}

/// Forgets every read the listing already accounts for: a read of an entry
/// that is gone, and a read older than the entry's own write.
pub(super) fn prune(reads: &mut HashMap<Entry, u64>, listing: &Listing) {
    reads.retain(|entry, read| {
        listing
            .entries
            .get(entry)
            .is_some_and(|stored| *read > stored.written_s)
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(byte: u8) -> Entry {
        Entry::from([byte; 32])
    }

    fn listing(entries: &[(u8, u64, u64)], other_bytes: u64) -> Listing {
        Listing {
            entries: entries
                .iter()
                .map(|&(byte, size, written_s)| (entry(byte), Stored { size, written_s }))
                .collect(),
            other_bytes,
        }
    }

    #[test]
    fn a_listed_object_is_an_entry_only_when_sccache_wrote_it() {
        let hash = crate::consts::ENTRY_HASH;
        let mut listing = Listing::default();

        listing.insert(
            &format!("sccache/a/b/c/{hash}"),
            Stored {
                size: 300,
                written_s: 100,
            },
        );
        listing.insert(
            "sccache/.sccache_check",
            Stored {
                size: 13,
                written_s: 100,
            },
        );

        assert_eq!(listing.entries.len(), 1);
        assert_eq!(listing.other_bytes, 13);
        assert_eq!(listing.used(), 313);
    }

    #[test]
    fn a_bucket_below_four_fifths_of_its_quota_keeps_everything() {
        let listing = listing(&[(1, 300, 100), (2, 300, 200)], 199);

        assert!(victims(&listing, &HashMap::new(), 1000).is_empty());
    }

    /// The age rule this replaces measured a write, so the entries every
    /// build reads - the oldest written - were the first to expire.
    #[test]
    fn eviction_takes_what_was_used_longest_ago_until_thirteen_twentieths_remain() {
        let listing = listing(&[(1, 300, 100), (2, 300, 200), (3, 300, 300)], 100);
        let reads = HashMap::from([(entry(1), 400)]);

        let chosen = victims(&listing, &reads, 1000);

        assert_eq!(
            chosen,
            [
                Victim {
                    entry: entry(2),
                    size: 300,
                    used_s: 200,
                    read: false,
                },
                Victim {
                    entry: entry(3),
                    size: 300,
                    used_s: 300,
                    read: false,
                },
            ]
        );
    }

    /// The startup probe and the snapshot layers fill the budget like any
    /// object, but evicting the probe turns a lane's cache read-only and the
    /// snapshots answer to their own retention.
    #[test]
    fn what_is_not_an_entry_counts_but_is_never_chosen() {
        let listing = listing(&[(1, 100, 100)], 900);

        let chosen = victims(&listing, &HashMap::new(), 1000);

        assert_eq!(
            chosen.iter().map(|victim| victim.entry).collect::<Vec<_>>(),
            [entry(1)]
        );
        assert_eq!(listing.used(), 1000);
    }

    #[test]
    fn a_victim_read_after_it_was_chosen_is_kept() {
        let listing = listing(&[(1, 300, 100), (2, 300, 200), (3, 300, 300)], 100);
        let chosen = victims(&listing, &HashMap::new(), 1000);
        let reads = HashMap::from([(entry(1), 500)]);

        let still = still_unused(&chosen, &reads);

        assert!(chosen.iter().any(|victim| victim.entry == entry(1)));
        assert!(still.iter().all(|victim| victim.entry != entry(1)));
        assert_eq!(still.len(), chosen.len() - 1);
    }

    #[test]
    fn a_read_the_listing_already_accounts_for_is_forgotten() {
        let listing = listing(&[(1, 1, 100), (2, 1, 200)], 0);
        let mut reads = HashMap::from([(entry(1), 150), (entry(2), 150), (entry(3), 150)]);

        prune(&mut reads, &listing);

        assert_eq!(reads, HashMap::from([(entry(1), 150)]));
    }
}
