use num_traits::cast::AsPrimitive;

use super::GainDb;

struct Consts;

impl Consts {
    const DB_DIVISOR: f32 = 20.0;
    const LOG_FREQ_BASE: f32 = 10.0;
    const MS_PER_SEC: f32 = 1000.0;
    const SMOOTH_BLOCK_SIZE: usize = 32;
    const SMOOTH_CONVERGENCE_THRESHOLD: f32 = 0.0001;
    const SMOOTH_TIME_MS: f32 = 10.0;
}

struct GainState {
    current_linear: f32,
    target_db: GainDb,
    target_linear: f32,
}

impl GainState {
    fn new(gain_db: GainDb) -> Self {
        let linear = db_to_linear(f32::from(gain_db));
        Self {
            target_db: gain_db,
            target_linear: linear,
            current_linear: linear,
        }
    }

    fn set_target(&mut self, gain_db: GainDb) {
        let db = f32::from(gain_db);
        if (db - f32::from(self.target_db)).abs() < f32::EPSILON {
            return;
        }
        self.target_db = gain_db;
        self.target_linear = db_to_linear(db);
    }

    #[inline]
    fn smooth(&mut self, coeff: f32) {
        let diff = self.target_linear - self.current_linear;
        if diff.abs() < Consts::SMOOTH_CONVERGENCE_THRESHOLD {
            self.current_linear = self.target_linear;
        } else {
            self.current_linear = coeff.mul_add(diff, self.current_linear);
        }
    }
}

#[derive(fieldwork::Fieldwork)]
#[fieldwork(opt_in, get)]
pub(crate) struct GainBank {
    gains: Vec<GainState>,
    #[field(get, vis = "pub(crate)")]
    bypass_active: bool,
    #[field(get, vis = "pub(crate)")]
    silence_active: bool,
    smooth_coeff: f32,
    block_counter: usize,
}

impl GainBank {
    pub(crate) fn new(gains_db: impl Iterator<Item = GainDb>, sample_rate: f32) -> Self {
        let gains = gains_db.map(GainState::new).collect();
        let mut bank = Self {
            gains,
            smooth_coeff: compute_smooth_coeff(sample_rate),
            block_counter: 0,
            bypass_active: false,
            silence_active: false,
        };
        bank.refresh_fastpath();
        bank
    }

    #[cfg(test)]
    pub(crate) fn force_current(&mut self, band: usize, linear: f32) {
        self.gains[band].current_linear = linear;
        self.refresh_fastpath();
    }

    #[cfg(test)]
    pub(crate) fn is_smoothing(&self) -> bool {
        self.gains.iter().any(|gain| {
            (gain.target_linear - gain.current_linear).abs() > Consts::SMOOTH_CONVERGENCE_THRESHOLD
        })
    }

    delegate::delegate! {
        to self.gains {
            pub(crate) const fn len(&self) -> usize;
            #[expr($.map(|state| state.target_db))]
            #[call(get)]
            pub(crate) fn target(&self, band: usize) -> Option<GainDb>;
        }
    }

    pub(crate) fn linear(&self, band: usize) -> f32 {
        self.gains[band].current_linear
    }

    fn refresh_fastpath(&mut self) {
        self.bypass_active = !self.gains.is_empty()
            && self.gains.iter().all(|gain| {
                (gain.target_linear - 1.0).abs() < f32::EPSILON
                    && (gain.current_linear - 1.0).abs() < f32::EPSILON
            });
        self.silence_active = !self.gains.is_empty()
            && self.gains.iter().all(|gain| {
                gain.target_linear.abs() < f32::EPSILON && gain.current_linear.abs() < f32::EPSILON
            });
    }

    pub(crate) fn reset(&mut self) {
        for gain in &mut self.gains {
            gain.target_db = GainDb::default();
            gain.target_linear = 1.0;
            gain.current_linear = 1.0;
        }
        self.block_counter = 0;
        self.refresh_fastpath();
    }

    pub(crate) fn set(&mut self, band: usize, gain_db: GainDb) {
        if let Some(state) = self.gains.get_mut(band) {
            state.set_target(gain_db);
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
            gain.smooth(self.smooth_coeff);
        }
        self.refresh_fastpath();
    }

    pub(crate) fn update_sample_rate(&mut self, sample_rate: f32) {
        self.smooth_coeff = compute_smooth_coeff(sample_rate);
    }
}

#[inline]
fn db_to_linear(db: f32) -> f32 {
    if db <= f32::from(GainDb::MIN) {
        0.0
    } else {
        Consts::LOG_FREQ_BASE.powf(db / Consts::DB_DIVISOR)
    }
}

fn compute_smooth_coeff(sample_rate: f32) -> f32 {
    let tau = Consts::SMOOTH_TIME_MS / Consts::MS_PER_SEC;
    let block_size_f32: f32 = Consts::SMOOTH_BLOCK_SIZE.as_();
    let effective_rate = sample_rate / block_size_f32;
    1.0 - (-1.0 / (tau * effective_rate)).exp()
}

#[cfg(test)]
mod tests {
    use kithara_test_utils::kithara;

    use super::*;

    #[kithara::test]
    fn db_to_linear_kill_at_min() {
        assert!(db_to_linear(f32::from(GainDb::MIN)).abs() < f32::EPSILON);
        assert!(db_to_linear(-30.0).abs() < f32::EPSILON);
    }

    #[kithara::test]
    #[case::unity_at_zero(0.0, 1.0, 0.001)]
    #[case::boost_at_6db(6.0, 2.0, 0.02)]
    fn db_to_linear_maps_to_gain(#[case] db: f32, #[case] expected: f32, #[case] eps: f32) {
        assert!((db_to_linear(db) - expected).abs() < eps);
    }
}
