use kithara_config::LiveConfig;
use kithara_events::TrackId;
use kithara_platform::sync::{Arc, atomic::Ordering};
use ringbuf::traits::Producer;

use super::{
    processor::Deck,
    track::{PlayerResource, PlayerTrack},
};
use crate::bridge::{DeckPart, PlayerNotification, TrackState, TrackTransition};

impl Deck {
    fn apply_fade_duration(&mut self, duration: f32) {
        self.crossfade.duration = duration;
    }

    fn apply_prefetch_duration(&mut self, duration: f32) {
        self.prefetch_duration = duration.max(0.0);
        for (_, track) in self.tracks.iter_mut() {
            track.set_prefetch_duration(self.prefetch_duration);
        }
    }

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
                    track.stop();
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
            self.playback.playing.store(true, Ordering::SeqCst);
        }
    }

    fn clear_all_tracks(&mut self) {
        for slot in self.tracks.slots() {
            self.unload_slot(slot);
        }
        self.tracks_transitions.clear();
        self.playback.playing.store(false, Ordering::SeqCst);
        self.playback.position.store(0.0);
        self.playback.frontier.store(0.0);
        self.playback.cached.store(0.0);
        self.playback.duration.store(0.0);
    }

    /// Applies one part of a due batch.
    pub(super) fn apply(&mut self, part: DeckPart) {
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
            DeckPart::Clear => {
                self.clear_all_tracks();
            }
            DeckPart::Fade(transition) => {
                self.handle_transition(transition);
            }
            DeckPart::Seek {
                seconds,
                seek_epoch,
            } => {
                self.apply_seek(seconds, seek_epoch);
            }
            DeckPart::Start => {
                self.playback.playing.store(true, Ordering::SeqCst);
            }
            DeckPart::Stop => {
                self.playback.playing.store(false, Ordering::SeqCst);
            }
            DeckPart::Mix(change) => {
                self.mix.apply_change(change);
                self.render.set_gain(self.mix.gain());
            }
            DeckPart::SetFadeDuration(duration) => {
                self.apply_fade_duration(duration);
            }
            DeckPart::SetPrefetchDuration(duration) => {
                self.apply_prefetch_duration(duration);
            }
            DeckPart::SetRate(rate) => {
                self.apply_rate(rate);
            }
        }
    }

    fn handle_transition(&mut self, transition: TrackTransition) {
        let mut leading_changed = false;

        if let TrackTransition::FadeIn {
            item_id, settings, ..
        } = &transition
        {
            self.tracks_transitions.clear();

            let maybe_old = self
                .tracks
                .iter()
                .find_map(|(_, track)| track.state().is_leading().then(|| track.item_id()));

            if let Some(old_id) = maybe_old
                && old_id != *item_id
            {
                leading_changed = true;
                self.tracks_transitions.push_back(TrackTransition::FadeOut {
                    item_id: old_id,
                    settings: *settings,
                });
            }
        }

        self.tracks_transitions.push_back(transition);
        let playback = Arc::clone(&self.playback);
        let mut changed_src = None;
        self.tracks_transitions.retain(|transition| {
            let item_id = match transition {
                TrackTransition::FadeIn { item_id, .. }
                | TrackTransition::FadeOut { item_id, .. } => *item_id,
            };
            if let Some(track) = self.tracks.get_mut(item_id) {
                match transition {
                    TrackTransition::FadeIn {
                        settings, epoch, ..
                    } => {
                        changed_src = Some(Arc::clone(track.src()));
                        if track.position() > Self::FADE_IN_SEEK_THRESHOLD {
                            track.seek(0.0);
                        }
                        track.fade_in(*settings);
                        playback.adopt(*epoch, track.position(), track.duration());
                    }
                    TrackTransition::FadeOut { settings, .. } => {
                        track.fade_out(*settings);
                    }
                }
                return false;
            }
            true
        });

        if leading_changed && let Some(new_src) = changed_src {
            self.notif_tx
                .try_push(PlayerNotification::Changed { src: new_src })
                .ok();
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
            .crossfade(self.crossfade)
            .prefetch_duration(self.prefetch_duration)
            .seek_epoch(self.playback.seek_epoch.load(Ordering::SeqCst))
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
