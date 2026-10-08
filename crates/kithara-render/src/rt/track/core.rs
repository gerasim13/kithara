use std::num::NonZeroU32;

use bon::bon;
use kithara_dsp::param::SmootherConfig;
use kithara_platform::sync::Arc;
use kithara_signal::{SegmentId, SessionFrame};

use super::{PlayerResource, fade::TrackFade, gate::TrackGate};
use crate::{
    CrossfadeCurve, CrossfadeSettings,
    bridge::{Fade, FadeDir, SlotMark, SlotState},
    consts::DEFAULT_DECLICK,
};

/// A slot's packet consumer and data-driven envelope.
pub struct PlayerTrack {
    pub(super) resource: Box<PlayerResource>,
    pub(super) fade: TrackFade,
    pub(super) gate: TrackGate,
    pub(super) state: SlotState,
    pub(super) gap: u32,
    pub(super) stop_at: Option<SessionFrame>,
    pub(super) stop_resume: Option<SlotMark>,
    sample_rate: NonZeroU32,
    declick: SmootherConfig,
}

#[bon]
impl PlayerTrack {
    #[builder]
    #[must_use]
    pub fn new(
        #[builder(finish_fn)] mut resource: Box<PlayerResource>,
        sample_rate: NonZeroU32,
        #[builder(default = DEFAULT_DECLICK)] declick: SmootherConfig,
        #[builder(default = SegmentId::FIRST)] segment: SegmentId,
    ) -> Self {
        resource.select_segment(segment);
        Self {
            resource,
            fade: TrackFade::default(),
            gate: TrackGate::new(false, declick, sample_rate),
            state: SlotState::Stopped,
            gap: 0,
            stop_at: None,
            stop_resume: None,
            sample_rate,
            declick,
        }
    }

    #[must_use]
    pub const fn state(&self) -> SlotState {
        self.state
    }

    #[must_use]
    pub fn segment(&self) -> SegmentId {
        self.resource.segment()
    }

    #[must_use]
    pub fn mark(&self, session: SessionFrame) -> Option<SlotMark> {
        self.resource.mark(session)
    }

    #[must_use]
    pub fn position(&self) -> f64 {
        self.resource.position().as_secs_f64()
    }

    #[must_use]
    pub fn gain(&self) -> f32 {
        if self.state == SlotState::Playing {
            self.fade.gain()
        } else {
            0.0
        }
    }

    delegate::delegate! {
        to self.resource {
            #[must_use]
            pub fn duration(&self) -> f64;
            #[must_use]
            pub fn decoded_frontier(&self) -> f64;
            #[must_use]
            pub fn cached_span(&self) -> f64;
            #[must_use]
            pub fn src(&self) -> &Arc<str>;
        }
    }

    fn settings(&self, fade: Fade) -> CrossfadeSettings {
        match fade {
            Fade::Declick => CrossfadeSettings {
                duration: self.declick.smooth_seconds.max(0.0),
                curve: CrossfadeCurve::Linear,
                depth: 0.0,
                position: 0.5,
            },
            Fade::Crossfade(settings) => settings,
        }
    }

    pub fn start(&mut self, fade: Fade) {
        self.fade.fade_in(self.settings(fade), self.sample_rate);
        if self.fade.remaining() == 0 {
            self.fade.play(self.sample_rate);
        }
        self.gate.steer(true);
        self.gate.snap();
        self.state = SlotState::Playing;
        self.gap = 0;
        self.resource.set_playing(true);
    }

    pub(crate) fn stop(&mut self, fade: Fade, at: SessionFrame) {
        self.gap = 0;
        self.stop_resume = None;
        if self.state == SlotState::Playing {
            self.fade.fade_out(self.settings(fade), self.sample_rate);
            let frames = i64::try_from(self.fade.remaining()).unwrap_or(i64::MAX);
            self.stop_at = Some(SessionFrame::new(i64::from(at).saturating_add(frames)));
            if self.fade.remaining() > 0 {
                return;
            }
        } else {
            self.stop_at = Some(at);
        }
        self.settle_stop();
    }

    pub fn fade(&mut self, settings: CrossfadeSettings, dir: FadeDir) {
        match dir {
            FadeDir::In => self.fade.fade_in(settings, self.sample_rate),
            FadeDir::Out => self.fade.fade_out(settings, self.sample_rate),
        }
        if dir == FadeDir::Out && self.fade.remaining() == 0 {
            self.settle_stop();
        }
    }

    pub(crate) fn adopt(&mut self, segment: SegmentId) {
        let playing = self.state == SlotState::Playing || self.state == SlotState::Ended;
        self.resource.select_segment(segment);
        self.gap = 0;
        if playing {
            self.fade.stop(self.sample_rate);
            self.start(Fade::Declick);
        }
    }

    pub(crate) fn recycle_obsolete(&mut self, budget: &mut usize) {
        self.resource.refresh_mark(budget);
        if self.state == SlotState::Stopped && self.stop_at.is_some() {
            self.settle_stop();
        }
    }

    pub(crate) fn stop_resume(&self) -> Option<SlotMark> {
        self.stop_resume
    }

    pub(crate) fn clear_stop(&mut self) {
        self.stop_at = None;
        self.stop_resume = None;
    }

    pub(crate) fn interrupt_stop(&mut self) -> Option<SlotMark> {
        let resume = self.stop_resume;
        self.clear_stop();
        resume
    }

    pub(super) fn settle_stop(&mut self) {
        self.state = SlotState::Stopped;
        self.shut();
        self.fade.stop(self.sample_rate);
        if let Some(at) = self.stop_at {
            self.stop_resume = self.resource.mark(at);
        }
    }

    pub(crate) fn snap_gate(&mut self) {
        self.gate.snap();
    }

    pub(in crate::rt) fn shut(&mut self) {
        self.gate.steer(false);
        self.gate.snap();
        self.resource.set_playing(false);
    }

    pub fn set_host_sample_rate(&mut self, sample_rate: NonZeroU32) {
        self.fade.update_sample_rate(sample_rate);
        self.gate.update_sample_rate(sample_rate);
        self.sample_rate = sample_rate;
    }

    #[must_use]
    pub fn into_resource(mut self) -> Box<PlayerResource> {
        self.resource.set_playing(false);
        self.resource
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
