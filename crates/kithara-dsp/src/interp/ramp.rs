use std::num::NonZeroU32;

use num_traits::ToPrimitive;

/// Per-frame read rates moving linearly from `from` to `to` over `frames`
/// frames, then holding `to`: frame `j` advances the cursor by
/// `from + j·(to − from)/frames` while `j < frames` and by `to` after. The
/// cursor offset after `k` frames has the closed form
/// `s·from + step·s(s − 1)/2 + (k − s)·to` with `s = min(k, frames)`, so a
/// block of positions needs no running sum and the ramp lands on `to`
/// exactly.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RateRamp {
    from: f64,
    to: f64,
    frames: u32,
}

impl RateRamp {
    /// A ramp from `from` to `to` over `frames` frames.
    #[must_use]
    pub const fn new(from: f64, to: f64, frames: NonZeroU32) -> Self {
        Self {
            from,
            to,
            frames: frames.get(),
        }
    }

    /// A constant rate.
    #[must_use]
    pub const fn hold(rate: f64) -> Self {
        Self {
            from: rate,
            to: rate,
            frames: 0,
        }
    }

    /// Rate of the next frame.
    #[must_use]
    pub const fn current(self) -> f64 {
        self.from
    }

    /// Rate the ramp ends on.
    #[must_use]
    pub const fn target(self) -> f64 {
        self.to
    }

    /// The constant rate once the ramp has finished.
    #[must_use]
    pub fn held(self) -> Option<f64> {
        (self.frames == 0).then_some(self.to)
    }

    /// Cursor advance over the next `frames` frames. Runs once per position,
    /// so it uses `*` and `+`: a scalar `mul_add` on a target without FMA is
    /// a libm call.
    #[must_use]
    pub fn offset(self, frames: usize) -> f64 {
        let frames = frames.to_f64().unwrap_or(f64::INFINITY);
        let steps = frames.min(f64::from(self.frames));
        let ramp = steps * self.from + self.step() * steps * (steps - 1.0) * 0.5;
        (frames - steps) * self.to + ramp
    }

    /// The rest of the ramp after `frames` frames.
    #[must_use]
    pub fn after(self, frames: usize) -> Self {
        let done = u32::try_from(frames).unwrap_or(u32::MAX);
        self.frames
            .checked_sub(done)
            .and_then(NonZeroU32::new)
            .map_or_else(
                || Self::hold(self.to),
                |left| {
                    Self::new(
                        f64::from(done).mul_add(self.step(), self.from),
                        self.to,
                        left,
                    )
                },
            )
    }

    /// Fastest rate among the next `frames` frames, and never below
    /// [`current`](Self::current): a linear ramp peaks at an end.
    #[must_use]
    pub fn peak(self, frames: usize) -> f64 {
        self.from
            .max(self.after(frames.saturating_sub(1)).current())
    }

    /// Writes `start + offset(k)` as `f32` into `output[k]` while it stays
    /// below `end`; returns how many positions it wrote.
    pub fn positions(self, start: f64, end: f64, output: &mut [f32]) -> usize {
        for (frame, slot) in output.iter_mut().enumerate() {
            let Some(position) = Some(start + self.offset(frame))
                .filter(|position| *position < end)
                .and_then(|position| position.to_f32())
            else {
                return frame;
            };
            *slot = position;
        }
        output.len()
    }

    fn step(self) -> f64 {
        NonZeroU32::new(self.frames).map_or(0.0, |frames| {
            (self.to - self.from) / f64::from(frames.get())
        })
    }
}
