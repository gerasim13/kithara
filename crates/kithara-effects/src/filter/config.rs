use bon::bon;
use kithara_config::Config;
use kithara_dsp::filter::{Coefficients, FilterError, Hertz, Type};

/// The four causal conditioning responses supported by [`FilterConfig`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum FilterKind {
    HighPass,
    LowPass,
    LowShelf,
    HighShelf,
}

/// Parameters of one prepared second-order filter.
#[derive(Clone, Copy, Debug, PartialEq, Config)]
#[config(builder(existing), owner_access, fields(value, get(copy)))]
#[non_exhaustive]
pub struct FilterConfig {
    /// Pass response or shelf polarity.
    kind: FilterKind,
    /// Positive frequency in Hz, below Nyquist at preparation time.
    cutoff_hz: f64,
    /// Shelf gain in `[-24, 24]` dB; pass filters require zero.
    gain_db: f64,
    /// Finite positive RBJ quality factor.
    q: f64,
}

#[bon]
impl FilterConfig {
    #[builder(
        builder_type(vis = "pub"),
        start_fn(name = builder, vis = "pub"),
        finish_fn(vis = "pub")
    )]
    fn new(
        kind: FilterKind,
        cutoff_hz: f64,
        #[builder(default = std::f64::consts::FRAC_1_SQRT_2)] q: f64,
        #[builder(default)] gain_db: f64,
    ) -> Result<Self, FilterError> {
        if !cutoff_hz.is_finite()
            || cutoff_hz <= 0.0
            || !q.is_finite()
            || q <= 0.0
            || !gain_db.is_finite()
            || !(-24.0..=24.0).contains(&gain_db)
            || (matches!(kind, FilterKind::HighPass | FilterKind::LowPass) && gain_db != 0.0)
        {
            return Err(FilterError::Parameters);
        }
        Ok(Self {
            kind,
            cutoff_hz,
            gain_db,
            q,
        })
    }

    pub(super) fn coefficients(self, rate: f64) -> Result<Coefficients<f64>, FilterError> {
        if self.cutoff_hz >= rate / 2.0 {
            return Err(FilterError::Parameters);
        }
        let kind = match self.kind {
            FilterKind::HighPass => Type::HighPass,
            FilterKind::LowPass => Type::LowPass,
            FilterKind::LowShelf => Type::LowShelf(self.gain_db),
            FilterKind::HighShelf => Type::HighShelf(self.gain_db),
        };
        let design = || {
            Coefficients::from_params(
                kind,
                Hertz::from_hz(rate)?,
                Hertz::from_hz(self.cutoff_hz)?,
                self.q,
            )
        };
        design().map_err(|_| FilterError::Parameters)
    }
}
