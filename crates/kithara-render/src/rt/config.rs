use std::num::NonZeroUsize;

use kithara_config::Config;
use kithara_dsp::param::SmootherConfig;
use kithara_signal::FrameCount;

use crate::{
    bridge::DeckMixSettings,
    consts::{DEFAULT_DECK_SLOTS, DEFAULT_DECLICK, DEFAULT_EVICT_FADE},
};

/// What a [`DeckMixer`](super::DeckMixer) is built with, fixed for its life.
#[derive(Clone, Copy, Debug, PartialEq, Config)]
#[config(default, fields(value, get(copy)))]
pub struct DeckMixerConfig {
    /// How many tracks the deck holds at once; its owner assigns them. Default: 4.
    #[config(builder(default = DEFAULT_DECK_SLOTS))]
    slots: NonZeroUsize,
    /// The ramp a slot starts and stops with. Default: 5 ms.
    #[config(builder(default = DEFAULT_DECLICK))]
    declick: SmootherConfig,
    /// Frames of a replaced consumer that play out of its slot's tail, ramped down to silence.
    /// Default: 512.
    #[config(builder(default = DEFAULT_EVICT_FADE))]
    evict_fade: FrameCount,
    /// How loud the deck sounds before its owner changes it.
    #[config(builder(default))]
    mix: DeckMixSettings,
}
