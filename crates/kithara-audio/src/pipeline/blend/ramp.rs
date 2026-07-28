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

    /// Equal-gain input coefficient for one frame of correlated audio.
    pub(super) fn gain(&self, n: u64) -> f32 {
        let frames = as_f32(self.frames.max(1));
        (as_f32(self.emitted.saturating_add(n).saturating_add(1)) / frames).min(1.0)
    }
}

/// Convert practical crossfade frame counts without precision loss.
fn as_f32(frames: u64) -> f32 {
    frames.to_f32().unwrap_or(f32::MAX)
}
