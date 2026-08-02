#![forbid(unsafe_code)]

use kithara_platform::time::Duration;

pub type VariantIndex = usize;

/// Sum `durations[..endpoint]` when `endpoint` is a valid slice endpoint
/// (`endpoint <= durations.len()`), else `None`.
///
/// `endpoint` is the *exclusive end* of a prefix, not a segment index:
/// `endpoint == len` legitimately means "at/after the last segment" and
/// must sum the full slice. Only `endpoint > len` is impossible/corrupt.
/// This replaces the silent `.min(len)` clamp in `Abr::progress`, which
/// masked an out-of-range endpoint as a full-prefix sum.
pub(crate) fn duration_prefix(durations: &[Duration], endpoint: usize) -> Option<Duration> {
    durations
        .get(..endpoint)
        .map(|prefix| prefix.iter().copied().sum())
}

#[cfg(test)]
mod tests {
    use kithara_platform::time::Duration;
    use kithara_test_utils::kithara;

    use super::duration_prefix;

    #[kithara::test]
    fn duration_prefix_endpoint_semantics() {
        let durations = [
            Duration::from_secs(1),
            Duration::from_secs(2),
            Duration::from_secs(3),
        ];
        assert_eq!(duration_prefix(&durations, 0), Some(Duration::ZERO));
        assert_eq!(duration_prefix(&durations, 1), Some(Duration::from_secs(1)));
        assert_eq!(duration_prefix(&durations, 2), Some(Duration::from_secs(3)));
        // endpoint == len: full sum, Some (the preserved boundary case).
        assert_eq!(duration_prefix(&durations, 3), Some(Duration::from_secs(6)));
        // endpoint > len: impossible, surfaced as None (no silent clamp).
        assert_eq!(duration_prefix(&durations, 4), None);
    }
}
