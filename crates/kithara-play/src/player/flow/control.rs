use kithara_bufpool::HasPool;
use kithara_effects::{GainDb, eq::EqBandConfig};
use kithara_platform::sync::atomic::Ordering;
use kithara_render::bridge::{DeckMixSettingsChange, DeckPart};
use kithara_signal::FaderValue;
use kithara_test_macros as kithara;
use kithara_warp::MIN_SPEED;
use tracing::warn;

use super::super::core::PlayerRuntime;
use crate::{
    api::{InterruptionKind, SessionEvent, SlotId},
    error::PlayError,
};

impl<S> PlayerRuntime<S> {
    /// Ensure we hold the deck's slot, taking it if we do not.
    pub fn ensure_slot(&self) -> Result<SlotId, PlayError>
    where
        S: HasPool<f32>,
    {
        if let Some(id) = self.slot() {
            return Ok(id);
        }
        let id = self.core.engine.slot().ok_or(PlayError::EngineNotRunning)?;
        self.enter_loading_with_slot(id);
        self.core.engine.send_slot_cmd(
            id,
            vec![
                DeckPart::Mix(DeckMixSettingsChange::Volume(FaderValue::from(
                    self.volume(),
                ))),
                DeckPart::Mix(DeckMixSettingsChange::Muted(self.is_muted())),
                DeckPart::Mix(DeckMixSettingsChange::Level(self.core.config.level())),
                self.core.engine.eq_part()?,
            ],
        )?;
        Ok(id)
    }

    /// Notify the player that the platform interrupted, or released, the audio
    /// output.
    ///
    /// An interruption stops the output below us — the RT processor is no
    /// longer scheduled, so it can neither observe the interruption nor report
    /// it. The fact enters here and playback state reads it from the session.
    /// Handing the output back is the route-invalidation path, which is what
    /// rebuilds the stream.
    pub fn notify_interruption(&self, kind: InterruptionKind) {
        if matches!(kind, InterruptionKind::Began) {
            let tick = self
                .slot_playback()
                .map_or(0, |shared| shared.process_count.load(Ordering::Relaxed));
            self.core.engine.suspend_output(tick);
        }
        self.core
            .engine
            .bus()
            .publish(SessionEvent::Interruption { kind });
    }

    /// Reset EQ gains to 0 dB for all bands.
    pub fn reset_eq(&self) -> Result<(), PlayError> {
        for band in 0..self.core.engine.eq_band_count() {
            self.set_eq_gain(band, 0.0)?;
        }
        Ok(())
    }

    /// Set the crossfade duration, in seconds, of transitions nobody gives settings of their own.
    pub fn set_crossfade_duration(&self, seconds: f32) {
        self.core.config.set_crossfade_duration(seconds);
    }

    /// Set the playback rate used by `play()` and `select()`, and apply it
    /// as a target to playback that is already running.
    ///
    /// While paused the live rate is 0.0 and must stay there — a rate change is
    /// not a resume. The new value takes effect on the next `play()`.
    pub fn set_default_rate(&self, rate: f32) {
        let target = self.core.config.set_default_rate(rate);
        self.set_rate(target);
    }

    /// Set EQ gain for a band in dB; an idle player keeps it for its next slot.
    ///
    /// # Errors
    /// Returns [`PlayError::EqBandOutOfRange`] for a band the layout does not have, and the
    /// deck's refusal of the gain.
    pub fn set_eq_gain(&self, band: usize, gain_db: f32) -> Result<(), PlayError> {
        self.core
            .engine
            .set_eq_gain(band, GainDb::from(gain_db), |part| self.send_to_slot(part))
    }

    /// Replaces the EQ layout and gains without releasing the running slot: the deck crosses
    /// over to it; an idle player keeps it for its next slot.
    ///
    /// # Errors
    /// Returns the pool's refusal to build the layout and the deck's refusal of it.
    pub fn set_eq_layout(&self, layout: Vec<EqBandConfig>) -> Result<(), PlayError>
    where
        S: HasPool<f32>,
    {
        self.core
            .engine
            .set_eq_layout(layout, |part| self.send_to_slot(part))
    }

    /// Set the deck's mix level, a linear amplitude in `0.0..=1.0` over its volume.
    ///
    /// # Errors
    /// Returns [`PlayError::MixLevel`] for a level outside `0.0..=1.0` and the deck's refusal
    /// of the change; the level stays as it was then.
    pub fn set_level(&self, level: f32) -> Result<(), PlayError> {
        self.core
            .config
            .set_level(level, |part| self.send_to_slot(part))
    }

    /// Set muted state.
    pub fn set_muted(&self, muted: bool) {
        if let Err(error) = self.core.config.set_muted(
            muted,
            |part| self.send_to_slot(part),
            self.core.engine.bus(),
        ) {
            warn!(?error, muted, "mute update rejected");
        }
    }

    /// Set the requested rate target, clamped to [`MIN_SPEED`].
    pub fn set_rate(&self, rate: f32) {
        let target = rate.max(MIN_SPEED);
        let snapshot = self
            .slot()
            .and_then(|slot| self.core.engine.slot_render_snapshot(slot));
        let change = self.core.tracks.lock().set_next_speed(target);
        let change = match change {
            Ok(change) => change,
            Err(error) => {
                warn!(%error, rate, "rate refused");
                return;
            }
        };
        let configured = self.with_tracks(|tracks, out| {
            tracks.configure(change, out, |seq| {
                if let Some(snapshot) = &snapshot {
                    kithara::probe_event!(
                        rate_requested,
                        request_revision = seq.get(),
                        target_rate_bits = target.to_bits(),
                        session_epoch = u64::from(snapshot.context().output().session_epoch()),
                        transport_revision = snapshot
                            .context()
                            .output()
                            .transport_revision()
                            .map_or(0, u64::from),
                        session_frame = i64::from(snapshot.context().output().output_frames().end)
                    );
                }
            })
        });
        match configured {
            Ok(()) | Err(PlayError::NoActiveSlot) => {}
            Err(error) => warn!(%error, rate = target, "rate not sent to the lanes"),
        }
        match self.send_to_slot(DeckPart::SetRate(target)) {
            Ok(()) | Err(PlayError::NoActiveSlot) => {}
            Err(error) => warn!(?error, rate = target, "rate not sent to the processor"),
        }
        self.core.config.worker.wake();
    }

    /// Set volume, clamped to `0.0..=1.0`.
    pub fn set_volume(&self, volume: f32) {
        if let Err(error) = self.core.config.set_volume(
            volume,
            |part| self.send_to_slot(part),
            self.core.engine.bus(),
        ) {
            warn!(?error, volume, "volume update rejected");
        }
    }

    delegate::delegate! {
        to self.core.engine {
            /// Pump audio backend/runtime state.
            pub fn tick(&self) -> Result<(), PlayError>;
        }
    }
}

#[cfg(test)]
mod tests {
    use kithara_effects::GainDb;
    use kithara_render::bridge::{DeckEqChange, DeckPart};
    use kithara_test_utils::kithara;

    use crate::{
        PlayWorker, PlayWorkerConfig, mock,
        player::{PlayerConfig, PlayerImpl},
        test_pools::pools,
    };

    /// The deck's slot is built with the deck: a player takes it without asking the session.
    #[kithara::test]
    fn a_player_takes_its_deck_slot_without_asking_the_session() {
        let mut player = PlayerImpl::new(
            PlayerConfig::builder()
                .sample_rate(mock::SAMPLE_RATE)
                .worker(PlayWorker::new(PlayWorkerConfig::builder(pools()).build()))
                .build(),
        );
        let audio_thread = mock::insert(&mut player);

        player.ensure_slot().expect("the deck's slot is handed out");

        assert!(
            audio_thread.asked().is_empty(),
            "{:?}",
            audio_thread.asked()
        );
    }

    /// An EQ gain is the deck's: a slot is handed the layout when it is taken, and a band cut
    /// goes to that slot's ring.
    #[kithara::test]
    fn an_eq_cut_rides_the_deck_not_the_session() {
        let mut player = PlayerImpl::new(
            PlayerConfig::builder()
                .sample_rate(mock::SAMPLE_RATE)
                .worker(PlayWorker::new(PlayWorkerConfig::builder(pools()).build()))
                .build(),
        );
        let audio_thread = mock::insert(&mut player);
        player.ensure_slot().expect("slot allocation must succeed");

        player.set_eq_gain(1, -12.0).expect("band 1 exists");

        let parts = audio_thread.take_commands();
        assert!(
            parts
                .iter()
                .any(|part| matches!(part, DeckPart::Eq(DeckEqChange::Layout(_)))),
            "the slot is handed the layout: {parts:?}"
        );
        assert!(
            parts.iter().any(|part| matches!(
                part,
                DeckPart::Eq(DeckEqChange::Gain { band: 1, gain }) if *gain == GainDb::from(-12.0)
            )),
            "the cut rides the slot's ring: {parts:?}"
        );
        assert_eq!(player.eq_gain(1), Some(-12.0));
    }
}
