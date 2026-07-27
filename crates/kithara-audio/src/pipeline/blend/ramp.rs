use num_traits::cast::ToPrimitive;

/// A crossfade: how many frames it covers and how much of it the blender has
/// already emitted. Zero frames is the cut — the same path, spent instantly.
pub(super) struct Ramp {
    pub(super) frames: u64,
    pub(super) emitted: u64,
}

impl Ramp {
    pub(super) fn none() -> Self {
        Self {
            frames: 0,
            emitted: 0,
        }
    }

    pub(super) fn over(frames: u64) -> Self {
        Self { frames, emitted: 0 }
    }

    pub(super) fn is_running(&self) -> bool {
        self.remaining() > 0
    }

    pub(super) fn remaining(&self) -> u64 {
        self.frames.saturating_sub(self.emitted)
    }

    /// Incoming gain at the `n`-th frame of the ramp.
    ///
    /// Equal-gain, not equal-power: the two sides are the same music decoded
    /// twice, so they are correlated and their sum is not root-two. An
    /// equal-power pair would peak three decibels high in the middle of every
    /// switch, which is a transient the oracle counts as an onset the source
    /// never had.
    pub(super) fn gain(&self, n: u64) -> f32 {
        let frames = as_f32(self.frames.max(1));
        (as_f32(self.emitted.saturating_add(n).saturating_add(1)) / frames).min(1.0)
    }
}

/// Frame counts are small enough that `f32` holds them exactly; a ramp long
/// enough to lose precision would be minutes of crossfade.
fn as_f32(frames: u64) -> f32 {
    frames.to_f32().unwrap_or(f32::MAX)
}
