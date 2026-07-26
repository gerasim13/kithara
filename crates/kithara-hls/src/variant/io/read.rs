use std::num::NonZeroUsize;

use kithara_stream::{PendingReason, ReadOutcome, StreamResult};
use kithara_test_utils::kithara;
use tracing::trace;

use super::ReadLease;
use crate::segment::PlannedFetch;

/// Where a lease turns byte offsets into bytes: the alias-aware lookup, and
/// the walk that serves a read out of the init prefix and the media segments
/// behind it.
impl ReadLease {
    pub(super) fn fetch_is_planned(&self, planned: PlannedFetch) -> bool {
        self.queue.lock().contains(&planned)
    }

    /// Reader-facing lookup in **virtual** byte space. A seek alias this lease
    /// planted wins: it stands in for the anchor the reader was handed before
    /// the exact prefix resolved. Otherwise the variant's offset table
    /// answers, gated against `[served_from..served_until)`, so cross-variant
    /// lookups fall through to the variant that still serves the byte.
    pub(crate) fn find_at_offset(&self, byte_offset: u64) -> Option<(u32, u64, u64)> {
        self.seek_alias_at(byte_offset)
            .or_else(|| self.variant.layout_find_at_offset(byte_offset))
    }

    #[kithara::hang_watchdog]
    pub(crate) fn read_at(&self, offset: u64, buf: &mut [u8]) -> StreamResult<ReadOutcome> {
        let total = self.variant.total_bytes();
        if total > 0 && offset >= total && self.eof_ready() {
            return Ok(ReadOutcome::Eof);
        }
        if self.exact_seek_metadata_phase().is_some() || self.exact_byte_metadata_phase().is_some()
        {
            trace!(
                variant = self.variant.index(),
                offset, "read_at: gated by exact-size metadata demand"
            );
            return Ok(Self::wrap(0));
        }

        let buf_len = u64::try_from(buf.len()).unwrap_or(u64::MAX);
        let mut written: usize = 0;
        let mut cursor = offset;
        let read_end = offset.saturating_add(buf_len);

        while let Some(init_range) = self.variant.init_descriptor_at(cursor) {
            hang_tick!();
            if cursor >= init_range.end {
                break;
            }
            let slice_end = read_end.min(init_range.end);
            let local_start = cursor - init_range.start;
            let local_end = slice_end - init_range.start;
            let take = usize::try_from(local_end - local_start).unwrap_or(usize::MAX);
            let dst = &mut buf[written..written + take];
            match self.variant.init_read_at(local_start..local_end, dst)? {
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
        if self.variant.has_init()
            && self.variant.init_size() == 0
            && self.variant.served_from() == 0
            && !self.variant.init_failed()
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
            let Some(n) = self
                .variant
                .segment_read_at(seg_idx, local_start..local_end, dst)?
            else {
                trace!(
                    variant = self.variant.index(),
                    seg_idx,
                    cursor,
                    size_exact = self
                        .variant
                        .segments
                        .get(seg_idx as usize)
                        .is_some_and(|s| s.size().is_exact()),
                    loaded = self
                        .variant
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
                    variant = self.variant.index(),
                    seg_idx, cursor, n, take, "read_at: short segment read"
                );
                break;
            }
        }

        Ok(Self::wrap(written))
    }

    fn wrap(written: usize) -> ReadOutcome {
        NonZeroUsize::new(written).map_or(
            ReadOutcome::Pending(PendingReason::Retry),
            ReadOutcome::Bytes,
        )
    }
}
