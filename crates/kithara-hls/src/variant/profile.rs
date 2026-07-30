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
    forward: Range<u64>,
    input: ReaderInput,
    warmup: Option<Range<u64>>,
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
        let first_segment = self.demand_segment_at_offset(warmup_start).ok_or_else(|| {
            StreamError::Source(
                HlsError::SegmentNotFound(format!(
                    "reader warmup not mapped: variant={} byte={warmup_start}",
                    self.variant
                ))
                .into(),
            )
        })?;

        self.set_prefetch_anchor(warmup_start);
        if !self.fetch_plan_satisfied(first_segment) {
            self.rebuild_queue(first_segment);
        }

        let forward_end = self.stream_len().map_or_else(
            || landing_byte.saturating_add(profile.read_ahead_bytes().get()),
            |len| {
                landing_byte
                    .saturating_add(profile.read_ahead_bytes().get())
                    .min(len)
            },
        );
        let warmup = (warmup_start < landing_byte).then_some(warmup_start..landing_byte);
        let anchor = SourceSeekAnchor::builder()
            .byte_offset(landing_byte)
            .segment_start(landing.decode_time)
            .segment_end(landing.decode_time.saturating_add(landing.duration))
            .segment_index(landing.segment_index)
            .variant_index(landing.variant_index)
            .build();
        self.set_seek_alias(landing_byte, landing.segment_index);
        self.set_exact_seek_demand(landing_byte, landing.segment_index);

        Ok(VariantReaderPreparation {
            anchor,
            forward: warmup_start..forward_end,
            input: profile.input(),
            warmup,
        })
    }

    pub(crate) fn reader_is_ready(
        &self,
        preparation: &VariantReaderPreparation,
    ) -> StreamResult<bool> {
        if matches!(preparation.input, ReaderInput::InitOnly) {
            let header = self.header_byte_range()?;
            if header.is_empty() || !self.reader_range_is_ready(header)? {
                return Ok(false);
            }
            return preparation
                .warmup
                .as_ref()
                .map_or(Ok(true), |range| self.reader_range_is_ready(range.clone()));
        }
        self.reader_range_is_ready(preparation.forward.clone())
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
