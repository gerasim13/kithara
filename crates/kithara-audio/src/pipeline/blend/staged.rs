use std::collections::VecDeque;

use kithara_decode::PcmChunk;

/// Decoded frames retained for emission or handoff.
#[derive(Default)]
pub(super) struct Staged {
    chunks: VecDeque<PcmChunk>,
    frames: u64,
}

impl Staged {
    /// Retain a non-empty chunk.
    pub(super) fn push(&mut self, chunk: PcmChunk) {
        if chunk.frames() == 0 {
            return;
        }
        self.frames = self.frames.saturating_add(u64::from(chunk.meta.frames));
        self.chunks.push_back(chunk);
    }

    pub(super) fn pop(&mut self) -> Option<PcmChunk> {
        let chunk = self.chunks.pop_front()?;
        self.frames = self.frames.saturating_sub(u64::from(chunk.meta.frames));
        Some(chunk)
    }

    pub(super) fn frames(&self) -> u64 {
        self.frames
    }

    pub(super) fn is_empty(&self) -> bool {
        self.chunks.is_empty()
    }

    pub(super) fn front(&self) -> Option<&PcmChunk> {
        self.chunks.front()
    }

    /// Drop `n` frames from the front of the leading chunk, and the chunk
    /// itself once nothing is left of it.
    pub(super) fn consume_front(&mut self, n: usize) {
        let Some(front) = self.chunks.front_mut() else {
            return;
        };
        let channels = usize::from(front.meta.spec.channels.max(1));
        let samples = n.saturating_mul(channels);
        let len = front.samples.len();
        if samples >= len {
            self.pop();
            return;
        }
        front.samples.copy_within(samples.., 0);
        front.samples.truncate(len - samples);
        front.meta.frames = front
            .meta
            .frames
            .saturating_sub(u32::try_from(n).unwrap_or(u32::MAX));
        self.frames = self.frames.saturating_sub(n as u64);
    }

    /// Frames retained behind the front chunk.
    pub(super) fn frames_behind_front(&self) -> u64 {
        self.chunks.front().map_or(0, |front| {
            self.frames.saturating_sub(u64::from(front.meta.frames))
        })
    }

    pub(super) fn clear(&mut self) {
        self.chunks.clear();
        self.frames = 0;
    }
}
