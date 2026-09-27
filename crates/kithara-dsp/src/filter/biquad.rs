use std::{
    num::NonZeroUsize,
    ops::{Deref, DerefMut, Range},
};

use num_traits::ToPrimitive;

use super::{Coefficients, FilterError};
use crate::backend::platform;

mod consts {
    /// Frames per settle pass; the scratch planes hold this many.
    pub(super) const SETTLE_CHUNK: usize = 64;
    /// `log2` of the residue a transient decays to before it counts as settled.
    pub(super) const SETTLE_FLOOR_LOG2: f64 = -24.0;
    /// Input and output peaks at or below this reset the state.
    pub(super) const SILENT_PEAK: f32 = 1.0e-9;
}

/// Cascaded biquad sections applied in place to channel planes.
///
/// Sections start as identity. The state carries over between calls, so a
/// stream split into any chunks yields the same samples. A call whose input
/// and output both stay at or below `1e-9` resets the state, so silence
/// never decays into denormals.
pub struct Biquad {
    cascade: platform::Cascade,
    decay: Box<[usize]>,
    scratch: Box<[Box<[f32]>]>,
}

impl Biquad {
    /// # Errors
    /// [`FilterError::Shape`] when `channels × sections` overflows,
    /// [`FilterError::Setup`] when the platform refuses the filter.
    pub fn new(channels: NonZeroUsize, sections: NonZeroUsize) -> Result<Self, FilterError> {
        Ok(Self {
            cascade: platform::Cascade::new(channels, sections)?,
            decay: vec![1; sections.get()].into_boxed_slice(),
            scratch: (0..channels.get())
                .map(|_| vec![0.0; consts::SETTLE_CHUNK].into_boxed_slice())
                .collect(),
        })
    }

    /// Replaces the coefficients of one section; the state carries over.
    ///
    /// # Errors
    /// [`FilterError::Shape`] when `section` is out of range.
    pub fn retune(
        &mut self,
        section: usize,
        coefficients: Coefficients,
    ) -> Result<(), FilterError> {
        let slot = self.decay.get_mut(section).ok_or(FilterError::Shape)?;
        self.cascade.set_section(section, coefficients.section())?;
        *slot = decay(coefficients);
        Ok(())
    }

    /// Filters `range` of every plane in place; an empty range is a no-op.
    ///
    /// # Errors
    /// [`FilterError::Shape`] when the plane count differs from the channel
    /// count or a plane does not cover `range`.
    pub fn process<P: DerefMut<Target = [f32]>>(
        &mut self,
        planes: &mut [P],
        range: Range<usize>,
    ) -> Result<(), FilterError> {
        if range.is_empty() {
            return Ok(());
        }
        let input = peak(planes, &range);
        self.cascade.process(planes, range.clone())?;
        if input.max(peak(planes, &range)) <= consts::SILENT_PEAK {
            self.cascade.reset();
        }
        Ok(())
    }

    /// Puts channel `c` in the steady state for the constant input `levels[c]`.
    ///
    /// # Errors
    /// [`FilterError::Shape`] when `levels` has another length than the
    /// channel count.
    pub fn settle(&mut self, levels: &[f32]) -> Result<(), FilterError> {
        if levels.len() != self.scratch.len() {
            return Err(FilterError::Shape);
        }
        self.cascade.reset();
        let frames = self
            .decay
            .iter()
            .fold(0_usize, |sum, frames| sum.saturating_add(*frames));
        for _ in 0..frames.div_ceil(consts::SETTLE_CHUNK).max(1) {
            for (plane, level) in self.scratch.iter_mut().zip(levels) {
                plane.fill(*level);
            }
            self.cascade
                .process(&mut self.scratch, 0..consts::SETTLE_CHUNK)?;
        }
        Ok(())
    }

    /// Takes over the state of `source`.
    ///
    /// # Errors
    /// [`FilterError::Shape`] when channel or section counts differ.
    pub fn copy_state(&mut self, source: &Self) -> Result<(), FilterError> {
        self.cascade.copy_state_from(&source.cascade)
    }
}

fn peak<P: Deref<Target = [f32]>>(planes: &[P], range: &Range<usize>) -> f32 {
    planes
        .iter()
        .filter_map(|plane| plane.get(range.clone()))
        .map(platform::peak)
        .fold(0.0, f32::max)
}

/// Frames until the slowest pole of `coefficients` decays below `2⁻²⁴`.
fn decay(coefficients: Coefficients) -> usize {
    let [_, _, _, a1, a2] = coefficients.section().map(f64::from);
    let discriminant = a1.mul_add(a1, -4.0 * a2);
    let radius = if discriminant < 0.0 {
        a2.sqrt()
    } else {
        (a1.abs() + discriminant.sqrt()) * 0.5
    };
    (consts::SETTLE_FLOOR_LOG2 / radius.log2())
        .ceil()
        .to_usize()
        .map_or(1, |frames| frames.max(1))
}
