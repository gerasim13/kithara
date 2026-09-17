mod config;
mod control;
mod core;
mod flow;
mod protocol;
mod state;
mod view;

pub use core::{PlayerImpl, PlayerRuntime};

pub use config::{
    DEFAULT_CROSSFADE_DURATION, DEFAULT_PLAYING_RATE, PlayerConfig, PlayerConfigPatch,
};
pub use control::PlayerControl;
pub use flow::SelectTransition;
pub use protocol::{Player, PlayerControlSource, PlayerMember};
pub use view::PlaybackView;
