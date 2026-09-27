#[cfg(feature = "analysis-beat")]
pub(crate) mod beat;
mod intake;
#[cfg(not(feature = "analysis-beat"))]
pub(crate) mod nobeat;
#[cfg(not(feature = "analysis-waveform"))]
pub(crate) mod nowaveform;
#[cfg(feature = "analysis-waveform")]
pub(crate) mod waveform;

pub(crate) use intake::{Intake, Opens};
#[cfg(not(feature = "analysis-beat"))]
pub(crate) use nobeat as beat;
#[cfg(not(feature = "analysis-waveform"))]
pub(crate) use nowaveform as waveform;
