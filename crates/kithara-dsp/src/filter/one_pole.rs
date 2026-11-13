use std::ops::{DerefMut, Range};

use super::FilterError;
use crate::backend;

/// Fixed-coefficient low-pass: `y[n] = (1 - a)*x[n] + a*y[n - 1]`.
#[derive(Debug)]
pub struct OnePoleLowPass<const CHANNELS: usize> {
    coefficient: f32,
    state: [f32; CHANNELS],
}

impl<const CHANNELS: usize> OnePoleLowPass<CHANNELS> {
    /// Starts every channel at silence, with the feedback coefficient `a`.
    ///
    /// # Errors
    /// [`FilterError::Parameters`] unless `coefficient` is finite and in
    /// `0..1`; [`FilterError::Shape`] when `CHANNELS` is zero.
    pub fn new(coefficient: f32) -> Result<Self, FilterError> {
        if CHANNELS == 0 {
            return Err(FilterError::Shape);
        }
        if !(0.0..1.0).contains(&coefficient) {
            return Err(FilterError::Parameters);
        }
        Ok(Self {
            coefficient,
            state: [0.0; CHANNELS],
        })
    }

    /// Filters `range` in place, without allocating or reassociating audio time.
    ///
    /// # Errors
    /// [`FilterError::Shape`] when channel count or plane coverage differs;
    /// all samples and channel histories remain untouched on refusal.
    pub fn process<P: DerefMut<Target = [f32]>>(
        &mut self,
        planes: &mut [P],
        range: Range<usize>,
    ) -> Result<(), FilterError> {
        if planes.len() != CHANNELS
            || planes
                .iter()
                .any(|plane| plane.get(range.clone()).is_none())
        {
            return Err(FilterError::Shape);
        }
        let mut planes = planes.iter_mut();
        let mut ranges: [Option<&mut [f32]>; CHANNELS] =
            std::array::from_fn(|_| planes.next().and_then(|plane| plane.get_mut(range.clone())));
        if ranges.iter().any(Option::is_none) {
            return Err(FilterError::Shape);
        }
        backend::one_pole_low_pass(self.coefficient, &mut self.state, &mut ranges, range.len());
        Ok(())
    }

    /// Clears the history of every channel, preserving the coefficient.
    pub fn reset(&mut self) {
        self.state.fill(0.0);
    }
}
