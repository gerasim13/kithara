use std::ops::Range;

use kithara_mp4::{Fmp4Layout, Fragment, ReadAt};
use kithara_platform::time::Duration;
use kithara_stream::SegmentDescriptor;

/// Pre-computed fragmented-mp4 layout for a fully cached file.
#[derive(Clone, Debug)]
pub(crate) struct FileSegmentIndex {
    init_range: Range<u64>,
    segments: Vec<SegmentDescriptor>,
}

impl FileSegmentIndex {
    pub(crate) fn init_range(&self) -> Range<u64> {
        self.init_range.clone()
    }

    pub(crate) fn segment_after_byte(&self, byte_offset: u64) -> Option<SegmentDescriptor> {
        self.segments
            .iter()
            .find(|desc| desc.byte_range.start >= byte_offset)
            .cloned()
    }

    pub(crate) fn segment_at_time(&self, t: Duration) -> Option<SegmentDescriptor> {
        let by_time = self
            .segments
            .iter()
            .find(|desc| t < desc.decode_time.saturating_add(desc.duration));
        let last = self.segments.last();
        by_time.or(last).cloned()
    }

    pub(crate) fn segment_count(&self) -> u32 {
        let n = self.segments.len();
        u32::try_from(n).unwrap_or_else(|_| {
            tracing::error!(segment_count = n, "BUG: fragment count exceeds u32::MAX");
            0
        })
    }

    /// Try to derive a fragmented-mp4 index for the `total`-byte file behind
    /// `source`. The walk lives in `kithara-mp4` and seeks over `mdat`, so a
    /// long track costs no more than a short one; what happens here is the
    /// projection of its media ticks onto the stream layer's vocabulary.
    ///
    /// Returns `None` when the file yields no fragment layout, or when it
    /// holds more fragments than a segment index can address.
    pub(crate) fn try_build<R: ReadAt>(source: &R, total: u64) -> Option<Self> {
        let layout = Fmp4Layout::read(source, total)?;
        let timescale = layout.timescale();

        let mut segments: Vec<SegmentDescriptor> = Vec::with_capacity(layout.fragments().len());
        for (idx, fragment) in layout.fragments().iter().enumerate() {
            let segment_index = u32::try_from(idx).ok()?;
            segments.push(descriptor(fragment, segment_index, timescale));
        }

        Some(Self {
            init_range: layout.init_range(),
            segments,
        })
    }
}

/// Project one fragment onto the descriptor the stream layer speaks. A plain
/// file carries a single variant, so `variant_index` is always zero.
fn descriptor(fragment: &Fragment, segment_index: u32, timescale: u32) -> SegmentDescriptor {
    SegmentDescriptor::new(
        fragment.byte_range.clone(),
        ticks_to_duration(fragment.decode_ticks, timescale),
        ticks_to_duration(fragment.duration_ticks, timescale),
        segment_index,
        0,
    )
}

fn ticks_to_duration(ticks: u64, timescale: u32) -> Duration {
    const NANOS_PER_SEC: u64 = 1_000_000_000;
    const NANOS_PER_SEC_MINUS_ONE: u32 = 999_999_999;
    if timescale == 0 {
        return Duration::ZERO;
    }
    let secs = ticks / u64::from(timescale);
    let rem = ticks % u64::from(timescale);
    let nanos = rem.saturating_mul(NANOS_PER_SEC) / u64::from(timescale);
    let nanos_u32 = u32::try_from(nanos).unwrap_or(NANOS_PER_SEC_MINUS_ONE);
    Duration::new(secs, nanos_u32)
}
