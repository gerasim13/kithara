use std::{num::NonZeroUsize, ops::Range};

use kithara_platform::time::Duration;
use kithara_storage::WaitOutcome;
use kithara_stream::{PendingReason, ReadOutcome, StreamError, StreamResult};
use kithara_test_utils::kithara;
use tracing::trace;

use super::{HlsVariant, read::RangeGate};
use crate::{HlsError, segment::PlannedFetch};

impl HlsVariant {
    #[kithara::hang_watchdog]
    pub(crate) fn read_at(&self, offset: u64, buf: &mut [u8]) -> StreamResult<ReadOutcome> {
        let uses_seek_alias = self.seek_alias_at(offset).is_some();
        if !uses_seek_alias && self.eof_at(offset) {
            return Ok(ReadOutcome::Eof);
        }
        if self.exact_seek_metadata_phase().is_some() || self.exact_byte_metadata_phase().is_some()
        {
            trace!(
                variant = self.variant,
                offset, "read_at: gated by exact-size metadata demand"
            );
            return Ok(Self::wrap(0));
        }

        let buf_len = u64::try_from(buf.len()).unwrap_or(u64::MAX);
        let mut written: usize = 0;
        let mut cursor = offset;
        let read_end = offset.saturating_add(buf_len);

        while let Some(init_range) = self.init_descriptor_at(cursor) {
            hang_tick!();
            if cursor >= init_range.end {
                break;
            }
            let slice_end = read_end.min(init_range.end);
            let local_start = cursor - init_range.start;
            let local_end = slice_end - init_range.start;
            let take = usize::try_from(local_end - local_start).unwrap_or(usize::MAX);
            let dst = &mut buf[written..written + take];
            match self.init_read_at(local_start..local_end, dst)? {
                Some(n) => {
                    written += n;
                    cursor += n as u64;
                    if n < take {
                        return Ok(Self::wrap(written));
                    }
                    if cursor >= read_end {
                        return Ok(Self::wrap(written));
                    }
                }
                None => return Ok(Self::wrap(written)),
            }
        }

        // An `#EXT-X-MAP` init occupies the virtual prefix `[0, init_size)`.
        // While the init is declared (`has_init`) but not yet sized
        // (`init_size() == 0` — before lazy probe or body commit resolves it),
        // the offset table transiently seeds segment 0 at
        // offset 0. Serving media here would hand the demuxer segment 0's
        // container where the init's `ftyp`/`moov` belongs
        // ("re_mp4: ftyp not found"), or wedge the reader. Hold the read
        // pending: `needs_init_fetch` keeps the init enqueued and its commit
        // sizes the prefix, after which `init_descriptor_at` routes offset 0
        // to the init. Only the fresh-activation frame (`served_from() == 0`)
        // places the init at offset 0; a switched-in variant's init is
        // orphaned in natural space (see `init_descriptor_at`), so its reads
        // continue past offset 0 and must not be gated here. A terminally
        // failed init (`init_failed`) stops reserving the prefix so the read
        // surfaces an error instead of waiting forever.
        if self.has_init()
            && self.init_size() == 0
            && self.served_from() == 0
            && !self.init_failed()
        {
            return Ok(Self::wrap(written));
        }

        while cursor < read_end {
            hang_tick!();
            let Some((seg_idx, seg_off, seg_size)) = self.find_at_offset(cursor) else {
                break;
            };
            let seg_end = seg_off + seg_size;
            let slice_end = read_end.min(seg_end);
            let local_start = cursor - seg_off;
            let local_end = slice_end - seg_off;
            let take = usize::try_from(local_end - local_start).unwrap_or(usize::MAX);
            let dst = &mut buf[written..written + take];
            let Some(n) = self.segment_read_at(seg_idx, local_start..local_end, dst)? else {
                trace!(
                    variant = self.variant,
                    seg_idx,
                    cursor,
                    size_exact = self
                        .segments
                        .get(seg_idx as usize)
                        .is_some_and(|s| s.size().is_exact()),
                    loaded = self
                        .segments
                        .get(seg_idx as usize)
                        .is_some_and(|s| s.state().is_loaded()),
                    "read_at: segment bytes unavailable"
                );
                break;
            };
            written += n;
            cursor += n as u64;
            if n < take {
                trace!(
                    variant = self.variant,
                    seg_idx, cursor, n, take, "read_at: short segment read"
                );
                break;
            }
        }

        Ok(Self::wrap(written))
    }

    pub(super) fn segment_has_demand(&self, seg_idx: u32) -> bool {
        self.segment_downloading(seg_idx) || self.fetch_is_planned(PlannedFetch::Segment(seg_idx))
    }

    #[kithara::hang_watchdog]
    pub(crate) fn wait_range(
        &self,
        range: Range<u64>,
        _timeout: Option<Duration>,
    ) -> StreamResult<WaitOutcome> {
        let stable_pending = match self.range_gate(&range) {
            Some(RangeGate::Eof) => return Ok(WaitOutcome::Eof),
            Some(RangeGate::Ready) => {
                hang_reset!();
                return Ok(WaitOutcome::Ready);
            }
            Some(RangeGate::Metadata(_) | RangeGate::Pending) => true,
            None => false,
        };
        if self.flow.reader.is_flushing() {
            return Ok(WaitOutcome::Interrupted);
        }
        if stable_pending && self.range_has_failed(&range) {
            return Err(StreamError::Source(HlsError::SegmentUnavailable.into()));
        }
        // The reader driver wakes the peer for this range. An unstable layout
        // publication remains pending and is retried on the next probe.
        trace!(
            variant = self.variant,
            start = range.start,
            end = range.end,
            "wait_range: range not ready (budget exceeded)"
        );
        Err(StreamError::Source(HlsError::WaitBudgetExceeded.into()))
    }

    fn wrap(written: usize) -> ReadOutcome {
        NonZeroUsize::new(written).map_or(
            ReadOutcome::Pending(PendingReason::Retry),
            ReadOutcome::Bytes,
        )
    }
}
