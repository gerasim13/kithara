use delegate::delegate;
use kithara_bufpool::HasPool;
use kithara_events::EventBus;
use kithara_play::{EngineLoadSnapshot, EqBandConfig, PlayError, PlayerStatus};

use super::QueueControl;

impl<S> QueueControl<S>
where
    S: HasPool<u8> + HasPool<f32> + Send + Sync + 'static,
{
    /// Underlying event bus used by queue and player events.
    #[must_use]
    pub fn bus(&self) -> &EventBus {
        &self.bus
    }

    /// Drain pending player-side notifications. Called by FFI tick
    /// loops after [`Self::tick`].
    pub fn process_notifications(&self) {
        self.command(|queue| queue.player.process_notifications());
    }

    /// Reset all EQ bands to 0 dB.
    ///
    /// # Errors
    /// Forwards `PlayError` from the underlying player.
    pub fn reset_eq(&self) -> Result<(), PlayError> {
        self.with_open_result(|queue| queue.player.reset_eq())
    }

    pub fn set_crossfade_duration(&self, seconds: f32) {
        self.command(|queue| {
            queue.player.set_crossfade_duration(seconds);
            queue
                .bus
                .publish(kithara_events::QueueEvent::CrossfadeDurationChanged {
                    seconds: queue.player.crossfade_duration(),
                });
        });
    }

    /// Set the default playback rate.
    pub fn set_default_rate(&self, rate: f32) {
        self.command(|queue| queue.player.set_default_rate(rate));
    }

    /// Set gain for an EQ band.
    ///
    /// # Errors
    /// Forwards `PlayError` from the underlying player.
    pub fn set_eq_gain(&self, band: usize, gain_db: f32) -> Result<(), PlayError> {
        self.with_open_result(|queue| queue.player.set_eq_gain(band, gain_db))
    }

    /// Replace the live player's EQ band layout.
    ///
    /// # Errors
    /// Forwards `PlayError` from the underlying player.
    pub fn set_eq_layout(&self, layout: Vec<EqBandConfig>) -> Result<(), PlayError> {
        self.with_open_result(|queue| queue.player.set_eq_layout(layout))
    }

    /// Set the mute flag.
    pub fn set_muted(&self, muted: bool) {
        self.command(|queue| queue.player.set_muted(muted));
    }

    /// Set the live playback rate (mirrors into the tempo-mode sibling
    /// so a running key-locked stretch tracks the move).
    pub fn set_rate(&self, rate: f32) {
        self.command(|queue| queue.player.set_rate(rate));
    }

    /// Set the volume (0.0..=1.0).
    pub fn set_volume(&self, volume: f32) {
        self.command(|queue| queue.player.set_volume(volume));
    }

    delegate! {
        to self.player {
            /// Whether playback is active.
            #[must_use]
            pub fn is_playing(&self) -> bool;
            /// Current crossfade duration in seconds.
            #[must_use]
            pub fn crossfade_duration(&self) -> f32;
            /// Live engine playback rate (player-reported, 0.0 when paused).
            #[must_use]
            pub fn rate(&self) -> f32;
            /// Default playback rate.
            #[must_use]
            pub fn default_rate(&self) -> f32;
            /// Current volume (0.0..=1.0).
            #[must_use]
            pub fn volume(&self) -> f32;
            /// Whether output is muted.
            #[must_use]
            pub fn is_muted(&self) -> bool;
            /// Live engine playback status.
            #[must_use]
            pub fn status(&self) -> PlayerStatus;
            /// Live audio-engine cost (realtime factor / load / ms).
            #[must_use]
            pub fn engine_load(&self) -> EngineLoadSnapshot;
            /// Number of EQ bands.
            #[must_use]
            pub fn eq_band_count(&self) -> usize;
            /// Current gain for an EQ band.
            #[must_use]
            pub fn eq_gain(&self, band: usize) -> Option<f32>;
            /// Current track duration in seconds.
            #[must_use]
            pub fn duration_seconds(&self) -> Option<f64>;
        }
    }
}
