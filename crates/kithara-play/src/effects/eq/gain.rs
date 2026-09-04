use std::{num::NonZeroU32, ops::Index};

use firewheel_core::dsp::filter::smoothing_filter::{SmoothingFilter, SmoothingFilterCoeff};
use num_traits::cast::AsPrimitive;

use super::GainDb;

struct Consts;

impl Consts {
    const SETTLE_EPSILON: f32 = 0.0001;
    const SMOOTH_BLOCK_SIZE: usize = 32;
    const SMOOTH_SECONDS: f32 = 0.01;
}

#[derive(fieldwork::Fieldwork)]
#[fieldwork(opt_in, get)]
struct SmoothedGain {
    #[field(get(copy), vis = "pub(crate)")]
    target: GainDb,
    filter: SmoothingFilter,
    target_linear: f32,
}

impl SmoothedGain {
    fn new(target: GainDb) -> Self {
        let linear = target.linear();
        Self {
            target,
            filter: SmoothingFilter::new(linear),
            target_linear: linear,
        }
    }

    fn current(&self) -> f32 {
        self.filter.z1
    }

    #[cfg(test)]
    fn is_smoothing(&self) -> bool {
        !self.filter.has_settled(self.target_linear)
    }

    fn set_target(&mut self, target: GainDb) {
        if target == self.target {
            return;
        }
        self.target = target;
        self.target_linear = target.linear();
    }

    /// Jump to the target without the ramp.
    #[cfg(test)]
    fn settle(&mut self) {
        self.filter = SmoothingFilter::new(self.target_linear);
    }

    /// Whether this gain is aimed at `target` and has arrived. The fast paths
    /// need both: a band on its way to unity still has to run the filters.
    fn settled_at(&self, target: GainDb) -> bool {
        self.target == target && self.filter.has_settled(self.target_linear)
    }

    #[inline]
    fn smooth(&mut self, coeff: SmoothingFilterCoeff) {
        self.filter.process(self.target_linear, coeff);
        self.filter
            .settle(self.target_linear, Consts::SETTLE_EPSILON);
    }
}

#[derive(fieldwork::Fieldwork)]
#[fieldwork(opt_in, get)]
pub(crate) struct GainBank {
    coeff: SmoothingFilterCoeff,
    gains: Vec<SmoothedGain>,
    #[field(get, vis = "pub(crate)")]
    bypass_active: bool,
    #[field(get, vis = "pub(crate)")]
    silence_active: bool,
    block_counter: usize,
}

impl GainBank {
    pub(crate) fn new(gains_db: impl Iterator<Item = GainDb>, sample_rate: f32) -> Self {
        let gains = gains_db.map(SmoothedGain::new).collect();
        let mut bank = Self {
            gains,
            coeff: smoothing_coeff(sample_rate),
            block_counter: 0,
            bypass_active: false,
            silence_active: false,
        };
        bank.refresh_fastpath();
        bank
    }

    fn all_settled_at(&self, target: GainDb) -> bool {
        !self.gains.is_empty() && self.gains.iter().all(|gain| gain.settled_at(target))
    }

    fn refresh_fastpath(&mut self) {
        self.bypass_active = self.all_settled_at(GainDb::default());
        self.silence_active = self.all_settled_at(GainDb::MIN);
    }

    pub(crate) fn reset(&mut self) {
        for gain in &mut self.gains {
            *gain = SmoothedGain::new(GainDb::default());
        }
        self.block_counter = 0;
        self.refresh_fastpath();
    }

    pub(crate) fn set(&mut self, band: usize, gain_db: GainDb) {
        if let Some(gain) = self.gains.get_mut(band) {
            gain.set_target(gain_db);
        }
        self.refresh_fastpath();
    }

    #[cfg(test)]
    pub(crate) fn settle(&mut self, band: usize) {
        if let Some(gain) = self.gains.get_mut(band) {
            gain.settle();
        }
        self.refresh_fastpath();
    }

    pub(crate) fn tick(&mut self) {
        self.block_counter += 1;
        if self.block_counter < Consts::SMOOTH_BLOCK_SIZE {
            return;
        }
        self.block_counter = 0;
        for gain in &mut self.gains {
            gain.smooth(self.coeff);
        }
        self.refresh_fastpath();
    }

    pub(crate) fn update_sample_rate(&mut self, sample_rate: f32) {
        self.coeff = smoothing_coeff(sample_rate);
    }

    delegate::delegate! {
        to self.gains {
            #[cfg(test)]
            #[expr($.any(SmoothedGain::is_smoothing))]
            #[call(iter)]
            pub(crate) fn is_smoothing(&self) -> bool;
            pub(crate) const fn len(&self) -> usize;
            #[expr($.current())]
            #[call(index)]
            pub(crate) fn linear(&self, band: usize) -> f32;
            #[expr($.map(SmoothedGain::target))]
            #[call(get)]
            pub(crate) fn target(&self, band: usize) -> Option<GainDb>;
        }
    }
}

/// Coefficients for a smoother that steps once per [`Consts::SMOOTH_BLOCK_SIZE`]
/// samples. What shapes the curve is how many steps fit in the smoothing
/// window, so dividing the window by the block size gives the same curve as
/// dividing the rate by it - and it leaves the rate an exact integer.
fn smoothing_coeff(sample_rate: f32) -> SmoothingFilterCoeff {
    let block_size: f32 = Consts::SMOOTH_BLOCK_SIZE.as_();
    let rate: u32 = sample_rate.max(1.0).as_();
    let rate = NonZeroU32::new(rate).unwrap_or(NonZeroU32::MIN);
    SmoothingFilterCoeff::new(rate, Consts::SMOOTH_SECONDS / block_size)
}

#[cfg(test)]
mod tests {
    use kithara_test_utils::kithara;

    use super::*;

    /// `IsolatorEq::new` takes a plain `u32`, so a caller can hand the bank a
    /// rate no filter can be built from. The band still has to arrive.
    #[kithara::test]
    fn a_bank_built_at_an_unusable_sample_rate_still_reaches_its_target() {
        let mut bank = GainBank::new([GainDb::default()].into_iter(), 0.0);
        bank.set(0, GainDb::MAX);

        for _ in 0..Consts::SMOOTH_BLOCK_SIZE * Consts::SMOOTH_BLOCK_SIZE {
            bank.tick();
        }

        assert_eq!(bank.linear(0), GainDb::MAX.linear());
    }
}
