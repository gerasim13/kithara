use super::{Player, PlayerConfig, PlayerImpl, TrackCommand, TrackSettings, TrackSnapshot};
use crate::PlayError;

/// A player of one track: what a queue drives and builds the next track from.
pub trait Track<S>: Player<S, Command = TrackCommand<S>, Snapshot: AsRef<TrackSnapshot>> {
    /// The settings a track built after this one starts with: the applied ones and the changes still in flight.
    fn projected(&self) -> TrackSettings;
}

/// How a queue builds the track that plays one item.
pub trait TrackFactory<S> {
    type Track: Track<S>;

    /// # Errors
    ///
    /// Returns the config's refusal.
    fn track(&self, config: PlayerConfig) -> Result<Self::Track, PlayError>;
}

/// Builds a bare `PlayerImpl`.
#[derive(Clone, Copy, Debug, Default)]
pub struct PlayerFactory;

impl<S> TrackFactory<S> for PlayerFactory {
    type Track = PlayerImpl<S>;

    fn track(&self, config: PlayerConfig) -> Result<Self::Track, PlayError> {
        PlayerImpl::new(config)
    }
}
