use std::num::NonZeroU32;

use kithara_config::Config;

use crate::{
    MetronomeConfig, PlayError,
    api::{SessionDuckingMode, Tempo},
    consts,
};

/// What a Host runs with that changes while it runs.
///
/// A change of one field goes to the Host through
/// [`kithara_config::Configure`], at the next block or at a session frame;
/// the getters read the settings as the render last applied them.
#[derive(Clone, Copy, Debug, PartialEq, Config)]
#[config(
    default,
    builder(state_mod(vis = "pub")),
    check(error = PlayError),
    fields(value, get(copy))
)]
#[non_exhaustive]
pub struct HostSettings {
    /// Rate the Host asks the output to run at, 44.1 kHz unless changed. A
    /// change restarts the output route at the new rate.
    #[config(live(owner), builder(default = consts::DEFAULT_SAMPLE_RATE))]
    sample_rate: NonZeroU32,
    /// Tempo the session transport counts beats in, 120 BPM unless changed.
    #[config(live(owner), builder(default = Tempo::DEFAULT))]
    tempo: Tempo,
    /// How the Host metronome sounds, switched off unless changed.
    #[config(nested, live, builder(default))]
    metronome: MetronomeConfig,
    /// How far the whole session output is lowered under a competing sound,
    /// not at all unless changed.
    #[config(live, builder(default))]
    ducking: SessionDuckingMode,
}
