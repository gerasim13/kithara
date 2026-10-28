use bon::bon;
use kithara_bufpool::PoolRegion;
use kithara_config::Config;
use kithara_dsp::filter::{Coefficients, FilterError, Hertz, Type};

use super::{Cutoff, Q};
use crate::GainDb;

/// The four causal conditioning responses supported by [`FilterConfig`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Kind {
    HighPass,
    LowPass,
    LowShelf,
    HighShelf,
}

/// Parameters and scratch resources of one prepared second-order filter.
#[derive(Config)]
#[derive_where::derive_where(Clone, Debug)]
#[config(builder(existing), fields(value, get(copy)))]
#[non_exhaustive]
pub struct FilterConfig<S> {
    /// Scratch storage supplied by the owning playback region.
    #[config(skip = "injected pool region", get(ref))]
    pools: PoolRegion<S>,
    /// Pass response or shelf polarity.
    kind: Kind,
    /// Frequency in Hz, below Nyquist at preparation time.
    cutoff_hz: Cutoff,
    /// Shelf gain in `[-24, 6]` dB; pass filters require zero.
    gain_db: GainDb,
    /// Resonance within the admitted positive range.
    q: Q,
}

#[bon]
impl<S> FilterConfig<S> {
    #[builder(
        builder_type(vis = "pub"),
        start_fn(name = builder, vis = "pub"),
        finish_fn(vis = "pub")
    )]
    fn new(
        #[builder(start_fn)] pools: PoolRegion<S>,
        kind: Kind,
        cutoff_hz: Cutoff,
        #[builder(default)] q: Q,
        #[builder(default)] gain_db: GainDb,
    ) -> Result<Self, FilterError> {
        if matches!(kind, Kind::HighPass | Kind::LowPass) && gain_db != GainDb::DEFAULT {
            return Err(FilterError::Parameters);
        }
        Ok(Self {
            pools,
            kind,
            cutoff_hz,
            gain_db,
            q,
        })
    }

    pub(super) fn coefficients(&self, rate: f64) -> Result<Coefficients<f64>, FilterError> {
        if f64::from(self.cutoff_hz) >= rate / 2.0 {
            return Err(FilterError::Parameters);
        }
        let kind = match self.kind {
            Kind::HighPass => Type::HighPass,
            Kind::LowPass => Type::LowPass,
            Kind::LowShelf => Type::LowShelf(f64::from(f32::from(self.gain_db))),
            Kind::HighShelf => Type::HighShelf(f64::from(f32::from(self.gain_db))),
        };
        let design = || {
            Coefficients::from_params(
                kind,
                Hertz::from_hz(rate)?,
                Hertz::from_hz(f64::from(self.cutoff_hz))?,
                f64::from(self.q),
            )
        };
        design().map_err(|_| FilterError::Parameters)
    }
}
