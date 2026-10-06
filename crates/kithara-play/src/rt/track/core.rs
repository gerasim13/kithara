use std::num::NonZeroU32;

use bon::bon;
use kithara_dsp::param::SmootherConfig;
use kithara_events::TrackId;
use kithara_platform::sync::Arc;
use kithara_warp::RenderReader;
use num_traits::cast::{AsPrimitive, ToPrimitive};

use super::{PlayerResource, fade::TrackFade, gate::TrackGate};
use crate::{CrossfadeSettings, bridge::TrackState, consts::DEFAULT_DECLICK, worker::ServiceClass};

/// Per-track state in the processor arena.
///
/// Manages the `MixDSP` fade, track state, cached position/duration,
/// and notification logic for a single loaded track.
#[derive(fieldwork::Fieldwork)]
#[fieldwork(opt_in, get)]
pub struct PlayerTrack {
    pub(super) resource: Box<PlayerResource>,
    pub(super) fade: TrackFade,
    pub(super) gate: TrackGate,
    #[field(get, copy)]
    pub(super) item_id: TrackId,
    #[field(get, copy)]
    pub(super) state: TrackState,
    /// The track that starts on the frame after this one's last.
    #[field(get, copy)]
    pub(super) successor: Option<TrackId>,
    /// The epoch this track leads under: from the `FadeIn` that leads it, or the `Chain` that
    /// stitches it in behind another.
    #[field(get, copy)]
    pub(super) epoch: u64,
    /// Set only when the track reaches *natural* EOF (`handle_natural_end`).
    /// Marks a played-out track as eligible to be kept warm at end-of-queue
    /// and revived by a later in-range seek (Superpowered-style resume).
    /// Cleared by `seek`/`play`. A `Finished` state from `stop()` or a
    /// faded-out crossfade leaves this `false`, so those are discarded as usual.
    #[field(get)]
    pub(super) ended_at_eof: bool,
    pub(super) state_dirty: bool,
    /// Last observed duration snapshot.
    ///
    /// Mirrors `PlayerResource::duration()` (post-gapless-trim, visible
    /// duration) captured under the resource lock.
    pub(super) observed_duration: f64,
    /// Cumulative *media* frames this track has served into the mix output,
    /// scaled by the resource's current effective playback rate.
    ///
    /// The source of truth for the published position, so it reflects what
    /// has been rendered to the audio output, not the decoder's pre-buffered
    /// position (which can be ~200 ms ahead of the mixer thanks to
    /// `PlayerResource`'s scratch buffer).
    pub(super) served_media_frames: f64,
    pub(super) sample_rate: u32,
    /// Slot seek epoch this track has been re-based onto.
    ///
    /// The control thread publishes the next epoch before it sends the matching
    /// `DeckPart::Seek`, so a render block that sees a newer published epoch is
    /// rendering a position the user has already left. [`read`](Self::read) uses the
    /// gap to refuse natural-EOF finalization until the re-base arrives.
    pub(super) seek_epoch: u64,
}

#[bon]
impl PlayerTrack {
    /// Create a new track in the `Preloading` state.
    ///
    /// The `MixDSP` starts at `FULLY_WET` (silent) so that an explicit
    /// `fade_in()` or `play()` is required to produce audio.
    #[builder]
    #[must_use]
    pub fn new(
        #[builder(finish_fn)] resource: Box<PlayerResource>,
        sample_rate: NonZeroU32,
        item_id: TrackId,
        /// Slot seek epoch already published when this track loaded — a track
        /// planted after earlier seeks starts level with them, not behind.
        #[builder(default)]
        seek_epoch: u64,
        /// The ramp of the track's start and stop.
        #[builder(default = DEFAULT_DECLICK)]
        declick: SmootherConfig,
        /// Whether the track is held stopped: it stays silent through a `play()` or a fade until
        /// it is started.
        #[builder(default)]
        stopped: bool,
    ) -> Self {
        let observed_duration = resource.duration();
        let track = Self {
            resource,
            item_id,
            observed_duration,
            seek_epoch,
            state: TrackState::Preloading,
            state_dirty: false,
            successor: None,
            epoch: 0,
            fade: TrackFade::default(),
            gate: TrackGate::new(!stopped, declick, sample_rate),
            sample_rate: sample_rate.get(),
            served_media_frames: 0.0,
            ended_at_eof: false,
        };
        track.update_service_class(TrackState::Preloading);
        track
    }

    /// Start a fade-in: transitions to `FadingIn`, targets `FULLY_DRY` (audible).
    pub fn fade_in(&mut self, settings: CrossfadeSettings) {
        self.set_state(TrackState::FadingIn);
        let sample_rate = NonZeroU32::new(self.sample_rate).unwrap_or(NonZeroU32::MIN);
        self.fade.fade_in(settings, sample_rate);
    }

    /// Start a fade-out: transitions to `FadingOut`, targets `FULLY_WET` (silent).
    pub fn fade_out(&mut self, settings: CrossfadeSettings) {
        self.set_state(TrackState::FadingOut);
        let sample_rate = NonZeroU32::new(self.sample_rate).unwrap_or(NonZeroU32::MIN);
        self.fade.fade_out(settings, sample_rate);
    }

    /// Make `successor` the track that starts on the frame after this one's last.
    pub const fn chain(&mut self, successor: TrackId) {
        self.successor = Some(successor);
    }

    /// Lead under `epoch` from the next time this track leads.
    pub const fn lead_under(&mut self, epoch: u64) {
        self.epoch = epoch;
    }

    /// Re-base this track onto a slot seek epoch the processor has applied.
    ///
    /// Every loaded track observes the epoch, not just the ones a seek moves:
    /// a track the seek left alone must still stop counting as behind, or its
    /// natural end would never finalize.
    pub const fn observe_seek_epoch(&mut self, epoch: u64) {
        self.seek_epoch = epoch;
    }

    /// Instantly start playing at full volume.
    pub fn play(&mut self) {
        self.set_state(TrackState::Playing);
        let sample_rate = NonZeroU32::new(self.sample_rate).unwrap_or(NonZeroU32::MIN);
        self.fade.play(sample_rate);
        self.ended_at_eof = false;
    }

    /// Let the track sound from the next frame it renders, ramped in from silence; a track that
    /// was not playing plays from where it stands.
    pub fn start(&mut self) {
        if !self.state.is_playing() {
            self.steer_gate(false);
            self.play();
        }
        self.steer_gate(true);
    }

    /// Ramp the track out from the next frame it renders. Once silent it is not read, so it holds
    /// its position until it is started again.
    pub fn stop(&mut self) {
        self.steer_gate(false);
    }

    /// Ramp the track in or out from the next frame it renders; a track that does not play moves
    /// at once, since nothing of it sounds.
    pub(crate) fn steer_gate(&mut self, started: bool) {
        self.gate.steer(started);
        if !self.state.is_playing() {
            self.gate.snap();
        }
    }

    delegate::delegate! {
        to self.resource {
            /// Cached span in seconds: how much of the source is on disk.
            #[must_use]
            pub fn cached_span(&self) -> f64;
            /// Decoded-ahead frontier in seconds.
            #[must_use]
            pub fn decoded_frontier(&self) -> f64;
            /// Current visible (post-gapless-trim) duration in seconds.
            #[must_use]
            #[expr(observed_duration(self.observed_duration, $))]
            pub fn duration(&self) -> f64;
            /// Control-plane handle used to begin this track's seeks off the audio thread.
            #[must_use]
            pub fn seek_handle(&self) -> Option<Arc<dyn kithara_audio::SeekBegin>>;
            pub(crate) fn render_reader(&self) -> Option<RenderReader>;
            /// Source identifier.
            #[must_use]
            pub fn src(&self) -> &Arc<str>;
            /// Effective media seconds consumed per output second.
            #[must_use]
            pub(crate) fn playback_rate(&self) -> f32;
            /// Apply a playback-rate target directly to this track's Warp controls.
            #[call(apply_playback_rate)]
            pub fn set_playback_rate(&mut self, rate: f32);
        }
        to self.gate {
            /// Move the track's ramp to where it is steered at once.
            #[call(snap)]
            pub(crate) fn snap_gate(&mut self);
            /// Whether the track has ramped out: it is silent and not read.
            #[call(is_shut)]
            pub(crate) fn is_stopped(&self) -> bool;
        }
    }

    /// Current media position in seconds.
    ///
    /// Tracks `served_media_frames / sample_rate` — i.e. what has actually
    /// been mixed into the output, on the media clock — so the value matches
    /// `duration` instead of the decoder's pre-buffered position.
    #[must_use]
    pub fn position(&self) -> f64 {
        let sample_rate = self.sample_rate.max(1);
        self.served_media_frames / f64::from(sample_rate)
    }

    /// Re-base the track on a seek the control thread already begun.
    ///
    /// Lock-free, so it is safe from the audio callback: it drops what the feeder buffered and
    /// moves the media clock, while the begin half of the seek — the epoch, the event, the wakes —
    /// happened on the control thread through [`PlayerResource::seek_handle`].
    pub fn seek(&mut self, seconds: f64) {
        self.resource.reset_for_seek();
        let frames = seek_frame_index(seconds, self.sample_rate, self.observed_duration);
        self.served_media_frames = AsPrimitive::as_(frames);
        self.ended_at_eof = false;
    }

    /// Propagate a stream sample-rate change to the resource and fade.
    pub fn set_host_sample_rate(&mut self, sample_rate: NonZeroU32) {
        self.resource.set_host_sample_rate(sample_rate);
        self.fade.update_sample_rate(sample_rate);
        self.gate.update_sample_rate(sample_rate);
        self.sample_rate = sample_rate.get();
    }

    /// Set the track state and mark as dirty.
    ///
    /// Also updates the shared worker's scheduling priority via
    /// [`ServiceClass`] bridge: Audible tracks get highest priority.
    pub(super) fn set_state(&mut self, new_state: TrackState) {
        if self.state != new_state {
            self.state = new_state;
            self.state_dirty = true;
            self.update_service_class(new_state);
        }
    }

    /// Instantly finish (silent, finished state).
    pub fn finish(&mut self) {
        self.set_state(TrackState::Finished);
        let sample_rate = NonZeroU32::new(self.sample_rate).unwrap_or(NonZeroU32::MIN);
        self.fade.stop(sample_rate);
    }

    /// Map track state to worker scheduling priority and push the update.
    fn update_service_class(&self, state: TrackState) {
        self.resource
            .set_service_class(service_class_for_state(state));
    }
}

fn observed_duration(observed: f64, resource: f64) -> f64 {
    if observed > 0.0 { observed } else { resource }
}

fn seek_frame_index(seconds: f64, sample_rate: u32, duration: f64) -> u64 {
    let sample_rate = sample_rate.max(1);
    let target_seconds = if seconds.is_nan() {
        0.0
    } else if seconds.is_finite() {
        seconds.max(0.0)
    } else if seconds.is_sign_positive() {
        duration.max(0.0)
    } else {
        0.0
    };
    let bounded_seconds = if duration > 0.0 {
        target_seconds.min(duration)
    } else {
        target_seconds
    };
    let frames = bounded_seconds * f64::from(sample_rate);
    ToPrimitive::to_u64(&frames).unwrap_or(0)
}

const fn service_class_for_state(state: TrackState) -> ServiceClass {
    match state {
        TrackState::Playing | TrackState::FadingIn | TrackState::FadingOut => ServiceClass::Audible,
        TrackState::Preloading => ServiceClass::Warm,
        TrackState::Finished => ServiceClass::Idle,
    }
}

#[cfg(test)]
mod tests {
    use kithara_test_utils::kithara;

    use super::*;

    #[kithara::test]
    fn seek_frame_index_clamps_unrepresentable_targets() {
        assert_eq!(seek_frame_index(f64::INFINITY, 44_100, 10.0), 441_000);
        assert_eq!(seek_frame_index(f64::INFINITY, 44_100, 0.0), 0);
        assert_eq!(seek_frame_index(f64::NAN, 44_100, 10.0), 0);
    }

    #[kithara::test]
    fn track_state_is_playing() {
        assert!(TrackState::Playing.is_playing());
        assert!(TrackState::FadingIn.is_playing());
        assert!(TrackState::FadingOut.is_playing());
        assert!(!TrackState::Preloading.is_playing());
        assert!(!TrackState::Finished.is_playing());
    }

    #[kithara::test]
    fn track_state_is_leading() {
        assert!(TrackState::Playing.is_leading());
        assert!(TrackState::FadingIn.is_leading());
        assert!(!TrackState::FadingOut.is_leading());
        assert!(!TrackState::Preloading.is_leading());
        assert!(!TrackState::Finished.is_leading());
    }

    #[kithara::test]
    #[case(TrackState::Playing, ServiceClass::Audible)]
    #[case(TrackState::FadingIn, ServiceClass::Audible)]
    #[case(TrackState::FadingOut, ServiceClass::Audible)]
    #[case(TrackState::Preloading, ServiceClass::Warm)]
    #[case(TrackState::Finished, ServiceClass::Idle)]
    fn track_state_maps_to_service_class(
        #[case] state: TrackState,
        #[case] expected: ServiceClass,
    ) {
        let class = match state {
            TrackState::Playing | TrackState::FadingIn | TrackState::FadingOut => {
                ServiceClass::Audible
            }
            TrackState::Preloading => ServiceClass::Warm,
            TrackState::Finished => ServiceClass::Idle,
        };
        assert_eq!(class, expected);
    }
}
