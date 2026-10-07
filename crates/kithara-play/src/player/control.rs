use delegate::delegate;
use kithara_abr::AbrHandle;
use kithara_audio::SeekOutcome;
use kithara_bufpool::HasPool;
use kithara_events::{EventBus, TrackId};
use kithara_platform::sync::Arc;
use kithara_render::bridge::RtMetricsSnapshot;

use super::{PlayerRuntime, SelectTransition};
use crate::{
    EngineLoadSnapshot, EqBandConfig, InterruptionKind, PlayError, PlaybackSnapshot, PlayerStatus,
    Resource, ResourceConfig, SelectionPlayback, SuccessorLink,
};

/// Cloneable runtime capability used by player-owned orchestration.
///
/// The handle deliberately excludes beat-grid identity, synchronization
/// topology, and engine/session getters. Closing the resident player
/// invalidates every outstanding clone through the shared runtime gate.
#[derive_where::derive_where(Clone)]
pub struct PlayerControl<S> {
    runtime: Arc<PlayerRuntime<S>>,
}

impl<S> PlayerControl<S>
where
    S: HasPool<f32>,
{
    pub(super) fn new(runtime: Arc<PlayerRuntime<S>>) -> Self {
        Self { runtime }
    }

    /// Attach `resource` to the deck as `item`, ahead of the current item and
    /// joined to it by `link`.
    ///
    /// # Errors
    /// Returns a closed-owner error, the failure to allocate its buffers, or
    /// the deck's refusal for want of room. Nothing is armed then, and the
    /// resource is spent: the item must be loaded again.
    pub fn arm_next(
        &self,
        item: TrackId,
        resource: Resource,
        link: SuccessorLink,
    ) -> Result<(), PlayError> {
        self.runtime
            .with_open_result(|runtime| runtime.arm_next(item, resource, link))
    }

    /// Drop the armed successor from the deck without committing it.
    pub fn unarm_next(&self) {
        self.command(PlayerRuntime::unarm_next);
    }

    /// Root event bus used to scope per-track loader events.
    #[must_use]
    pub fn bus(&self) -> EventBus {
        self.runtime.bus().clone()
    }

    fn command(&self, command: impl FnOnce(&PlayerRuntime<S>)) {
        let _ = self.runtime.with_open(command);
    }

    /// Whether playback is explicitly paused.
    #[must_use]
    pub fn is_paused(&self) -> bool {
        self.runtime.is_closed() || self.runtime.is_paused()
    }

    /// Whether the resident player is currently active.
    #[must_use]
    pub fn is_playing(&self) -> bool {
        !self.runtime.is_closed() && self.runtime.is_playing()
    }

    /// Record that the platform interrupted, or released, the audio output.
    pub fn notify_interruption(&self, kind: InterruptionKind) {
        self.command(|runtime| runtime.notify_interruption(kind));
    }

    /// Pause playback unless the owning player is closed.
    pub fn pause(&self) {
        self.command(PlayerRuntime::pause);
    }

    /// Start or resume playback unless the owning player is closed.
    pub fn play(&self) {
        self.command(PlayerRuntime::play);
    }

    /// Prepare one resource for this player's runtime.
    pub fn prepare_config<B>(
        &self,
        config: ResourceConfig<S, B>,
    ) -> Result<ResourceConfig<S, B>, PlayError>
    where
        B: Clone + Default,
        S: HasPool<u8> + HasPool<f32> + Send + Sync + 'static,
    {
        self.runtime
            .with_open_result(|runtime| runtime.prepare_config(config))
    }

    /// Drain pending player notifications.
    pub fn process_notifications(&self) {
        self.command(PlayerRuntime::process_notifications);
    }

    /// Drop every track the deck holds and release its slot.
    pub fn remove_all_items(&self) {
        self.command(PlayerRuntime::remove_all_items);
    }

    /// Reset all EQ bands.
    pub fn reset_eq(&self) -> Result<(), PlayError> {
        self.runtime.with_open_result(PlayerRuntime::reset_eq)
    }

    /// Seek within the current player item.
    pub fn seek_seconds(&self, seconds: f64) -> Result<SeekOutcome, PlayError> {
        self.runtime
            .with_open_result(|runtime| runtime.seek_seconds(seconds))
    }

    /// Make `item` current with the configured crossfade.
    ///
    /// # Errors
    /// As [`Self::select_with_crossfade`].
    pub fn select(
        &self,
        item: TrackId,
        resource: Option<Resource>,
        playback: SelectionPlayback,
    ) -> Result<(), PlayError> {
        self.runtime
            .with_open_result(|runtime| runtime.select(item, resource, playback))
    }

    /// Make `item` current: a given `resource` loads as `item`; without one
    /// the deck commits its armed successor `item` or reselects its current
    /// item.
    ///
    /// # Errors
    /// Returns a closed-owner error, [`PlayError::ItemConsumed`] when the deck
    /// holds no `item` and no resource came, or the load's failure. The
    /// resource is spent on any error.
    pub fn select_with_crossfade(
        &self,
        item: TrackId,
        resource: Option<Resource>,
        transition: SelectTransition,
    ) -> Result<(), PlayError> {
        self.runtime
            .with_open_result(|runtime| runtime.select_with_crossfade(item, resource, transition))
    }

    /// Update crossfade duration unless the owning player is closed.
    pub fn set_crossfade_duration(&self, seconds: f32) {
        self.command(|runtime| runtime.set_crossfade_duration(seconds));
    }

    /// Update the default playback rate unless the owning player is closed.
    pub fn set_default_rate(&self, rate: f32) {
        self.command(|runtime| runtime.set_default_rate(rate));
    }

    /// Update one EQ band.
    pub fn set_eq_gain(&self, band: usize, gain_db: f32) -> Result<(), PlayError> {
        self.runtime
            .with_open_result(|runtime| runtime.set_eq_gain(band, gain_db))
    }

    /// Replace the EQ band layout.
    pub fn set_eq_layout(&self, layout: Vec<EqBandConfig>) -> Result<(), PlayError> {
        self.runtime
            .with_open_result(|runtime| runtime.set_eq_layout(layout))
    }

    /// Set the deck's mix level, a linear amplitude in `0.0..=1.0` over its volume.
    ///
    /// # Errors
    /// Returns a closed-owner error, [`PlayError::MixLevel`] for a level outside
    /// `0.0..=1.0`, or the deck's refusal of the change.
    pub fn set_level(&self, level: f32) -> Result<(), PlayError> {
        self.runtime
            .with_open_result(|runtime| runtime.set_level(level))
    }

    /// Update mute state unless the owning player is closed.
    pub fn set_muted(&self, muted: bool) {
        self.command(|runtime| runtime.set_muted(muted));
    }

    /// Update live playback rate unless the owning player is closed.
    pub fn set_rate(&self, rate: f32) {
        self.command(|runtime| runtime.set_rate(rate));
    }

    /// Update output volume unless the owning player is closed.
    pub fn set_volume(&self, volume: f32) {
        self.command(|runtime| runtime.set_volume(volume));
    }

    /// Advance player control-plane work.
    pub fn tick(&self) -> Result<(), PlayError> {
        self.runtime.with_open_result(PlayerRuntime::tick)
    }

    delegate! {
        to self.runtime {
            /// Whether the owning player has been closed or is closing.
            #[must_use]
            pub fn is_closed(&self) -> bool;
            /// Close the player runtime and invalidate every outstanding control.
            ///
            /// # Errors
            ///
            /// Returns [`PlayError::Closed`] while another close is under way.
            pub fn close(&self) -> Result<(), PlayError>;
            /// Configured crossfade duration in seconds.
            #[must_use]
            pub fn crossfade_duration(&self) -> f32;
            /// The item the deck leads, as last announced.
            #[must_use]
            pub fn current_item(&self) -> Option<TrackId>;
            /// The successor armed on the deck and not yet committed.
            #[must_use]
            pub fn armed_next(&self) -> Option<TrackId>;
            /// Latest playback position.
            #[must_use]
            pub fn position_seconds(&self) -> Option<f64>;
            /// Latest coherent playback state.
            #[must_use]
            pub fn playback_snapshot(&self) -> Option<PlaybackSnapshot>;
            /// Current ABR handle for the active item.
            #[must_use]
            pub fn current_abr_handle(&self) -> Option<AbrHandle>;
            /// Current live playback rate.
            #[must_use]
            pub fn rate(&self) -> f32;
            /// Rate the player's master bus runs at.
            #[must_use]
            pub fn sample_rate(&self) -> u32;
            /// Configured default playback rate.
            #[must_use]
            pub fn default_rate(&self) -> f32;
            /// Current output volume.
            #[must_use]
            pub fn volume(&self) -> f32;
            /// Whether output is muted.
            #[must_use]
            pub fn is_muted(&self) -> bool;
            /// Current player status.
            #[must_use]
            pub fn status(&self) -> PlayerStatus;
            /// Current engine cost snapshot.
            #[must_use]
            pub fn engine_load(&self) -> EngineLoadSnapshot;
            /// Read the active audio slot's real-time counters.
            #[must_use]
            pub fn rt_metrics(&self) -> Option<RtMetricsSnapshot>;
            /// Number of EQ bands.
            #[must_use]
            pub fn eq_band_count(&self) -> usize;
            /// Gain of one EQ band.
            #[must_use]
            pub fn eq_gain(&self, band: usize) -> Option<f32>;
            /// Current item duration.
            #[must_use]
            pub fn duration_seconds(&self) -> Option<f64>;
        }
    }
}
