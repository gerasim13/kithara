use core::{fmt, sync::atomic::Ordering};

use arc_swap::ArcSwap;
use kithara_platform::sync::Arc;
use portable_atomic::AtomicF32;

use crate::{effects::eq::GainDb, error::PlayError};

#[derive(Clone)]
pub struct SharedEq {
    gains: Arc<ArcSwap<Vec<AtomicF32>>>,
}

impl fmt::Debug for SharedEq {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SharedEq")
            .field("gains", &self.snapshot())
            .finish()
    }
}

impl SharedEq {
    #[must_use]
    pub fn new(bands: usize) -> Self {
        let gains = (0..bands).map(|_| AtomicF32::new(unity())).collect();
        Self {
            gains: Arc::new(ArcSwap::from_pointee(gains)),
        }
    }

    delegate::delegate! {
        to self.gains.load() {
            #[expr($.map(load_gain))]
            #[call(get)]
            pub(crate) fn gain(&self, band: usize) -> Option<f32>;
            pub(crate) fn len(&self) -> usize;
        }
    }

    pub(crate) fn reset(&self) {
        for gain in self.gains.load().iter() {
            gain.store(unity(), Ordering::Relaxed);
        }
    }

    pub(crate) fn set_gain(&self, band: usize, gain_db: GainDb) -> Result<(), PlayError> {
        let gains = self.gains.load();
        let Some(current) = gains.get(band) else {
            return Err(PlayError::EqBandOutOfRange {
                band,
                bands: gains.len(),
            });
        };
        current.store(f32::from(gain_db), Ordering::Relaxed);
        Ok(())
    }

    /// Returns one control-plane copy of the current band gains.
    #[must_use]
    pub fn snapshot(&self) -> Vec<f32> {
        self.gains.load().iter().map(load_gain).collect()
    }

    /// Replaces the complete control-plane band layout atomically.
    pub fn replace(&self, gains: &[GainDb]) {
        self.gains.store(Arc::new(band_array(gains)));
    }
}

fn band_array(gains: &[GainDb]) -> Vec<AtomicF32> {
    gains
        .iter()
        .copied()
        .map(f32::from)
        .map(AtomicF32::new)
        .collect()
}

fn unity() -> f32 {
    f32::from(GainDb::default())
}

fn load_gain(gain: &AtomicF32) -> f32 {
    gain.load(Ordering::Relaxed)
}

#[cfg(test)]
mod tests {
    use kithara_test_utils::kithara;

    use super::*;

    #[kithara::test]
    fn a_handle_clone_sees_the_replacement_band_array() {
        let eq = SharedEq::new(3);
        let handle = eq.clone();
        eq.set_gain(1, GainDb::from(4.0)).unwrap();
        assert_eq!(handle.snapshot(), vec![0.0, 4.0, 0.0]);

        eq.replace(&[-6.0f32, -2.0, 2.0, 5.0].map(GainDb::from));
        assert_eq!(handle.len(), 4);
        assert_eq!(handle.gain(2), Some(2.0));
        handle.set_gain(3, GainDb::from(1.0)).unwrap();
        assert_eq!(eq.snapshot(), vec![-6.0, -2.0, 2.0, 1.0]);
    }
}
