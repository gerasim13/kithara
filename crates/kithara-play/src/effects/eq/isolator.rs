use kithara_signal::sanitize_sample;
use num_traits::cast::AsPrimitive;

use super::{EqBandConfig, EqConfig, GainDb, filter::CrossoverFilters, gain::GainBank};

/// Single-channel isolator crossover EQ.
#[non_exhaustive]
pub struct IsolatorEq {
    filters: CrossoverFilters,
    gains: GainBank,
    was_in_fastpath: bool,
}

impl IsolatorEq {
    #[must_use]
    pub fn new(config: &EqConfig, bands: &[EqBandConfig], sample_rate: u32) -> Self {
        let sample_rate: f32 = sample_rate.as_();
        let crossover_freqs = config.sample_pool().collect(
            bands
                .windows(2)
                .map(|pair| (pair[0].frequency() * pair[1].frequency()).sqrt()),
        );
        Self {
            filters: CrossoverFilters::new(config.sample_pool(), crossover_freqs, sample_rate),
            gains: GainBank::new(bands.iter().map(EqBandConfig::gain_db), sample_rate),
            was_in_fastpath: false,
        }
    }

    delegate::delegate! {
        to self.gains {
            #[must_use]
            #[call(len)]
            pub const fn band_count(&self) -> usize;
            #[must_use]
            #[call(target)]
            pub fn target_gain(&self, band: usize) -> Option<GainDb>;
            #[cfg(test)]
            pub(crate) fn bypass_active(&self) -> bool;
            #[cfg(test)]
            pub(crate) fn is_smoothing(&self) -> bool;
            #[call(set)]
            pub fn set_gain(&mut self, band: usize, gain_db: GainDb);
            #[cfg(test)]
            #[call(settle)]
            pub(crate) fn settle_gain(&mut self, band: usize);
            #[cfg(test)]
            pub(crate) fn silence_active(&self) -> bool;
        }
    }

    #[inline]
    pub fn process_sample(&mut self, input: f32) -> f32 {
        // Guarding the input covers the bypass and silence paths too.
        let input = sanitize_sample(input);
        self.gains.tick();
        if self.gains.silence_active() {
            self.filters.record(input);
            self.was_in_fastpath = true;
            return 0.0;
        }
        if self.gains.bypass_active() {
            self.filters.record(input);
            self.was_in_fastpath = true;
            return input;
        }
        if self.was_in_fastpath {
            self.was_in_fastpath = false;
            self.filters.rehydrate();
        }
        match self.gains.len() {
            0 => input,
            1 => sanitize_sample(input * self.gains.linear(0)),
            _ => sanitize_sample(self.filters.process(input, |band| self.gains.linear(band))),
        }
    }

    pub fn reset(&mut self) {
        self.gains.reset();
        self.filters.reset();
        self.was_in_fastpath = false;
    }

    pub fn update_sample_rate(&mut self, sample_rate: u32) {
        let sample_rate = sample_rate.as_();
        self.gains.update_sample_rate(sample_rate);
        self.filters.update_sample_rate(sample_rate);
    }
}

#[cfg(test)]
mod tests {
    use kithara_bufpool::SamplePool;
    use kithara_test_utils::kithara;

    use super::*;

    #[kithara::test]
    fn a_decaying_tail_never_leaks_denormals() {
        const SAMPLE_RATE: u32 = 48_000;
        const TAIL_SECONDS: u32 = 4;

        let bands = super::super::band::generate_log_spaced_bands(3);
        let config = EqConfig::for_pool(SamplePool::default()).build();
        let mut eq = IsolatorEq::new(&config, &bands, SAMPLE_RATE);
        for band in 0..bands.len() {
            eq.set_gain(band, GainDb::MAX);
        }

        let _ = eq.process_sample(1.0);
        let denormals = (0..SAMPLE_RATE * TAIL_SECONDS)
            .map(|_| eq.process_sample(0.0))
            .filter(|out| *out != 0.0 && out.abs() < f32::MIN_POSITIVE)
            .count();

        assert_eq!(denormals, 0, "impulse tail leaked {denormals} denormals");
    }

    #[kithara::test]
    fn reusable_storage_returns_to_the_injected_pool() {
        let pool = SamplePool::new(32, 200_000);
        let bands = super::super::band::generate_log_spaced_bands(3);
        let config = EqConfig::for_pool(pool.clone()).build();

        let first = IsolatorEq::new(&config, &bands, 48_000);
        let misses_after_first = pool.stats().alloc_misses;
        drop(first);

        let hits_before_second = pool.stats().home_hits + pool.stats().steal_hits;
        let _second = IsolatorEq::new(&config, &bands, 48_000);
        let stats = pool.stats();

        assert_eq!(stats.alloc_misses, misses_after_first);
        assert!(stats.home_hits + stats.steal_hits > hits_before_second);
    }
}
