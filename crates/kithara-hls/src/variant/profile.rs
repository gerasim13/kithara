use std::ops::Range;

use kithara_platform::time::Duration;
use kithara_storage::WaitOutcome;
use kithara_stream::{
    ReaderInput, ReaderProfile, SourceError, SourceSeekAnchor, StreamError, StreamResult,
};

use super::HlsVariant;
use crate::HlsError;

pub(crate) struct VariantReaderPreparation {
    anchor: SourceSeekAnchor,
    first_segment: u32,
    forward: Range<u64>,
    input: ReaderInput,
}

impl VariantReaderPreparation {
    pub(crate) const fn anchor(&self) -> SourceSeekAnchor {
        self.anchor
    }
}

impl HlsVariant {
    pub(crate) fn prepare_reader(
        &self,
        profile: ReaderProfile,
        content_time: Duration,
    ) -> StreamResult<VariantReaderPreparation> {
        self.reset_to_full_range();
        let landing = self.descriptor_at_time(content_time).ok_or_else(|| {
            StreamError::Source(
                HlsError::SegmentNotFound(format!(
                    "reader landing not found: variant={} target_ms={}",
                    self.variant,
                    content_time.as_millis()
                ))
                .into(),
            )
        })?;
        let landing_byte = landing.byte_range.start;
        let warmup_start = profile.warmup().max_bytes().map_or(landing_byte, |bytes| {
            landing_byte.saturating_sub(bytes.get())
        });
        let landing_segment = landing.segment_index;
        let forward_segment = self.demand_segment_at_offset(warmup_start).ok_or_else(|| {
            StreamError::Source(
                HlsError::SegmentNotFound(format!(
                    "reader warmup not mapped: variant={} byte={warmup_start}",
                    self.variant
                ))
                .into(),
            )
        })?;

        self.set_prefetch_anchor(warmup_start);
        let tail_start = self.seek_readahead_start_segment(forward_segment);
        let probe_segment = (landing_segment > 0).then_some(0);
        self.set_segment_aware_seek_tail(tail_start);
        if !self.fetch_plan_satisfied(tail_start, probe_segment) {
            self.rebuild_queue(tail_start, probe_segment);
        }

        let anchor = SourceSeekAnchor::builder()
            .byte_offset(landing_byte)
            .segment_start(landing.decode_time)
            .segment_end(landing.decode_time.saturating_add(landing.duration))
            .segment_index(landing.segment_index)
            .variant_index(landing.variant_index)
            .build();

        let forward_end = self.stream_len().map_or_else(
            || landing_byte.saturating_add(profile.read_ahead_bytes().get()),
            |len| {
                landing_byte
                    .saturating_add(profile.read_ahead_bytes().get())
                    .min(len)
            },
        );
        Ok(VariantReaderPreparation {
            anchor,
            first_segment: forward_segment,
            forward: warmup_start..forward_end,
            input: profile.input(),
        })
    }

    /// Last segment an inactive session may fetch while it is being built.
    ///
    /// Derived from the same byte range [`reader_is_ready`](Self::reader_is_ready)
    /// waits on, and derived *live*: segment sizes start as placeholders and are
    /// replaced by the committed lengths, which moves every byte-to-segment
    /// boundary. A ceiling frozen at plan time is a second source of truth for
    /// the same window — once the real sizes push the window's last byte past
    /// the frozen segment, dispatch refuses to fetch the segment readiness is
    /// waiting for and the transition never completes.
    pub(crate) fn construction_segment_end(&self, preparation: &VariantReaderPreparation) -> u32 {
        self.demand_segment_at_offset(preparation.forward.end.saturating_sub(1))
            .unwrap_or(preparation.first_segment)
            .max(preparation.first_segment)
    }

    pub(crate) fn reader_is_ready(
        &self,
        preparation: &VariantReaderPreparation,
    ) -> StreamResult<bool> {
        let header_ready = if matches!(preparation.input, ReaderInput::InitOnly) {
            let header = self.header_byte_range()?;
            !header.is_empty() && self.reader_range_is_ready(header)?
        } else {
            true
        };
        Ok(header_ready && self.reader_range_is_ready(preparation.forward.clone())?)
    }

    fn reader_range_is_ready(&self, range: Range<u64>) -> StreamResult<bool> {
        match self.wait_range(range, Some(Duration::ZERO)) {
            Ok(WaitOutcome::Ready | WaitOutcome::Eof) => Ok(true),
            Ok(WaitOutcome::Interrupted)
            | Err(StreamError::Source(SourceError::WaitBudgetExceeded)) => Ok(false),
            Err(error) => Err(error),
        }
    }
}
