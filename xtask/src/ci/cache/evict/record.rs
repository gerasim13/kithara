use std::collections::HashMap;

use anyhow::{Context, Result, ensure};

use super::entry::Entry;
use crate::consts::{RECENCY_RECORD_BYTES, RECENCY_RECORD_VERSION};

pub(super) fn encode(reads: &HashMap<Entry, u64>) -> Vec<u8> {
    let mut sorted = reads.iter().collect::<Vec<_>>();
    sorted.sort_unstable();
    let mut bytes = Vec::with_capacity(1 + sorted.len() * RECENCY_RECORD_BYTES);
    bytes.push(RECENCY_RECORD_VERSION);
    for (entry, read) in sorted {
        bytes.extend_from_slice(&<[u8; 32]>::from(*entry));
        bytes.extend_from_slice(&read.to_be_bytes());
    }
    bytes
}

/// A record that does not read back whole is refused rather than read in
/// part: the evictor then starts from no reads, which only ages entries.
pub(super) fn decode(bytes: &[u8]) -> Result<HashMap<Entry, u64>> {
    let (&version, body) = bytes.split_first().context("the record is empty")?;
    ensure!(
        version == RECENCY_RECORD_VERSION,
        "the record is format {version}, not {RECENCY_RECORD_VERSION}"
    );
    let records = body.chunks_exact(RECENCY_RECORD_BYTES);
    ensure!(
        records.remainder().is_empty(),
        "the record ends partway through an entry"
    );
    records
        .map(|record| {
            let (hash, read) = record.split_at(32);
            Ok((
                Entry::from(<[u8; 32]>::try_from(hash)?),
                u64::from_be_bytes(read.try_into()?),
            ))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_record_reads_back_what_was_written() {
        let reads = HashMap::from([(Entry::from([2; 32]), 7), (Entry::from([1; 32]), u64::MAX)]);

        assert_eq!(decode(&encode(&reads)).unwrap(), reads);
        assert_eq!(decode(&encode(&HashMap::new())).unwrap(), HashMap::new());
    }

    #[test]
    fn a_record_that_does_not_read_back_whole_is_refused() {
        let mut bytes = encode(&HashMap::from([(Entry::from([1; 32]), 7)]));

        assert!(decode(&bytes[..bytes.len() - 1]).is_err());
        assert!(decode(&[]).is_err());
        bytes[0] = RECENCY_RECORD_VERSION + 1;
        assert!(decode(&bytes).is_err());
    }
}
