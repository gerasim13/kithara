use std::ops::Range;

use kithara_stream::{SourcePhase, needs_exact_byte_sizes};

use super::HlsVariant;
use crate::segment::PlannedFetch;

pub(super) enum RangeGate {
    Eof,
    Metadata(SourcePhase),
    Pending,
    Ready,
}

impl HlsVariant {
    pub(super) fn fetch_is_planned(&self, planned: PlannedFetch) -> bool {
        self.flow.queue.lock().contains(&planned)
    }

    fn init_has_demand(&self) -> bool {
        self.init_downloading() || self.fetch_is_planned(PlannedFetch::Init)
    }

    pub(crate) fn phase_at(&self, range: Range<u64>) -> SourcePhase {
        self.phase_at_with(range, || {})
    }

    fn phase_at_with(&self, range: Range<u64>, after_eof: impl FnOnce()) -> SourcePhase {
        match self.range_gate_with(&range, after_eof) {
            Some(RangeGate::Eof) => SourcePhase::Eof,
            Some(RangeGate::Metadata(phase)) => phase,
            Some(RangeGate::Ready) => SourcePhase::Ready,
            Some(RangeGate::Pending) => {
                if self.flow.reader.is_flushing() {
                    SourcePhase::Seeking
                } else {
                    self.range_wait_phase(&range)
                }
            }
            None => {
                if self.flow.reader.is_flushing() {
                    SourcePhase::Seeking
                } else {
                    SourcePhase::WaitingDemand
                }
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn phase_at_after_eof(
        &self,
        range: Range<u64>,
        after_eof: impl FnOnce(),
    ) -> SourcePhase {
        self.phase_at_with(range, after_eof)
    }

    pub(super) fn range_gate(&self, range: &Range<u64>) -> Option<RangeGate> {
        self.range_gate_with(range, || {})
    }

    fn range_gate_with(&self, range: &Range<u64>, after_eof: impl FnOnce()) -> Option<RangeGate> {
        self.layout.try_published(|| {
            let uses_seek_alias = self.seek_alias_at(range.start).is_some();
            let total = self.total_bytes();
            let eof = !uses_seek_alias
                && self.eof_at_published(range.start, total)
                && !self.flow.reader.is_flushing();
            after_eof();
            if eof {
                return Some(RangeGate::Eof);
            }
            if let Some(phase) = self.exact_seek_metadata_phase() {
                return Some(RangeGate::Metadata(phase));
            }
            if let Some(phase) = self.exact_byte_metadata_phase() {
                return Some(RangeGate::Metadata(phase));
            }
            Some(if self.range_ready_published_with(range, || {}) {
                RangeGate::Ready
            } else {
                RangeGate::Pending
            })
        })
    }

    /// Whether any init/media segment covering `range` settled terminally
    /// (`Failed`): the downloader exhausted its retry budget, so the range
    /// will never load. [`wait_range`](Self::wait_range) consults this when
    /// a range is not ready to tell "still downloading" (spin) from
    /// "permanently failed" (terminal error). Walks the same descriptors as
    /// `range_ready_published_with`, checking slot state rather than
    /// on-disk bytes; the per-byte `contains_range` walk stays out so this
    /// only fires on a real terminal settle, never on a transient gap.
    pub(super) fn range_has_failed(&self, range: &Range<u64>) -> bool {
        let total = self.total_bytes();
        let uses_seek_alias = self.seek_alias_at(range.start).is_some();
        let end = if !uses_seek_alias && total > 0 {
            range.end.min(total)
        } else {
            range.end
        };
        let mut cursor = range.start;
        // The init prefix is not a media segment, so `find_at_offset` returns
        // `None` for a byte inside it — skip past it (jumping to media space)
        // after checking the init's own terminal state, exactly as
        // `range_ready` walks init then media.
        if let Some(init_range) = self.init_descriptor_at(cursor) {
            if self.init_failed() {
                return true;
            }
            cursor = init_range.end;
        }
        while cursor < end {
            let Some((seg_idx, seg_off, seg_size)) = self.find_at_offset(cursor) else {
                break;
            };
            if self.segment_failed(seg_idx) {
                return true;
            }
            cursor = (seg_off + seg_size).max(cursor + 1);
        }
        false
    }

    fn range_ready_published_with(&self, range: &Range<u64>, after_total: impl FnOnce()) -> bool {
        let total = self.total_bytes();
        after_total();
        let uses_seek_alias = self.seek_alias_at(range.start).is_some();
        let clamp_alias_to_eof = uses_seek_alias
            && !needs_exact_byte_sizes(self.profile.codec, self.profile.container)
            && self.eof_ready();
        // When a served segment's size is still unknown, `total` is a lower
        // bound, not the stream end. An offset at/past it is NOT "ready"
        // (clamping `end` to the under-count would falsely report a zero-width
        // ready range and let the reader spin past a real, not-yet-sized
        // segment) — treat it as not-ready so the gate holds Waiting.
        if !uses_seek_alias && total > 0 && range.start >= total && !self.sizes_complete() {
            return false;
        }
        let end = if total > 0 && (!uses_seek_alias || clamp_alias_to_eof) {
            range.end.min(total)
        } else {
            range.end
        };
        if range.start >= end {
            return true;
        }

        let mut cursor = range.start;
        while let Some(init_range) = self.init_descriptor_at(cursor) {
            if cursor >= init_range.end {
                break;
            }
            let slice_end = end.min(init_range.end);
            let local_start = cursor - init_range.start;
            let local_end = slice_end - init_range.start;
            if !self.init_contains(local_start..local_end) {
                return false;
            }
            cursor = slice_end;
            if cursor >= end {
                return true;
            }
        }
        if cursor >= end {
            return true;
        }

        while cursor < end {
            let Some((seg_idx, seg_off, seg_size)) = self.find_at_offset(cursor) else {
                return false;
            };
            let seg_end = seg_off + seg_size;
            let slice_end = end.min(seg_end);
            let local_start = cursor - seg_off;
            let local_end = slice_end - seg_off;
            if !self.segment_contains(seg_idx, local_start..local_end) {
                return false;
            }
            cursor = slice_end;
        }
        cursor >= end
    }

    #[cfg(test)]
    fn range_ready_with(&self, range: &Range<u64>, after_total: impl FnOnce()) -> bool {
        self.layout
            .try_published(|| Some(self.range_ready_published_with(range, after_total)))
            .unwrap_or(false)
    }

    #[cfg(test)]
    pub(crate) fn range_ready(&self, range: &Range<u64>) -> bool {
        self.range_ready_with(range, || {})
    }

    #[cfg(test)]
    pub(crate) fn range_ready_after_total(
        &self,
        range: &Range<u64>,
        after_total: impl FnOnce(),
    ) -> bool {
        self.range_ready_with(range, after_total)
    }

    fn range_wait_phase(&self, range: &Range<u64>) -> SourcePhase {
        self.layout
            .try_published(|| Some(self.range_wait_phase_published(range)))
            .unwrap_or(SourcePhase::WaitingDemand)
    }

    fn range_wait_phase_published(&self, range: &Range<u64>) -> SourcePhase {
        let total = self.total_bytes();
        let uses_seek_alias = self.seek_alias_at(range.start).is_some();
        let clamp_alias_to_eof = uses_seek_alias
            && !needs_exact_byte_sizes(self.profile.codec, self.profile.container)
            && self.eof_ready();
        if !uses_seek_alias && total > 0 && range.start >= total && !self.sizes_complete() {
            let head = self.download_head();
            return if self.segment_has_demand(head) {
                SourcePhase::WaitingDemand
            } else {
                SourcePhase::Waiting
            };
        }

        let end = if total > 0 && (!uses_seek_alias || clamp_alias_to_eof) {
            range.end.min(total)
        } else {
            range.end
        };
        if range.start >= end {
            return SourcePhase::Waiting;
        }
        let mut waiting_on_demand = false;
        let mut cursor = range.start;
        while let Some(init_range) = self.init_descriptor_at(cursor) {
            if cursor >= init_range.end {
                break;
            }
            let slice_end = end.min(init_range.end);
            let local_start = cursor - init_range.start;
            let local_end = slice_end - init_range.start;
            if !self.init_contains(local_start..local_end) {
                if !self.init_has_demand() {
                    return SourcePhase::Waiting;
                }
                waiting_on_demand = true;
            }
            cursor = slice_end;
            if cursor >= end {
                return if waiting_on_demand {
                    SourcePhase::WaitingDemand
                } else {
                    SourcePhase::Waiting
                };
            }
        }

        while cursor < end {
            let Some((seg_idx, seg_off, seg_size)) = self.find_at_offset(cursor) else {
                return SourcePhase::Waiting;
            };
            let seg_end = seg_off + seg_size;
            let slice_end = end.min(seg_end);
            let local_start = cursor - seg_off;
            let local_end = slice_end - seg_off;
            if !self.segment_contains(seg_idx, local_start..local_end) {
                if !self.segment_has_demand(seg_idx) {
                    return SourcePhase::Waiting;
                }
                waiting_on_demand = true;
            }
            cursor = slice_end;
        }

        if waiting_on_demand {
            SourcePhase::WaitingDemand
        } else {
            SourcePhase::Waiting
        }
    }
}
