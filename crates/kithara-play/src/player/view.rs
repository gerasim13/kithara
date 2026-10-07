use arc_swap::ArcSwap;
use kithara_abr::AbrHandle;
use kithara_bufpool::HasPool;
use kithara_events::{EventBus, TrackId};
use kithara_platform::{
    sync::{Arc, atomic::Ordering},
    time::Duration,
};
use kithara_render::bridge::{PlaybackShared, PlaybackSnapshot};

use super::flow::ResourcePrep;
use crate::{EngineLoad, EngineLoadSnapshot, PlayError, PlayWorker, PlayerStatus, ResourceConfig};

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

/// What a player's owner last published for its handles to read.
pub(crate) struct PlayerState {
    pub(crate) closed: bool,
    pub(crate) paused: bool,
    /// The active slot's live playback, which its audio thread publishes.
    pub(crate) playback: Option<Arc<PlaybackShared>>,
    /// The audio-thread tick the output was suspended at, while it is.
    pub(crate) suspended_at: Option<u64>,
    /// Where the current item starts once a slot carries it.
    pub(crate) start_position: Option<Duration>,
    pub(crate) crossfade_duration: f32,
    pub(crate) default_rate: f32,
    pub(crate) volume: f32,
    pub(crate) muted: bool,
    pub(crate) current_item: Option<TrackId>,
    pub(crate) armed_next: Option<TrackId>,
    pub(crate) abr_handle: Option<AbrHandle>,
    pub(crate) sample_rate: u32,
    pub(crate) status: PlayerStatus,
    pub(crate) eq_gains: Vec<f32>,
    /// What a resource prepared for the player takes from it.
    pub(crate) prep: ResourcePrep,
}

/// What `shared` tells a reader while the output may be suspended at
/// `suspended_at`.
///
/// A suspended output leaves its processor unscheduled, so what it last
/// published describes an output that is gone: while the audio thread still
/// stands where the suspension found it, the snapshot reads silenced. The
/// call count is read first, so the values read after it are at least what
/// the blocks before the counted one published.
pub(crate) fn observed_playback(
    shared: &PlaybackShared,
    suspended_at: Option<u64>,
) -> PlaybackSnapshot {
    let calls = shared.process_count.load(Ordering::Acquire);
    let snapshot = shared.snapshot();
    let stalled = suspended_at.is_some_and(|tick| (tick..=tick.saturating_add(1)).contains(&calls));
    if stalled {
        snapshot.silenced()
    } else {
        snapshot
    }
}

/// A player's state as its handles read it.
///
/// The player's owner publishes it after each command it runs, before it
/// answers, and on every tick, so a handle that waited for an answer reads
/// its own write. Values the audio thread owns are read live from the slot
/// the owner last published.
#[derive(Clone)]
pub struct PlayerView {
    state: Arc<ArcSwap<PlayerState>>,
    engine_load: Arc<EngineLoad>,
    bus: EventBus,
}

impl PlayerView {
    pub(crate) fn new(state: PlayerState, engine_load: Arc<EngineLoad>, bus: EventBus) -> Self {
        Self {
            state: Arc::new(ArcSwap::from_pointee(state)),
            engine_load,
            bus,
        }
    }

    pub(crate) fn publish(&self, state: PlayerState) {
        self.state.store(Arc::new(state));
    }

    /// Root event bus of the player.
    #[must_use]
    pub fn bus(&self) -> EventBus {
        self.bus.clone()
    }

    delegate::delegate! {
        to self.state {
            /// Configured crossfade duration in seconds.
            #[must_use]
            #[expr($.crossfade_duration)]
            #[call(load)]
            pub fn crossfade_duration(&self) -> f32;
            /// The item the deck leads, as last announced.
            #[must_use]
            #[expr($.current_item)]
            #[call(load)]
            pub fn current_item(&self) -> Option<TrackId>;
            /// The successor armed on the deck and not yet committed.
            #[must_use]
            #[expr($.armed_next)]
            #[call(load)]
            pub fn armed_next(&self) -> Option<TrackId>;
            /// Configured default playback rate.
            #[must_use]
            #[expr($.default_rate)]
            #[call(load)]
            pub fn default_rate(&self) -> f32;
            /// Whether the player has been closed.
            #[must_use]
            #[expr($.closed)]
            #[call(load)]
            pub fn is_closed(&self) -> bool;
            /// Whether output is muted.
            #[must_use]
            #[expr($.muted)]
            #[call(load)]
            pub fn is_muted(&self) -> bool;
            /// Rate the player's master bus runs at.
            #[must_use]
            #[expr($.sample_rate)]
            #[call(load)]
            pub fn sample_rate(&self) -> u32;
            /// Current player status.
            #[must_use]
            #[expr($.status)]
            #[call(load)]
            pub fn status(&self) -> PlayerStatus;
            /// Current output volume.
            #[must_use]
            #[expr($.volume)]
            #[call(load)]
            pub fn volume(&self) -> f32;
        }
        to self.state.load().eq_gains {
            /// Number of EQ bands.
            #[must_use]
            #[call(len)]
            pub fn eq_band_count(&self) -> usize;
            /// Gain of one EQ band in dB.
            #[must_use]
            #[expr($.copied())]
            #[call(get)]
            pub fn eq_gain(&self, band: usize) -> Option<f32>;
        }
    }

    /// ABR handle of the current item.
    #[must_use]
    pub fn current_abr_handle(&self) -> Option<AbrHandle> {
        self.state.load().abr_handle.clone()
    }

    /// Current item duration; `None` while unknown.
    #[must_use]
    pub fn duration_seconds(&self) -> Option<f64> {
        let duration = self.playback_snapshot()?.duration();
        (duration > 0.0).then_some(duration)
    }

    /// Live cost of the audio engine.
    #[must_use]
    pub fn engine_load(&self) -> EngineLoadSnapshot {
        self.engine_load.snapshot()
    }

    /// Whether playback is explicitly paused; a closed player is.
    #[must_use]
    pub fn is_paused(&self) -> bool {
        let state = self.state.load();
        state.closed || state.paused
    }

    /// Whether the player is playing; a closed player is not.
    #[must_use]
    pub fn is_playing(&self) -> bool {
        !self.is_closed() && self.playback_snapshot().is_some_and(|s| s.is_playing())
    }

    /// The active slot's live playback state; `None` without a slot.
    #[must_use]
    pub fn playback_snapshot(&self) -> Option<PlaybackSnapshot> {
        let state = self.state.load();
        let shared = state.playback.as_deref()?;
        Some(observed_playback(shared, state.suspended_at))
    }

    /// Playback position in seconds: the slot's clock once a slot carries
    /// the item, the position it will start at before that.
    #[must_use]
    pub fn position_seconds(&self) -> Option<f64> {
        if let Some(snapshot) = self.playback_snapshot() {
            return Some(snapshot.position());
        }
        self.state
            .load()
            .start_position
            .map(|held| held.as_secs_f64())
    }

    /// Current effective playback rate; `0.0` without a slot.
    #[must_use]
    pub fn rate(&self) -> f32 {
        self.playback_snapshot().map_or(0.0, |s| s.rate())
    }

    /// Prepares `config` for a resource this player will play on `worker`,
    /// the player's own: its output rate and shape, the next track's warp,
    /// bus, and cancel scope, as the player last published them.
    ///
    /// # Errors
    /// [`PlayError::Closed`] after close, or a buffer geometry the player's
    /// response budget cannot hold.
    pub fn prepare_config<S, B>(
        &self,
        config: ResourceConfig<S, B>,
        worker: PlayWorker<S>,
    ) -> Result<ResourceConfig<S, B>, PlayError>
    where
        S: HasPool<u8> + HasPool<f32> + Send + Sync + 'static,
        B: Clone + Default,
    {
        let state = self.state.load();
        if state.closed {
            return Err(PlayError::Closed);
        }
        state.prep.prepare(config, worker)
    }
}

#[cfg(test)]
mod tests {
    use kithara_render::{bridge::PlaybackShared, mock};
    use kithara_test_utils::kithara;

    use super::*;

    fn view_of(frontier: f64, cached: f64) -> PlaybackView {
        let playback = PlaybackShared::default();
        mock::publish_playhead(&playback, 0.0, 200.0);
        mock::publish_buffered(&playback, frontier, cached);
        PlaybackView::from(playback.snapshot())
    }

    /// A fully downloaded track must report its cached span, not the sliver
    /// the decoder has produced — that span is what a host progress bar and
    /// `loadedTimeRanges` mean by "available without more network".
    #[kithara::test]
    fn buffered_covers_the_cached_span() {
        assert_eq!(view_of(4.0, 120.0).buffered, Some(120.0));
    }

    /// The frontier is a floor, not a value the cached span replaces: a
    /// reported window that falls behind the playhead makes the host pause
    /// into a buffering deadlock.
    #[kithara::test]
    fn buffered_never_falls_behind_the_decoded_frontier() {
        assert_eq!(view_of(90.0, 12.0).buffered, Some(90.0));
    }

    #[kithara::test]
    fn buffered_is_zero_when_nothing_is_available() {
        assert_eq!(view_of(0.0, 0.0).buffered, Some(0.0));
    }
}
