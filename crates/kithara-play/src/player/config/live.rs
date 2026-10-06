use delegate::delegate;
use kithara_config::LiveConfig;
use kithara_events::EventBus;
use kithara_render::bridge::{DeckMixSettings, DeckMixSettingsChange, DeckPart};
use kithara_signal::FaderValue;
use kithara_warp::MIN_SPEED;

use super::PlayerConfig;
use crate::{api::PlayerEvent, error::PlayError};

impl<S> PlayerConfig<S> {
    pub(crate) const MIN_PLAYBACK_RATE: f32 = MIN_SPEED;

    pub(crate) fn normalize_live_values(&self) {
        self.default_rate
            .store(self.default_rate().max(Self::MIN_PLAYBACK_RATE));
    }

    delegate! {
        to self.crossfade_duration {
            #[call(load)]
            pub(crate) fn crossfade_duration(&self) -> f32;
        }
        to self.default_rate {
            #[call(load)]
            pub(crate) fn default_rate(&self) -> f32;
        }
        to self.level {
            #[call(load)]
            pub(crate) fn level(&self) -> f32;
        }
        to self.muted {
            #[call(load)]
            pub(crate) fn is_muted(&self) -> bool;
        }
        to self.volume {
            #[call(load)]
            pub(crate) fn volume(&self) -> f32;
        }
    }

    pub(crate) fn set_crossfade_duration(&self, seconds: f32) {
        self.crossfade_duration.store(seconds.max(0.0));
    }

    pub(crate) fn set_default_rate(&self, rate: f32) -> f32 {
        let clamped = rate.max(Self::MIN_PLAYBACK_RATE);
        self.default_rate.store(clamped);
        clamped
    }

    /// Sends the deck its mix level and keeps it; an idle player keeps it for its next slot.
    pub(crate) fn set_level(
        &self,
        level: f32,
        send: impl FnOnce(DeckPart) -> Result<(), PlayError>,
    ) -> Result<(), PlayError> {
        let change = DeckMixSettings::check(DeckMixSettingsChange::Level(level))?;
        match send(DeckPart::Mix(change)) {
            Ok(()) | Err(PlayError::NoActiveSlot) => {}
            Err(error) => return Err(error),
        }
        self.level.store(level);
        Ok(())
    }

    pub(crate) fn set_muted(
        &self,
        muted: bool,
        send: impl FnOnce(DeckPart) -> Result<(), PlayError>,
        bus: &EventBus,
    ) -> Result<(), PlayError> {
        match send(DeckPart::Mix(DeckMixSettingsChange::Muted(muted))) {
            Ok(()) | Err(PlayError::NoActiveSlot) => {}
            Err(error) => return Err(error),
        }
        self.muted.store(muted);
        bus.publish(PlayerEvent::MuteChanged { muted });
        Ok(())
    }

    pub(crate) fn set_volume(
        &self,
        volume: f32,
        send: impl FnOnce(DeckPart) -> Result<(), PlayError>,
        bus: &EventBus,
    ) -> Result<(), PlayError> {
        let clamped = volume.clamp(0.0, 1.0);
        match send(DeckPart::Mix(DeckMixSettingsChange::Volume(
            FaderValue::from(clamped),
        ))) {
            Ok(()) | Err(PlayError::NoActiveSlot) => {}
            Err(error) => return Err(error),
        }
        self.volume.store(clamped);
        bus.publish(PlayerEvent::VolumeChanged { volume: clamped });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use kithara_config::Config as _;
    use kithara_events::SlotId;
    use kithara_test_utils::kithara;

    use super::*;
    use crate::{
        PlayWorker, PlayWorkerConfig, mock,
        test_pools::{TestPools, pools},
    };

    fn config() -> PlayerConfig<TestPools> {
        PlayerConfig::builder()
            .sample_rate(mock::SAMPLE_RATE)
            .worker(PlayWorker::new(PlayWorkerConfig::builder(pools()).build()))
            .build()
    }

    #[kithara::test]
    fn a_refused_mix_part_leaves_volume_and_mute_unchanged() {
        let config = config();
        let bus = EventBus::new(8);
        let mut events = bus.subscribe::<PlayerEvent>();
        let rejected = |_: DeckPart| {
            Err(PlayError::SlotChannelFull {
                slot: SlotId::new(1),
            })
        };

        assert!(matches!(
            config.set_volume(0.4, rejected, &bus),
            Err(PlayError::SlotChannelFull { .. })
        ));
        assert_eq!(config.values().volume, 1.0);

        assert!(matches!(
            config.set_muted(true, rejected, &bus),
            Err(PlayError::SlotChannelFull { .. })
        ));
        assert!(!config.values().muted);
        assert!(events.try_recv().is_err());

        config
            .set_volume(0.4, |_| Err(PlayError::NoActiveSlot), &bus)
            .expect("an idle player retains its next-slot volume");
        assert_eq!(config.values().volume, 0.4);
        assert!(matches!(
            events.try_recv().map(|event| event.event),
            Ok(PlayerEvent::VolumeChanged { volume }) if volume == 0.4
        ));
    }

    #[kithara::test]
    #[case::not_a_number(f32::NAN)]
    #[case::over_unity(1.5)]
    #[case::below_silence(-0.25)]
    fn a_refused_mix_level_never_reaches_the_deck(#[case] level: f32) {
        let config = config();
        let mut sent = false;

        let refused = config.set_level(level, |_| {
            sent = true;
            Ok(())
        });

        assert!(matches!(refused, Err(PlayError::MixLevel { .. })));
        assert!(!sent, "a refused level is never sent to the deck");
        assert_eq!(config.values().level, 1.0);
    }

    #[kithara::test]
    fn an_idle_player_keeps_its_level_for_the_next_slot() {
        let config = config();

        config
            .set_level(0.5, |_| Err(PlayError::NoActiveSlot))
            .expect("an idle player retains its next-slot level");

        assert_eq!(config.values().level, 0.5);
    }
}
