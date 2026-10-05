use std::{num::NonZeroU32, ops::Range};

use kithara_dsp::param::{DEFAULT_GAIN_SPAN, SmoothedParam, SmootherConfig};

/// A track's transport: the ramp that lets its share into the mix on start and takes it out on
/// stop, so neither steps the waveform.
pub(super) struct TrackGate {
    level: SmoothedParam,
}

impl TrackGate {
    const OPEN: f32 = 1.0;
    const SHUT: f32 = 0.0;

    pub(super) fn new(open: bool, declick: SmootherConfig, sample_rate: NonZeroU32) -> Self {
        Self {
            level: SmoothedParam::new(Self::level(open), DEFAULT_GAIN_SPAN, declick, sample_rate),
        }
    }

    const fn level(open: bool) -> f32 {
        if open { Self::OPEN } else { Self::SHUT }
    }

    /// Ramps the gate open or shut from the next frame it passes.
    pub(super) fn steer(&mut self, open: bool) {
        self.level.set_value(Self::level(open));
    }

    delegate::delegate! {
        to self.level {
            /// Moves the gate to where it is steered at once.
            #[call(reset_to_target)]
            pub(super) fn snap(&mut self);
            pub(super) fn update_sample_rate(&mut self, sample_rate: NonZeroU32);
        }
    }

    /// Whether the gate has shut: nothing passes it, so its track need not be read.
    pub(super) fn is_shut(&self) -> bool {
        self.level.has_settled_at(Self::SHUT)
    }

    /// Scales the frames of `range` in a stereo pair by the gate.
    pub(super) fn apply(&mut self, bufs: &mut [&mut [f32]], range: Range<usize>) {
        if self.level.has_settled_at(Self::OPEN) {
            return;
        }
        let [left, right, ..] = bufs else {
            return;
        };
        for (l, r) in left[range.clone()].iter_mut().zip(right[range].iter_mut()) {
            let gain = self.level.next_smoothed();
            *l *= gain;
            *r *= gain;
        }
        self.level.settle();
    }
}
