use kithara_config::LiveConfig;
use kithara_events::TrackId;
use kithara_platform::sync::{Arc, atomic::Ordering};
use ringbuf::traits::Producer;

use super::{
    processor::Deck,
    track::{PlayerResource, PlayerTrack},
};
use crate::{
    CrossfadeSettings,
    bridge::{DeckApplied, DeckPart, PlayerNotification, TrackState, TrackTransition},
};

impl Deck {
    fn apply_rate(&mut self, rate: f32) {
        self.rate = rate;
        for (_, track) in self.tracks.iter_mut() {
            track.set_playback_rate(rate);
        }
    }

    /// Releases the natural-end hold on every loaded track in the slot, including ones this seek
    /// does not move, since the re-base is slot-wide.
    fn apply_seek(&mut self, seconds: f64, seek_epoch: u64) {
        if seek_epoch != self.playback.seek_epoch.load(Ordering::SeqCst) {
            return;
        }

        let mut revived = false;
        for (_, track) in self.tracks.iter_mut() {
            track.observe_seek_epoch(seek_epoch);
            match track.state() {
                TrackState::FadingIn => {
                    track.seek(seconds);
                }
                TrackState::Playing => {
                    track.seek(seconds);
                    track.play();
                }
                TrackState::FadingOut => {
                    track.finish();
                }
                TrackState::Finished if track.ended_at_eof() && seconds < track.duration() => {
                    track.seek(seconds);
                    track.play();
                    revived = true;
                }
                _ => {}
            }
        }
        if revived {
            self.set_playing(true);
        }
    }

    /// Starts or stops every held track with the deck's transport; tracks attached later follow
    /// it.
    pub(super) fn set_playing(&mut self, playing: bool) {
        self.playback.playing.store(playing, Ordering::SeqCst);
        for (_, track) in self.tracks.iter_mut() {
            track.steer_gate(playing);
        }
    }

    fn clear_all_tracks(&mut self) {
        for slot in self.tracks.slots() {
            self.unload_slot(slot);
        }
        self.set_playing(false);
        self.playback.position.store(0.0);
        self.playback.frontier.store(0.0);
        self.playback.cached.store(0.0);
        self.playback.duration.store(0.0);
    }

    /// Applies one part of a due batch, noting in `applied` what the batch reports.
    pub(super) fn apply(&mut self, part: DeckPart, applied: &mut DeckApplied) {
        match part {
            DeckPart::Attach { resource, item_id } => {
                self.load_track(resource, item_id);
            }
            DeckPart::Detach { item_id } => {
                if let Some(slot) = self.tracks.slot_of(item_id) {
                    self.unload_slot(slot);
                }
            }
            DeckPart::Withdraw { item_id } => {
                if self
                    .tracks
                    .get(item_id)
                    .is_some_and(|track| track.state() == TrackState::Preloading)
                    && let Some(slot) = self.tracks.slot_of(item_id)
                {
                    self.unload_slot(slot);
                }
            }
            DeckPart::Chain { from, to, epoch } => {
                if let Some(track) = self.tracks.get_mut(to) {
                    track.lead_under(epoch);
                }
                if let Some(track) = self.tracks.get_mut(from) {
                    track.chain(to);
                }
            }
            DeckPart::Clear => {
                self.clear_all_tracks();
            }
            DeckPart::Fade(TrackTransition::FadeIn {
                item_id,
                settings,
                epoch,
            }) => {
                self.lead(item_id, settings, epoch);
            }
            DeckPart::Fade(TrackTransition::FadeOut { item_id, settings }) => {
                if let Some(track) = self.tracks.get_mut(item_id) {
                    track.fade_out(settings);
                }
            }
            DeckPart::Seek {
                seconds,
                seek_epoch,
            } => {
                self.apply_seek(seconds, seek_epoch);
            }
            DeckPart::Start { item_id } => {
                if let Some(track) = self.tracks.get_mut(item_id) {
                    track.start();
                }
            }
            DeckPart::Stop { item_id } => {
                if let Some(track) = self.tracks.get_mut(item_id) {
                    applied.stopped_at = Some(track.position());
                    track.stop();
                }
            }
            DeckPart::StartAll => {
                self.set_playing(true);
            }
            DeckPart::StopAll => {
                self.set_playing(false);
            }
            DeckPart::Mix(change) => {
                self.mix.apply_change(change);
                self.render.set_gain(self.mix.gain());
            }
            DeckPart::SetRate(rate) => {
                self.apply_rate(rate);
            }
        }
    }

    /// Makes `item_id` leading: the track that led fades out as it fades in. A fade-in for a
    /// track the deck does not hold changes nothing.
    fn lead(&mut self, item_id: TrackId, settings: CrossfadeSettings, epoch: u64) {
        let Some(slot) = self.tracks.slot_of(item_id) else {
            return;
        };
        let old = self
            .tracks
            .iter()
            .find_map(|(_, track)| track.state().is_leading().then(|| track.item_id()))
            .filter(|old| *old != item_id);
        if let Some(track) = old.and_then(|old| self.tracks.get_mut(old)) {
            track.fade_out(settings);
        }
        if let Some(track) = self.tracks.at_mut(slot) {
            track.fade_in(settings);
            track.lead_under(epoch);
            self.playback
                .adopt(epoch, track.position(), track.duration());
            if old.is_some() {
                self.notif_tx
                    .try_push(PlayerNotification::Changed {
                        src: Arc::clone(track.src()),
                    })
                    .ok();
            }
        }
    }

    fn load_track(&mut self, resource: Box<PlayerResource>, item_id: TrackId) {
        let src = Arc::clone(resource.src());
        if let Some(slot) = self.tracks.slot_of(item_id) {
            self.unload_slot(slot);
        }
        self.evict_tracks_if_needed();

        resource.set_host_sample_rate(self.sample_rate);

        let mut track = PlayerTrack::builder()
            .sample_rate(self.sample_rate)
            .item_id(item_id)
            .seek_epoch(self.playback.seek_epoch.load(Ordering::SeqCst))
            .declick(self.declick)
            .stopped(!self.playback.playing.load(Ordering::SeqCst))
            .build(resource);
        track.set_playback_rate(self.rate);

        if let Some(rejected) = self.tracks.insert(track) {
            self.discard_track(rejected);
            return;
        }

        self.notif_tx
            .try_push(PlayerNotification::Loaded { src })
            .ok();
    }
}
