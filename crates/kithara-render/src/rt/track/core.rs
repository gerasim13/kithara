use std::num::NonZeroU32;

use bon::bon;
use kithara_dsp::param::SmootherConfig;
use kithara_platform::sync::Arc;
use kithara_warp::RenderReader;
use num_traits::cast::{AsPrimitive, ToPrimitive};

use super::{PlayerResource, fade::TrackFade, gate::TrackGate};
use crate::{
    CrossfadeSettings, ServiceClass,
    bridge::{Fade, FadeDir, SlotState},
    consts::DEFAULT_DECLICK,
};

/// The track a mixer slot holds: its consumer, its transport and its envelope.
///
/// A track that is not playing has its gate shut, so it is silent and not read, and holds its
/// position until it is started again.
#[derive(fieldwork::Fieldwork)]
#[fieldwork(opt_in, get)]
pub struct PlayerTrack {
    pub(super) resource: Box<PlayerResource>,
    pub(super) fade: TrackFade,
    pub(super) gate: TrackGate,
    #[field(get, copy)]
    pub(super) state: SlotState,
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
    /// position.
    pub(super) served_media_frames: f64,
    pub(super) sample_rate: u32,
}

#[bon]
impl PlayerTrack {
    /// A stopped track over `resource`: silent and not read until it is started.
    #[builder]
    #[must_use]
    pub fn new(
        #[builder(finish_fn)] resource: Box<PlayerResource>,
        sample_rate: NonZeroU32,
        /// The ramp of the track's start and stop.
        #[builder(default = DEFAULT_DECLICK)]
        declick: SmootherConfig,
    ) -> Self {
        let observed_duration = resource.duration();
        let mut fade = TrackFade::default();
        fade.play(sample_rate);
        let track = Self {
            resource,
            observed_duration,
            fade,
            state: SlotState::Stopped,
            gate: TrackGate::new(false, declick, sample_rate),
            sample_rate: sample_rate.get(),
            served_media_frames: 0.0,
        };
        track.update_service_class();
        track
    }

    fn rate(&self) -> NonZeroU32 {
        NonZeroU32::new(self.sample_rate).unwrap_or(NonZeroU32::MIN)
    }

    /// Let the track sound from the next frame it renders, entering with `fade`.
    pub fn start(&mut self, fade: Fade) {
        let rate = self.rate();
        match fade {
            Fade::Declick => {
                self.fade.play(rate);
                self.gate.steer(true);
            }
            Fade::Crossfade(settings) => {
                self.fade.stop(rate);
                self.fade.fade_in(settings, rate);
                self.gate.steer(true);
                self.gate.snap();
            }
        }
        self.set_state(SlotState::Playing);
    }

    /// Take the track out from the next frame it renders with `fade`; once silent it is not read.
    pub fn stop(&mut self, fade: Fade) {
        match fade {
            Fade::Declick => self.gate.steer(false),
            Fade::Crossfade(settings) => self.fade.fade_out(settings, self.rate()),
        }
        if self.state != SlotState::Playing {
            self.shut();
        }
    }

    /// Ramp the envelope from its gain on the next frame along one half of `settings`.
    pub fn fade(&mut self, settings: CrossfadeSettings, dir: FadeDir) {
        let rate = self.rate();
        match dir {
            FadeDir::In => self.fade.fade_in(settings, rate),
            FadeDir::Out => self.fade.fade_out(settings, rate),
        }
    }

    /// Move the gate to where it is steered at once.
    pub(crate) fn snap_gate(&mut self) {
        self.gate.snap();
    }

    /// Shut the gate at once: the track is silent from the next frame.
    pub(super) fn shut(&mut self) {
        self.gate.steer(false);
        self.gate.snap();
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
            /// Reader of the render this track publishes, when it publishes one.
            #[must_use]
            pub fn render_reader(&self) -> Option<RenderReader>;
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
    }

    /// The envelope's gain on the last mixed frame.
    #[must_use]
    pub fn gain(&self) -> f32 {
        if self.state == SlotState::Playing {
            self.fade.gain()
        } else {
            0.0
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
    /// moves the media clock, while the begin half of the seek happened on the control thread
    /// through [`PlayerResource::seek_handle`]. A track that ended stands stopped at the new
    /// position.
    pub fn seek(&mut self, seconds: f64) {
        self.resource.reset_for_seek();
        let frames = seek_frame_index(seconds, self.sample_rate, self.observed_duration);
        self.served_media_frames = AsPrimitive::as_(frames);
        if self.state == SlotState::Ended {
            self.set_state(SlotState::Stopped);
        }
    }

    /// Propagate a stream sample-rate change to the resource and envelopes.
    pub fn set_host_sample_rate(&mut self, sample_rate: NonZeroU32) {
        self.resource.set_host_sample_rate(sample_rate);
        self.fade.update_sample_rate(sample_rate);
        self.gate.update_sample_rate(sample_rate);
        self.sample_rate = sample_rate.get();
    }

    /// Hand the consumer back, for the receipt that returns it off the audio thread.
    #[must_use]
    pub fn into_resource(self) -> Box<PlayerResource> {
        self.resource
    }

    pub(super) fn set_state(&mut self, state: SlotState) {
        if self.state != state {
            self.state = state;
            self.update_service_class();
        }
    }

    /// Map the track's state to the shared worker's scheduling priority.
    fn update_service_class(&self) {
        self.resource
            .set_service_class(service_class_for_state(self.state));
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

const fn service_class_for_state(state: SlotState) -> ServiceClass {
    match state {
        SlotState::Playing => ServiceClass::Audible,
        SlotState::Stopped => ServiceClass::Warm,
        SlotState::Empty | SlotState::Ended => ServiceClass::Idle,
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
    #[case(SlotState::Playing, ServiceClass::Audible)]
    #[case(SlotState::Stopped, ServiceClass::Warm)]
    #[case(SlotState::Ended, ServiceClass::Idle)]
    fn slot_state_maps_to_service_class(#[case] state: SlotState, #[case] expected: ServiceClass) {
        assert_eq!(service_class_for_state(state), expected);
    }
}
