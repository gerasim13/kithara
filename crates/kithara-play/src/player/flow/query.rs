use delegate::delegate;
use kithara_events::{EventBus, EventReceiver, EventSet};
use kithara_render::bridge::PlaybackSnapshot;

use super::super::{
    core::PlayerRuntime,
    view::{PlayerState, observed_playback},
};
use crate::{
    EngineLoadSnapshot, PlayWorker,
    api::{PlayerStatus, TrackId},
    engine::EngineImpl,
};

impl<S> PlayerRuntime<S> {
    /// ABR handle of the current item, if any.
    ///
    /// Reads the stash set when an item becomes current (its load, a
    /// crossfade commit or a gapless stitch), not when it is attached — stays
    /// valid for the whole life of the track.
    #[must_use]
    pub fn current_abr_handle(&self) -> Option<kithara_abr::AbrHandle> {
        self.phase.lock().abr_handle()
    }

    /// Current media duration in seconds.
    ///
    /// Returns `None` while duration is unknown — the engine sets the shared
    /// atomic from the demuxer once mvhd / fmt-equivalent metadata is parsed.
    /// The atomic's default `0.0` conflates "unknown" with "empty track";
    /// callers that distinguish (e.g. `seek_seconds`'s `target >= dur` check,
    /// queue auto-advance) need the `None` to avoid false-EOF on a freshly-
    /// loaded track whose demuxer has not yet seen the metadata box.
    pub fn duration_seconds(&self) -> Option<f64> {
        let dur = self.playback_snapshot()?.duration();
        (dur > 0.0).then_some(dur)
    }

    /// Live cost snapshot of the audio engine (decode + effects).
    #[must_use]
    pub fn engine_load(&self) -> EngineLoadSnapshot {
        self.core.engine_load.snapshot()
    }

    /// Single coherent read of the active slot's live playback scalars.
    ///
    /// `None` when no slot is allocated. The standalone `position_seconds`
    /// / `duration_seconds` / `is_playing` / `buffered_seconds` getters are
    /// thin derivations of this snapshot — one shared read primitive.
    pub fn playback_snapshot(&self) -> Option<PlaybackSnapshot> {
        let shared = self.slot_playback()?;
        Some(observed_playback(&shared, self.core.engine.suspended_at()))
    }

    /// Current playback position in seconds.
    ///
    /// The media clock owns the answer once a slot carries the track. Before
    /// that there is no clock to ask, and the only truth about the current
    /// item's playhead is the position handed over for it to start at: a host
    /// restoring a stored position reads it back here to draw its progress,
    /// and answering `None` is what puts the scrubber at the head of a track
    /// the player has already accepted a seek for.
    #[must_use]
    pub fn position_seconds(&self) -> Option<f64> {
        if let Some(snapshot) = self.playback_snapshot() {
            return Some(snapshot.position());
        }
        self.core
            .start_position
            .lock()
            .map(|held| held.as_secs_f64())
    }

    /// What this player's handles read, as it stands now.
    pub(crate) fn state(&self) -> PlayerState {
        PlayerState {
            closed: self.is_closed(),
            paused: self.is_paused(),
            playback: self.slot_playback(),
            suspended_at: self.core.engine.suspended_at(),
            start_position: *self.core.start_position.lock(),
            crossfade_duration: self.crossfade_duration(),
            default_rate: self.default_rate(),
            volume: self.volume(),
            muted: self.is_muted(),
            current_item: self.current_item(),
            armed_next: self.armed_next(),
            abr_handle: self.current_abr_handle(),
            sample_rate: self.sample_rate(),
            status: self.status(),
            eq_gains: (0..self.eq_band_count())
                .filter_map(|band| self.eq_gain(band))
                .collect(),
            prep: self.resource_prep(),
        }
    }

    /// Get current player status.
    pub fn status(&self) -> PlayerStatus {
        *self.core.status.lock()
    }

    /// Subscribe to player events.
    pub fn subscribe<E: EventSet>(&self) -> EventReceiver<E> {
        self.core.engine.bus().subscribe()
    }

    /// Shared playback worker configured for this Player.
    #[must_use]
    pub const fn worker(&self) -> &PlayWorker<S> {
        &self.core.config.worker
    }

    delegate! {
        to self.core {
            /// Get a reference to the underlying engine.
            #[field(&engine)]
            pub const fn engine(&self) -> &EngineImpl<S>;
        }
        to self.core.config {
            /// Get crossfade duration in seconds.
            pub fn crossfade_duration(&self) -> f32;
            /// Default playback-rate target used by `play()` and `select()`.
            pub fn default_rate(&self) -> f32;
            /// Returns `true` if the player is muted.
            pub fn is_muted(&self) -> bool;
            /// Get current volume (0.0..=1.0).
            pub fn volume(&self) -> f32;
        }
        to self.core.engine {
            /// Root event bus for this player.
            #[must_use]
            pub fn bus(&self) -> &EventBus;
            /// Number of EQ bands available for this player.
            pub fn eq_band_count(&self) -> usize;
            /// Get EQ gain for a band in dB.
            pub fn eq_gain(&self, band: usize) -> Option<f32>;
        }
        to self {
            /// Returns `true` if the player is in playing state.
            #[expr($.is_some_and(|s| s.is_playing()))]
            #[call(playback_snapshot)]
            pub fn is_playing(&self) -> bool;
            /// Current effective playback rate (`0.0` while paused or without a slot).
            #[expr($.map_or(0.0, |snapshot| snapshot.rate()))]
            #[call(playback_snapshot)]
            pub fn rate(&self) -> f32;
        }
        to self.core.current {
            /// The item the deck leads, as last announced by
            /// `CurrentItemChanged`; `None` before the first selection and
            /// once the deck is emptied.
            #[must_use]
            #[call(get)]
            pub fn current_item(&self) -> Option<TrackId>;
        }
    }
}
