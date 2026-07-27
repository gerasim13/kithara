use std::mem;

use kithara_decode::{Decoder, PcmChunk, duration_for_frames};
use kithara_platform::time::Duration;

use crate::pipeline::{
    blend::{ramp::Ramp, staged::Staged},
    decode::core::DecoderSession,
    gapless::GaplessStage,
    seek::skip::frames as frames_for,
};

/// One input to the blender: a decoder generation together with the gapless
/// stage that owns its trim contract and the frames it is holding back.
///
/// The stage belongs to the side, not to the pipeline, because two generations
/// blending across a variant switch are at different points of their own
/// leading/trailing contracts — a shared trimmer would apply one side's state
/// to the other's frames.
pub(crate) struct BlendSide {
    pub(crate) session: DecoderSession,
    pub(crate) gapless: GaplessStage,
    staged: Staged,
    /// What the generation this one replaced was still holding back, fading
    /// out under these first frames. Empty whenever nothing is crossing over,
    /// which is every frame of a track that never switches variant.
    tail: Staged,
    ramp: Ramp,
}

impl BlendSide {
    pub(crate) fn new(session: DecoderSession, gapless: GaplessStage) -> Self {
        Self {
            session,
            gapless,
            staged: Staged::default(),
            tail: Staged::default(),
            ramp: Ramp::none(),
        }
    }

    /// Hand this side over to `session` and start fading out of what the
    /// outgoing generation had in hand. Returns the retired decoder.
    ///
    /// The outgoing decoder is already stopped when this runs — a variant
    /// fence holds it at the boundary and it can produce nothing further — so
    /// the frames it kept back are the only material a crossfade can have, and
    /// keeping them back is the whole reason the blender holds a ramp.
    pub(crate) fn hand_over(&mut self, session: DecoderSession) -> Box<dyn Decoder> {
        self.stage();
        self.tail = mem::take(&mut self.staged);
        // The ramp is exactly what was held, not what was configured: the two
        // have to agree to the frame, because the incoming generation is
        // seeked back by this much so both sides cover the same audio. A ramp
        // longer than the tail would fade into frames nobody kept; a shorter
        // one would strand the rest of the tail.
        self.ramp = Ramp::over(self.tail.frames());
        mem::replace(&mut self.session, session).decoder
    }

    /// How far back the incoming generation has to start for its frames to
    /// line up with the tail it is fading out of.
    pub(crate) fn ramp_length(&self) -> Duration {
        self.tail.front().map_or(Duration::ZERO, |front| {
            duration_for_frames(front.meta.spec.sample_rate.get(), self.ramp.frames)
        })
    }

    /// How many frames this side keeps in hand, read off the decoder that
    /// produced them: whoever built it chose the transition length, and a cut
    /// is a ramp of zero — which holds nothing back and passes frames straight
    /// through.
    pub(super) fn holdback(&self) -> u64 {
        ramp_frames(&self.session)
    }

    /// Move everything the stage has ready into the holdback.
    pub(super) fn stage(&mut self) {
        while let Some(chunk) = self.gapless.next() {
            self.staged.push(chunk);
        }
    }

    /// The next chunk this side is willing to give up: it emits only what it
    /// can spare above the ramp it must keep. Mid-crossfade it spares
    /// everything — the ramp it was keeping is being spent right now.
    pub(super) fn emit(&mut self) -> Option<PcmChunk> {
        self.stage();
        if !self.ramp.is_running() && self.staged.frames_behind_front() < self.holdback() {
            return None;
        }
        self.take()
    }

    /// Give up what is held back — at end of stream there is no next
    /// generation to ramp into, so the ramp is owed to the listener as audio.
    pub(super) fn release(&mut self) -> Option<PcmChunk> {
        self.stage();
        self.take()
    }

    /// The next chunk, faded in over the outgoing generation's tail for as
    /// long as one is still running out.
    pub(super) fn take(&mut self) -> Option<PcmChunk> {
        let mut chunk = self.staged.pop()?;
        if self.ramp.is_running() && !self.tail.is_empty() {
            mix(&mut self.tail, &mut chunk, &mut self.ramp);
        }
        Some(chunk)
    }

    pub(crate) fn drop_staged(&mut self) {
        self.staged.clear();
        self.tail.clear();
        self.ramp = Ramp::none();
    }
}

/// Frames of crossfade the decoder that produced a side asked for. A cut is a
/// ramp of zero length, so it needs no arm of its own here either.
fn ramp_frames(session: &DecoderSession) -> u64 {
    let length = Duration::from(session.decoder.blend());
    frames_for(session.decoder.spec(), length) as u64
}

/// Fade `chunk` in over whatever `tail` still has, frame by frame.
///
/// The tail is consumed as it is spent, so a chunk longer than what is left of
/// the ramp keeps its remaining frames untouched — past the ramp the incoming
/// generation is already the whole signal, and multiplying it by one is the
/// same thing as leaving it alone.
fn mix(tail: &mut Staged, chunk: &mut PcmChunk, ramp: &mut Ramp) {
    align(tail, chunk);
    let channels = usize::from(chunk.meta.spec.channels.max(1));
    let chunk_frames = chunk.frames();
    let mut done = 0usize;
    while done < chunk_frames && ramp.remaining() > 0 {
        let Some(front) = tail
            .front()
            .filter(|front| front.meta.spec == chunk.meta.spec)
        else {
            // Two generations in different formats have no common frame to
            // cross-fade over — an interleaved buffer means something else on
            // each side. There is nothing to ramp between, so there is no ramp.
            break;
        };
        let available = front
            .frames()
            .min(usize::try_from(ramp.remaining()).unwrap_or(usize::MAX));
        let n = available.min(chunk_frames - done);
        if n == 0 {
            break;
        }
        for frame in 0..n {
            let gain_in = ramp.gain(frame as u64);
            let gain_out = 1.0 - gain_in;
            for channel in 0..channels {
                let into = (done + frame) * channels + channel;
                let from = frame * channels + channel;
                let faded = front.samples.get(from).copied().unwrap_or(0.0) * gain_out;
                if let Some(sample) = chunk.samples.get_mut(into) {
                    *sample = sample.mul_add(gain_in, faded);
                }
            }
        }
        ramp.emitted = ramp.emitted.saturating_add(n as u64);
        done += n;
        tail.consume_front(n);
    }
}

/// Bring the incoming side to the tail's first frame before a sample is mixed.
///
/// The incoming generation is asked to land a ramp before the outgoing one
/// stopped, but a demuxer lands on its own packet boundary, not on the frame it
/// was asked for — for AAC that reaches back up to a full 1024-frame packet too
/// far. Those surplus frames are a second copy of audio the outgoing generation
/// already emitted, and letting them out is what shifts the musical timeline.
///
/// Only the incoming side is ever trimmed. Frames the tail has and the incoming
/// side does not are the only copy of that moment, so cutting them here would
/// take audio out of the track to save a crossfade.
fn align(tail: &Staged, chunk: &mut PcmChunk) {
    let Some(front) = tail.front() else { return };
    let want = front.meta.frame_offset;
    let have = chunk.meta.frame_offset;
    if have < want {
        drop_head(chunk, want.saturating_sub(have));
    }
}

/// Drop `frames` from the head of a chunk, keeping its position labels true.
fn drop_head(chunk: &mut PcmChunk, frames: u64) {
    let channels = usize::from(chunk.meta.spec.channels.max(1));
    let drop = usize::try_from(frames)
        .unwrap_or(usize::MAX)
        .min(chunk.frames());
    let samples = drop.saturating_mul(channels);
    let len = chunk.samples.len();
    chunk.samples.copy_within(samples.min(len).., 0);
    chunk.samples.truncate(len.saturating_sub(samples));
    chunk.meta.frame_offset = chunk.meta.frame_offset.saturating_add(drop as u64);
    chunk.meta.frames = chunk
        .meta
        .frames
        .saturating_sub(u32::try_from(drop).unwrap_or(u32::MAX));
    chunk.meta.timestamp = chunk.meta.timestamp.saturating_add(duration_for_frames(
        chunk.meta.spec.sample_rate.get(),
        drop as u64,
    ));
}
