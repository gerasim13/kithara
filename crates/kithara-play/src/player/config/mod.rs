mod definition;
mod live;
mod settings;

pub use definition::{
    DEFAULT_CROSSFADE_DURATION, DEFAULT_PLAYING_RATE, PlayerConfig, PlayerConfigPatch,
};
pub(crate) use settings::{TrackSettings, TrackSettingsChange, TrackSettingsExec};
