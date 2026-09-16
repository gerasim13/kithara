use std::{num::NonZeroU32, ops::Range};

use firewheel::{
    dsp::{
        fade::FadeCurve,
        filter::smoothing_filter::DEFAULT_SETTLE_EPSILON,
        mix::{Mix, MixDSP},
    },
    param::smoother::SmootherConfig,
};

#[derive(fieldwork::Fieldwork)]
#[fieldwork(opt_in, get)]
pub(super) struct TrackFade {
    curve: FadeCurve,
    mix: MixDSP,
    #[field(get, vis = "pub(super)")]
    duration: f32,
    next_duration: f32,
}

impl TrackFade {
    pub(super) fn new(duration: f32, curve: FadeCurve, sample_rate: NonZeroU32) -> Self {
        Self {
            curve,
            duration,
            next_duration: duration,
            mix: MixDSP::new(
                Mix::FULLY_WET,
                curve,
                Self::smoother_config(duration),
                sample_rate,
            ),
        }
    }

    pub(super) fn fade_in(&mut self, sample_rate: NonZeroU32) {
        if self.mix.has_settled() {
            self.rebuild_if_latched(Mix::FULLY_WET, sample_rate);
        }
        self.mix.set_mix(Mix::FULLY_DRY, self.curve);
    }

    pub(super) fn fade_out(&mut self, sample_rate: NonZeroU32) {
        if self.mix.has_settled() {
            self.rebuild_if_latched(Mix::FULLY_DRY, sample_rate);
        }
        self.mix.set_mix(Mix::FULLY_WET, self.curve);
    }

    pub(super) fn mix_range(
        &mut self,
        scratch_bufs: &mut [&mut [f32]],
        mix_bufs: &mut [&mut [f32]],
        range: Range<usize>,
        frames: usize,
    ) {
        const MIN_STEREO_CHANNELS: usize = 2;
        if scratch_bufs.len() < MIN_STEREO_CHANNELS || mix_bufs.len() < MIN_STEREO_CHANNELS {
            return;
        }

        let (output_l_slice, output_r_slice) = mix_bufs.split_at_mut(1);
        let output_l = &mut output_l_slice[0][range.clone()];
        let output_r = &mut output_r_slice[0][range.clone()];

        self.mix.mix_dry_into_wet_stereo(
            &scratch_bufs[0][range.clone()],
            &scratch_bufs[1][range],
            output_l,
            output_r,
            frames,
        );
    }

    pub(super) fn play(&mut self, sample_rate: NonZeroU32) {
        let settled = self.mix.has_settled();
        if settled {
            self.rebuild_if_latched(Mix::FULLY_DRY, sample_rate);
        }
        self.mix.set_mix(Mix::FULLY_DRY, self.curve);
        if settled {
            self.mix.reset_to_target();
        }
    }

    const fn smoother_config(duration: f32) -> SmootherConfig {
        SmootherConfig {
            smooth_seconds: duration,
            settle_epsilon: DEFAULT_SETTLE_EPSILON,
        }
    }

    pub(super) fn stop(&mut self, sample_rate: NonZeroU32) {
        if self.mix.has_settled() {
            self.rebuild_if_latched(Mix::FULLY_WET, sample_rate);
        }
        self.mix.set_mix(Mix::FULLY_WET, self.curve);
        self.mix.reset_to_target();
    }

    /// The next fade uses `duration`; a running fade keeps its own.
    pub(super) fn set_next_duration(&mut self, duration: f32) {
        self.next_duration = duration;
    }

    fn rebuild_if_latched(&mut self, target: Mix, sample_rate: NonZeroU32) {
        if (self.next_duration - self.duration).abs() < f32::EPSILON {
            return;
        }
        self.duration = self.next_duration;
        self.mix = MixDSP::new(
            target,
            self.curve,
            Self::smoother_config(self.duration),
            sample_rate,
        );
    }

    delegate::delegate! {
        to self.mix {
            pub(super) fn has_settled(&self) -> bool;
            pub(super) fn update_sample_rate(&mut self, sample_rate: NonZeroU32);
        }
    }
}
