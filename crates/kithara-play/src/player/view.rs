use crate::bridge::PlaybackSnapshot;

/// One coherent view of a player's live playback state.
///
/// Each field preserves its own unknown state. Decorators may refine the raw
/// player position while keeping the other fields from the same snapshot.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[non_exhaustive]
pub struct PlaybackView {
    /// Seconds playable without further network access.
    pub buffered: Option<f64>,
    /// Total media duration in seconds; `None` while unknown.
    pub duration: Option<f64>,
    /// Playback position in seconds; `None` until a stable value exists.
    pub position: Option<f64>,
    /// Whether playback is active.
    pub playing: bool,
}

impl From<PlaybackSnapshot> for PlaybackView {
    fn from(snapshot: PlaybackSnapshot) -> Self {
        Self {
            position: Some(snapshot.position()),
            duration: (snapshot.duration() > 0.0).then_some(snapshot.duration()),
            buffered: Some(snapshot.frontier().max(snapshot.cached())),
            playing: snapshot.is_playing(),
        }
    }
}
