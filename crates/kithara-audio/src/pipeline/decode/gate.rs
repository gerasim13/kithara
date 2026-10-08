#[cfg(test)]
mod tests {
    use std::ops::Range;

    use kithara_platform::time::Duration;
    use kithara_stream::{ByteMap, SegmentDescriptor};
    use kithara_test_utils::kithara;

    use super::forward_window;
    use crate::consts;

    /// Equal media segments behind an init range, as HLS delivers them.
    struct SegmentedMap;

    impl SegmentedMap {
        const COUNT: u32 = 3;
        const INIT_BYTES: u64 = 627;
        const MID_SEGMENT_BYTE: u64 = Self::INIT_BYTES + Self::SEGMENT_BYTES / 2;
        const SEGMENT_BYTES: u64 = 8_000;
        const SEGMENT_SECS: u64 = 4;

        fn descriptor(index: u32) -> SegmentDescriptor {
            let start = Self::INIT_BYTES + u64::from(index) * Self::SEGMENT_BYTES;
            SegmentDescriptor::new(
                start..start + Self::SEGMENT_BYTES,
                Duration::from_secs(u64::from(index) * Self::SEGMENT_SECS),
                Duration::from_secs(Self::SEGMENT_SECS),
                index,
                0,
            )
        }

        fn segment_start(index: u32) -> u64 {
            Self::INIT_BYTES + u64::from(index) * Self::SEGMENT_BYTES
        }
    }

    impl ByteMap for SegmentedMap {
        fn init_segment_range(&self) -> Range<u64> {
            0..Self::INIT_BYTES
        }

        fn len(&self) -> Option<u64> {
            Some(Self::segment_start(Self::COUNT))
        }

        fn segment_after_byte(&self, byte_offset: u64) -> Option<SegmentDescriptor> {
            (0..Self::COUNT)
                .map(Self::descriptor)
                .find(|segment| segment.byte_range.start >= byte_offset)
        }

        fn segment_at_byte(&self, byte_offset: u64) -> Option<SegmentDescriptor> {
            (0..Self::COUNT)
                .map(Self::descriptor)
                .find(|segment| segment.byte_range.contains(&byte_offset))
        }

        fn segment_at_time(&self, t: Duration) -> Option<SegmentDescriptor> {
            (0..Self::COUNT)
                .map(Self::descriptor)
                .find(|segment| segment.decode_time + segment.duration > t)
        }

        fn segment_count(&self) -> Option<u32> {
            Some(Self::COUNT)
        }
    }

    #[kithara::test(native, flash(false))]
    fn a_wait_on_a_segment_boundary_ends_at_that_segment() {
        let map: &dyn ByteMap = &SegmentedMap;

        let window = forward_window(SegmentedMap::segment_start(0), Some(map), map.len());

        assert_eq!(
            window,
            SegmentedMap::segment_start(0)..SegmentedMap::segment_start(1)
        );
    }

    #[kithara::test(native, flash(false))]
    fn a_wait_inside_a_segment_ends_at_its_read_boundary() {
        let map: &dyn ByteMap = &SegmentedMap;

        let window = forward_window(SegmentedMap::MID_SEGMENT_BYTE, Some(map), map.len());

        assert_eq!(window.end, SegmentedMap::segment_start(1));
    }

    #[kithara::test(native, flash(false))]
    fn a_wait_on_a_source_with_no_segments_spans_the_read_ahead_window() {
        let pos = SegmentedMap::segment_start(0);

        let window = forward_window(pos, None, None);

        assert_eq!(window, pos..pos + consts::DEFAULT_READ_AHEAD_BYTES);
    }
}
