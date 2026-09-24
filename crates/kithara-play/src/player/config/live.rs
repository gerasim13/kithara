use delegate::delegate;
use kithara_events::EventBus;
use kithara_warp::StretchControls;
use tracing::{debug, warn};

use super::PlayerConfig;
use crate::{
    api::{PlayerEvent, SlotId},
    bridge::PlayerCmd,
    error::PlayError,
};

impl<S> PlayerConfig<S> {
    pub(crate) const MIN_PLAYBACK_RATE: f32 = StretchControls::MIN_SPEED;

    pub(crate) fn normalize_live_values(&self) {
        self.default_rate
            .store(self.default_rate().max(Self::MIN_PLAYBACK_RATE));
        self.prefetch_duration
            .store(self.prefetch_duration().max(0.0));
    }

    delegate! {
        to self.auto_advance_enabled {
            #[call(load)]
            pub(crate) fn auto_advance_enabled(&self) -> bool;
            #[call(store)]
            pub(crate) fn set_auto_advance_enabled(&self, enabled: bool);
        }
        to self.crossfade_duration {
            #[call(load)]
            pub(crate) fn crossfade_duration(&self) -> f32;
        }
        to self.default_rate {
            #[call(load)]
            pub(crate) fn default_rate(&self) -> f32;
        }
        to self.muted {
            #[call(load)]
            pub(crate) fn is_muted(&self) -> bool;
        }
        to self.prefetch_duration {
            #[call(load)]
            pub(crate) fn prefetch_duration(&self) -> f32;
        }
        to self.volume {
            #[call(load)]
            pub(crate) fn volume(&self) -> f32;
        }
    }

    pub(crate) fn set_crossfade_duration(
        &self,
        seconds: f32,
        send: impl FnOnce(PlayerCmd) -> Result<(), PlayError>,
    ) {
        let clamped = seconds.max(0.0);
        self.crossfade_duration.store(clamped);
        let _ = send(PlayerCmd::SetFadeDuration(clamped));
    }

    pub(crate) fn set_default_rate(&self, rate: f32) -> f32 {
        let clamped = rate.max(Self::MIN_PLAYBACK_RATE);
        self.default_rate.store(clamped);
        clamped
    }

    pub(crate) fn set_muted(
        &self,
        muted: bool,
        slot: Option<SlotId>,
        set_slot_volume: impl FnOnce(SlotId, f32) -> Result<(), PlayError>,
        bus: &EventBus,
    ) {
        self.muted.store(muted);
        let effective = if muted { 0.0 } else { self.volume() };
        apply_effective_volume(effective, slot, set_slot_volume);
        bus.publish(PlayerEvent::MuteChanged { muted });
    }

    pub(crate) fn set_prefetch_duration(
        &self,
        seconds: f32,
        send: impl FnOnce(PlayerCmd) -> Result<(), PlayError>,
    ) {
        let clamped = seconds.max(0.0);
        self.prefetch_duration.store(clamped);
        let _ = send(PlayerCmd::SetPrefetchDuration(clamped));
    }

    pub(crate) fn set_volume(
        &self,
        volume: f32,
        slot: Option<SlotId>,
        set_slot_volume: impl FnOnce(SlotId, f32) -> Result<(), PlayError>,
        bus: &EventBus,
    ) {
        let clamped = volume.clamp(0.0, 1.0);
        self.volume.store(clamped);
        if !self.is_muted() {
            apply_effective_volume(clamped, slot, set_slot_volume);
        }
        bus.publish(PlayerEvent::VolumeChanged { volume: clamped });
    }
}

fn apply_effective_volume(
    volume: f32,
    slot: Option<SlotId>,
    set_slot_volume: impl FnOnce(SlotId, f32) -> Result<(), PlayError>,
) {
    if let Some(slot_id) = slot {
        debug!(volume, ?slot_id, "applying effective volume to slot");
        if let Err(error) = set_slot_volume(slot_id, volume) {
            warn!(?error, volume, "failed to set slot volume");
        }
    } else {
        debug!(volume, "apply_effective_volume: no slot allocated yet");
    }
}
