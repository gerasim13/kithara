use std::{num::NonZeroU64, ops::Range};

use kithara_platform::time::Duration;
use kithara_storage::WaitOutcome;
use kithara_stream::{
    ReaderInput, ReaderProfile, SourceError, SourceSeekAnchor, StreamError, StreamResult,
};
use tracing::{debug, trace};

use super::HlsVariant;
use crate::HlsError;

pub(crate) struct VariantReaderPreparation {
    input: ReaderInput,
    anchor: SourceSeekAnchor,
    first_segment: u32,
    landing_segment: u32,
    read_ahead: u64,
    warmup: u64,
}

impl VariantReaderPreparation {
    pub(crate) const fn anchor(&self) -> SourceSeekAnchor {
        self.anchor
    }
}

impl HlsVariant {
    /// Last segment an inactive session may fetch while it is being built.
    ///
    /// Derived from the same window [`reader_is_ready`](Self::reader_is_ready)
    /// waits on, so the two cannot disagree: a ceiling short of the segment
    /// readiness waits for deadlocks the transition.
    pub(crate) fn construction_segment_end(&self, preparation: &VariantReaderPreparation) -> u32 {
        self.demand_segment_at_offset(self.forward_window(preparation).end.saturating_sub(1))
            .unwrap_or(preparation.first_segment)
            .max(preparation.first_segment)
    }

    /// Byte window a session must be able to read before its decoder is built.
    ///
    /// Named by *segment*, resolved to bytes on every call. Segment lengths
    /// start as placeholders and are replaced by the committed ones, which moves
    /// every byte-to-segment boundary — most visibly when the init segment
    /// lands, since its real length re-keys everything after it. A window frozen
    /// in placeholder bytes at plan time stops describing the segments it was
    /// built from: readiness then waits on bytes that now belong elsewhere and
    /// never becomes true, so the reader is never handed over.
    fn forward_window(&self, preparation: &VariantReaderPreparation) -> Range<u64> {
        let landing = self
            .segment_byte_offset(preparation.landing_segment)
            .unwrap_or_default();
        let start = landing.saturating_sub(preparation.warmup);
        let end = landing.saturating_add(preparation.read_ahead);
        let end = self.stream_len().map_or(end, |len| end.min(len));
        start..end.max(start)
    }

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
        // The plan covers one segment behind the landing, whatever the
        // container. A demuxer handed a landing time parks at the packet
        // boundary at or before it, and when the landing is a segment start the
        // first packet it reads begins in the segment before. Exact byte sizes
        // say nothing about where the codec's packet grid falls, so this backoff
        // cannot be conditional on them: without it the decoder is handed a
        // reader that cannot reach its own first packet, produces one chunk, and
        // stalls with the transition still pending.
        // What the plan must not carry is a segment-0 decoder probe. That probe
        // belongs to a reader opened at the head of the stream, which scans
        // forward from byte zero; this one opens on the landing anchor and seeks
        // straight to it, so it never reads segment 0 — and readiness does not
        // wait for it either. Queued at the front it was simply the first thing
        // the downloader served, ahead of the segment readiness *is* waiting
        // for: measured on a same-codec downswitch it took 9.8 ms of an 18.4 ms
        // switch and the landing segment never arrived at all.
        // The backoff still leads the landing. Ordering the landing first buys
        // nothing — readiness is reached on the same fetch either way — while
        // the decoder build that follows reads backwards from the landing, so a
        // backoff deferred by one fetch is a build deferred by one fetch, which
        // on a slow target is the whole switch.
        let tail_start = forward_segment.saturating_sub(1);
        self.set_segment_aware_seek_tail(tail_start);
        if !self.fetch_plan_satisfied(tail_start) {
            self.rebuild_queue(tail_start, None);
        }

        let anchor = SourceSeekAnchor::builder()
            .byte_offset(landing_byte)
            .segment_start(landing.decode_time)
            .segment_end(landing.decode_time.saturating_add(landing.duration))
            .segment_index(landing.segment_index)
            .variant_index(landing.variant_index)
            .build();

        debug!(
            variant = self.variant,
            content_time_ms = content_time.as_millis(),
            segment_idx = landing_segment,
            bytes = landing_byte,
            "variant reader landed"
        );
        Ok(VariantReaderPreparation {
            anchor,
            landing_segment,
            first_segment: forward_segment,
            read_ahead: profile.read_ahead_bytes().get(),
            warmup: profile.warmup().max_bytes().map_or(0, NonZeroU64::get),
            input: profile.input(),
        })
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
        let forward = self.forward_window(preparation);
        let forward_ready = self.reader_range_is_ready(forward.clone())?;
        // Polled every transition pass, so `trace!`: the pair of flags plus the
        // window they are asked about is the difference between "the header
        // never arrived" and "the window is not covered yet", which no other
        trace!(
            variant = self.variant,
            header_ready,
            forward_ready,
            forward_start = forward.start,
            forward_end = forward.end,
            ceiling = self.construction_segment_end(preparation),
            init_loaded = self.init().is_some_and(|i| i.state().is_loaded()),
            init_downloading = self.init().is_some_and(|i| i.state().is_downloading()),
            init_slow = self.init().is_some_and(|i| i.state().is_slow()),
            init_failed = self.init().is_some_and(|i| i.state().is_failed()),
            // A window this session waits on but has nothing queued for is the
            // shape of a stall: nothing is in flight, nothing is planned, and
            // the only thing that re-drives the peer is another readiness poll
            // the consumer cannot make while it waits for this one.
            queued = self.flow.queue.lock().len(),
            "variant reader readiness"
        );
        Ok(header_ready && forward_ready)
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
