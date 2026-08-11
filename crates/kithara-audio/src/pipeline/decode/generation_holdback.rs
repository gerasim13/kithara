use kithara_decode::{PcmChunk, PcmSpec};

use super::{
    DecoderGeneration, Holdback, StageFailure, StageOutput, StageProgress, StageResult,
    chunk_range, stage_failure,
};

impl DecoderGeneration {
    pub(crate) fn prepare_holdback(&mut self, spec: PcmSpec, join_frames: u64) -> StageResult {
        self.holdback = None;
        let Some(slots) = usize::try_from(join_frames)
            .ok()
            .and_then(|frames| frames.checked_add(1))
        else {
            return StageResult::NeedMore;
        };
        if self.staged.capacity() < slots {
            self.staged.reserve_exact(slots - self.staged.len());
        }
        let holdback = Holdback {
            join_frames,
            slots,
            spec,
        };
        if let Some(failure) = self.validate_staged_holdback(holdback) {
            return StageResult::Invalid(failure);
        }
        self.holdback = Some(holdback);
        self.drain_holdback()
    }

    pub(crate) fn push_holdback(&mut self, chunk: PcmChunk) -> StageResult {
        if self.gapless.has_output() {
            return StageResult::Invalid(stage_failure(
                chunk,
                "transition holdback has undrained gapless output",
            ));
        }
        if matches!(self.holdback_progress(), StageProgress::Ready) {
            return StageResult::Invalid(stage_failure(
                chunk,
                "transition holdback is already ready",
            ));
        }
        self.gapless.push(chunk);
        self.drain_holdback()
    }

    pub(crate) fn next_with_holdback(&mut self) -> StageOutput {
        match self.pump_holdback() {
            StageResult::Ready => StageOutput::Output(self.staged.pop_front()),
            StageResult::NeedMore => StageOutput::Output(None),
            StageResult::Invalid(failure) => StageOutput::Invalid(failure),
        }
    }

    fn validate_staged_holdback(&mut self, holdback: Holdback) -> Option<StageFailure> {
        #[cfg(test)]
        self.record_staged_scan();

        let mut staged_end = None;
        let mut invalid = None;
        for (index, chunk) in self.staged.iter().enumerate() {
            let Some((start, end)) = chunk_range(chunk, holdback.spec) else {
                let detail = if chunk.spec() == holdback.spec {
                    "invalid PCM metadata in transition holdback"
                } else {
                    "mixed PCM spec in transition holdback"
                };
                invalid = Some((index, detail));
                break;
            };
            if index >= holdback.slots {
                invalid = Some((index, "transition holdback capacity exceeded"));
                break;
            }
            if staged_end.is_some_and(|expected| expected != start) {
                invalid = Some((index, "discontinuous PCM in transition holdback"));
                break;
            }
            staged_end = Some(end);
        }
        if let Some((index, detail)) = invalid {
            return self
                .staged
                .remove(index)
                .map(|chunk| stage_failure(chunk, detail));
        }
        None
    }

    fn drain_holdback(&mut self) -> StageResult {
        self.pump_holdback()
    }

    fn pump_holdback(&mut self) -> StageResult {
        loop {
            let progress = self.holdback_progress();
            if matches!(progress, StageProgress::Ready) {
                return StageResult::Ready;
            }
            let Some(chunk) = self.gapless.next() else {
                return StageResult::NeedMore;
            };
            if let Some(failure) = self.push_holdback_chunk(chunk) {
                return StageResult::Invalid(failure);
            }
        }
    }

    fn holdback_progress(&self) -> StageProgress {
        let Some(holdback) = self.holdback else {
            return StageProgress::NeedMore;
        };
        let Some(front) = self.staged.front() else {
            return StageProgress::NeedMore;
        };
        let Some((_, frontier)) = chunk_range(front, holdback.spec) else {
            return StageProgress::NeedMore;
        };
        let Some(back) = self.staged.back() else {
            return StageProgress::NeedMore;
        };
        let Some((_, staged_end)) = chunk_range(back, holdback.spec) else {
            return StageProgress::NeedMore;
        };
        let Some(join_end) = frontier.checked_add(holdback.join_frames) else {
            return StageProgress::NeedMore;
        };
        if join_end <= staged_end {
            StageProgress::Ready
        } else {
            StageProgress::NeedMore
        }
    }

    fn push_holdback_chunk(&mut self, chunk: PcmChunk) -> Option<StageFailure> {
        let Some(holdback) = self.holdback else {
            return Some(stage_failure(chunk, "transition holdback was not prepared"));
        };
        let Some((start, _)) = chunk_range(&chunk, holdback.spec) else {
            let detail = if chunk.spec() == holdback.spec {
                "invalid PCM metadata in transition holdback"
            } else {
                "mixed PCM spec in transition holdback"
            };
            return Some(stage_failure(chunk, detail));
        };
        if self.staged.len() >= holdback.slots {
            return Some(stage_failure(
                chunk,
                "transition holdback capacity exceeded",
            ));
        }
        if let Some(back) = self.staged.back() {
            let Some((_, staged_end)) = chunk_range(back, holdback.spec) else {
                return Some(stage_failure(
                    chunk,
                    "invalid retained PCM in transition holdback",
                ));
            };
            if staged_end != start {
                return Some(stage_failure(
                    chunk,
                    "discontinuous PCM in transition holdback",
                ));
            }
        }
        self.staged.push_back(chunk);
        None
    }

    #[cfg(test)]
    pub(super) fn record_staged_scan(&self) {
        self.staged_scan_count
            .set(self.staged_scan_count.get().saturating_add(1));
    }
}
