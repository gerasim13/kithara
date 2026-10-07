use delegate::delegate;
use kithara_bufpool::HasPool;
use kithara_events::EventBus;
use kithara_play::{CrossfadeSettings, EngineLoadSnapshot, PlayError, PlayerStatus, SuccessorLink};

use super::{Queue, QueueRuntime};
use crate::event::QueueEvent;

impl<S> QueueRuntime<S>
where
    S: HasPool<u8> + HasPool<f32> + Send + Sync + 'static,
{
    /// Underlying event bus used by queue and player events.
    #[must_use]
    pub fn bus(&self) -> &EventBus {
        &self.bus
    }

    #[must_use]
    pub fn crossfade_settings(&self) -> CrossfadeSettings {
        self.config.crossfade_settings()
    }

    delegate! {
        to self.player {
            /// Whether playback is active.
            #[must_use]
            pub fn is_playing(&self) -> bool;
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

impl<S> Queue<S>
where
    S: HasPool<u8> + HasPool<f32> + Send + Sync + 'static,
{
    pub(crate) fn set_crossfade_settings(
        &mut self,
        settings: CrossfadeSettings,
    ) -> Result<(), PlayError> {
        let settings = settings.validate()?;
        self.with_open_result(|queue| {
            queue.resident.set_crossfade_duration(settings.duration);
            let relinked = SuccessorLink::from(queue.config.crossfade_settings())
                != SuccessorLink::from(settings);
            queue.config.set_crossfade_settings(settings);
            if relinked && let Some(armed) = queue.player.armed_next() {
                queue.disarm_successor(armed);
            }
            queue.announce(QueueEvent::CrossfadeSettingsChanged { settings });
            Ok(())
        })
    }
}
