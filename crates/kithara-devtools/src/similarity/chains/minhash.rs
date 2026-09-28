//! Token shingles, their Jaccard similarity, and `MinHash` banding that picks
//! the pairs worth comparing.

use std::collections::{BTreeSet, HashMap};

mod consts {
    pub(super) const BANDS: usize = 32;
    pub(super) const ROWS: usize = 2;
    /// A bucket holding more sets than this is boilerplate every function shares.
    pub(super) const MAX_BUCKET: usize = 200;
    /// A token sequence shorter than this share of the other one is not alike.
    pub(super) const MIN_LENGTH_RATIO: f64 = 0.5;
    pub(super) const SEED: u64 = 7;
    pub(super) const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    pub(super) const FNV_PRIME: u64 = 0x0100_0000_01b3;
    pub(super) const SEPARATOR: u8 = 0x1f;
}

type Signature = [[u64; consts::ROWS]; consts::BANDS];

/// Hashes of every `width` consecutive tokens; a shorter sequence is one shingle.
pub(super) fn shingles(tokens: &[String], width: usize) -> BTreeSet<u64> {
    if tokens.len() <= width {
        return BTreeSet::from([fnv(tokens)]);
    }
    tokens.windows(width.max(1)).map(fnv).collect()
}

fn fnv(window: &[String]) -> u64 {
    let mut hash = consts::FNV_OFFSET;
    for (index, token) in window.iter().enumerate() {
        let separator = (index > 0).then_some(consts::SEPARATOR);
        for byte in separator.into_iter().chain(token.bytes()) {
            hash = (hash ^ u64::from(byte)).wrapping_mul(consts::FNV_PRIME);
        }
    }
    hash
}

pub(super) fn jaccard(a: &BTreeSet<u64>, b: &BTreeSet<u64>) -> f64 {
    let common = a.intersection(b).count();
    ratio(common, a.len() + b.len() - common)
}

/// Share of the smaller set found in the larger one.
pub(super) fn containment(a: &BTreeSet<u64>, b: &BTreeSet<u64>) -> f64 {
    ratio(a.intersection(b).count(), a.len().min(b.len()))
}

/// Jaccard of two functions' shingles, or none when one token sequence is
/// less than half as long as the other.
pub(super) fn similarity(lengths: (usize, usize), a: &BTreeSet<u64>, b: &BTreeSet<u64>) -> f64 {
    if ratio(lengths.0.min(lengths.1), lengths.0.max(lengths.1)) < consts::MIN_LENGTH_RATIO {
        return 0.0;
    }
    jaccard(a, b)
}

/// `part / whole`, and none of an empty whole.
pub(super) fn ratio(part: usize, whole: usize) -> f64 {
    if whole == 0 {
        return 0.0;
    }
    count_f64(part) / count_f64(whole)
}

fn count_f64(value: usize) -> f64 {
    u32::try_from(value).map_or_else(|_| f64::from(u32::MAX), f64::from)
}

/// Pairs of ids whose sets agree on every row of at least one band.
pub(super) fn candidates(sets: &[(usize, &BTreeSet<u64>)]) -> BTreeSet<(usize, usize)> {
    let seeds = seeds();
    let mut buckets: HashMap<(usize, [u64; consts::ROWS]), Vec<usize>> = HashMap::new();
    for &(id, set) in sets {
        for (band, rows) in signature(set, &seeds).into_iter().enumerate() {
            buckets.entry((band, rows)).or_default().push(id);
        }
    }
    let mut pairs = BTreeSet::new();
    let shared = buckets
        .values()
        .filter(|ids| (2..=consts::MAX_BUCKET).contains(&ids.len()));
    for ids in shared {
        for (index, &x) in ids.iter().enumerate() {
            for &y in ids.iter().skip(index + 1) {
                pairs.insert((x.min(y), x.max(y)));
            }
        }
    }
    pairs
}

fn seeds() -> Signature {
    let mut seeds = [[0; consts::ROWS]; consts::BANDS];
    let mut state = consts::SEED;
    for slot in seeds.iter_mut().flatten() {
        state = splitmix(state);
        *slot = state;
    }
    seeds
}

/// The least seeded hash of the set under each row's hash function.
fn signature(set: &BTreeSet<u64>, seeds: &Signature) -> Signature {
    let mut signature = [[u64::MAX; consts::ROWS]; consts::BANDS];
    for &shingle in set {
        for (slot, seed) in signature.iter_mut().flatten().zip(seeds.iter().flatten()) {
            *slot = (*slot).min(splitmix(shingle ^ seed));
        }
    }
    signature
}

const fn splitmix(state: u64) -> u64 {
    let mut z = state.wrapping_add(0x9e37_79b9_7f4a_7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tokens(text: &str) -> Vec<String> {
        text.split_whitespace().map(str::to_string).collect()
    }

    #[test]
    fn equal_sequences_are_candidates_with_full_similarity() {
        let body = tokens("let value = self . source . read ( buf ) ? ; self . offset += value ;");
        let other = tokens("match kind { Kind :: A => one ( ) , Kind :: B => two ( ) , }");
        let (a, b, c) = (shingles(&body, 3), shingles(&body, 3), shingles(&other, 3));
        assert!((jaccard(&a, &b) - 1.0).abs() < f64::EPSILON);
        assert!(jaccard(&a, &c) < 0.1);
        let pairs = candidates(&[(0, &a), (1, &b), (2, &c)]);
        assert!(pairs.contains(&(0, 1)));
        assert!(!pairs.contains(&(0, 2)));
    }

    #[test]
    fn a_much_shorter_sequence_is_not_alike() {
        let long = tokens("a b c d e f g h i j");
        let short = tokens("a b c d");
        let (a, b) = (shingles(&long, 3), shingles(&short, 3));
        assert!(jaccard(&a, &b) > 0.0);
        assert!(similarity((long.len(), short.len()), &a, &b).abs() < f64::EPSILON);
    }
}
