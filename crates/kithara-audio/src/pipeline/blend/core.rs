use std::mem;

use kithara_decode::{BlenderProfile, PcmChunk};

struct Consts;

impl Consts {
    const JOIN_MICROS: u32 = 20_000;
}

enum JoinState {
    Steady,
    /// Ramp counters are `u16` so the per-frame gain is an exact
    /// [`f32::from`] conversion: at 20 ms the ramp is under 4 000 frames even
    /// at 192 kHz, and the saturating conversion pins the pathological case.
    Active {
        frame: u16,
        frames: u16,
    },
}

pub(crate) struct PcmBlender {
    active: BlenderProfile,
    join: JoinState,
    tail: Vec<f32>,
}

impl PcmBlender {
    pub(crate) fn new(active: BlenderProfile) -> Self {
        Self {
            active,
            join: JoinState::Steady,
            tail: Vec::new(),
        }
    }

    /// Frames of outgoing audio a join needs to cover its whole ramp.
    pub(crate) fn join_frames(&self) -> usize {
        usize::from(ramp_frames(self.active))
    }

    pub(crate) fn take_tail_buffer(&mut self) -> Vec<f32> {
        self.join = JoinState::Steady;
        let mut tail = mem::take(&mut self.tail);
        tail.clear();
        tail
    }

    pub(crate) fn replace_active(&mut self, active: BlenderProfile) {
        self.active = active;
        self.join = JoinState::Steady;
        self.tail.clear();
    }

    /// Arms a crossfade from real outgoing PCM into the incoming generation.
    /// A short tail shortens the ramp; an empty tail hard-replaces the profile.
    pub(crate) fn join_active(&mut self, active: BlenderProfile, mut outgoing_tail: Vec<f32>) {
        let channels = usize::from(self.active.spec().channels.max(1));
        let tail_frames = outgoing_tail.len() / channels;
        let frames = u16::try_from(tail_frames.min(self.join_frames())).unwrap_or(u16::MAX);
        outgoing_tail.truncate(usize::from(frames) * channels);
        self.tail = outgoing_tail;
        if self.active.spec() != active.spec() || tail_frames == 0 {
            self.replace_active(active);
            return;
        }
        self.active = active;
        self.join = JoinState::Active { frame: 0, frames };
    }

    pub(crate) fn process_active(&mut self, mut chunk: PcmChunk) -> PcmChunk {
        debug_assert_eq!(chunk.spec(), self.active.spec());
        self.apply_join(&mut chunk);
        chunk
    }

    fn apply_join(&mut self, chunk: &mut PcmChunk) {
        let JoinState::Active { frame, frames } = &mut self.join else {
            return;
        };
        let channels = usize::from(chunk.spec().channels.max(1));
        let tail = self.tail.chunks_exact(channels).skip(usize::from(*frame));
        for (samples, outgoing) in chunk.samples.chunks_exact_mut(channels).zip(tail) {
            let incoming_gain = f32::from(*frame) / f32::from(*frames);
            let outgoing_gain = 1.0 - incoming_gain;
            for (sample, outgoing) in samples.iter_mut().zip(outgoing) {
                *sample = outgoing.mul_add(outgoing_gain, *sample * incoming_gain);
            }
            *frame = frame.saturating_add(1);
        }
        if *frame >= *frames {
            self.join = JoinState::Steady;
            self.tail.clear();
        }
    }
}

fn ramp_frames(profile: BlenderProfile) -> u16 {
    u16::try_from(
        profile
            .spec()
            .sample_rate
            .get()
            .saturating_mul(Consts::JOIN_MICROS)
            .div_ceil(1_000_000),
    )
    .unwrap_or(u16::MAX)
    .max(2)
}
