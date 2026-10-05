use std::num::NonZeroUsize;

use kithara_config::Config;
use kithara_dsp::param::SmootherConfig;

use crate::consts::{DEFAULT_DECK_SLOTS, DEFAULT_DECLICK};

/// What a [`DeckMixer`](super::DeckMixer) is built with, fixed for its life.
#[derive(Clone, Copy, Debug, PartialEq, Config)]
#[config(default, fields(value, get(copy)))]
pub struct DeckMixerConfig {
    /// How many tracks the deck holds at once: an attach to a full deck evicts the track that
    /// matters least. Default: 4.
    #[config(builder(default = DEFAULT_DECK_SLOTS))]
    slots: NonZeroUsize,
    /// The ramp that opens the deck's output on start and closes it on pause. Default: 5 ms.
    #[config(builder(default = DEFAULT_DECLICK))]
    declick: SmootherConfig,
}
