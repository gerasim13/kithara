use core::{mem, num::NonZeroU32};

use firewheel::{
    StreamInfo,
    channel_config::{ChannelConfig, ChannelCount},
    diff::{Diff, Patch, PatchError},
    dsp::{
        fade::FadeCurve,
        mix::{Mix, MixDSP},
    },
    event::{NodeEventType, ParamData, ProcEvents},
    mask::MaskType,
    node::{
        AudioNode, AudioNodeInfo, AudioNodeProcessor, ConstructProcessorContext, EmptyConfig,
        ProcBuffers, ProcExtra, ProcInfo, ProcStreamCtx, ProcessStatus,
    },
};
use kithara_bufpool::{HasPool, PoolError, SampleBuffer};
use kithara_test_utils::kithara;
use num_traits::cast::AsPrimitive;
use tracing::warn;

use crate::effects::eq::{EqBandConfig, EqConfig, GainDb, IsolatorEq};

#[derive(Diff, Patch, Debug, Clone, Copy, PartialEq)]
pub(crate) struct MasterEqBand {
    pub(crate) frequency: f32,
    pub(crate) gain_db: f32,
    pub(crate) q_factor: f32,
    pub(crate) kind: u8,
}

#[derive(Diff, Debug)]
pub struct MasterEqNode<S> {
    pub(crate) bands: Vec<MasterEqBand>,
    pub(crate) enabled: bool,
    #[diff(skip)]
    config: EqConfig<S>,
}

/// A band layout on its way to the processor. Built on the control thread;
/// leaves the audio thread carrying the retired pair, so nothing is freed there.
pub(super) struct MasterEqLayout {
    bands: Vec<MasterEqBand>,
    equalizers: Option<(IsolatorEq, IsolatorEq)>,
}

/// An opaque runtime parameter patch for [`MasterEqNode`].
pub struct MasterEqNodePatch(MasterEqNodePatchKind);

enum MasterEqNodePatchKind {
    Bands(<Vec<MasterEqBand> as Patch>::Patch),
    Enabled(<bool as Patch>::Patch),
}

impl<S> Patch for MasterEqNode<S> {
    type Patch = MasterEqNodePatch;

    fn apply(&mut self, patch: Self::Patch) {
        match patch.0 {
            MasterEqNodePatchKind::Bands(patch) => self.bands.apply(patch),
            MasterEqNodePatchKind::Enabled(patch) => self.enabled.apply(patch),
        }
    }

    fn patch(data: &ParamData, path: &[u32]) -> Result<Self::Patch, PatchError> {
        match path {
            [0, tail @ ..] => Ok(MasterEqNodePatch(MasterEqNodePatchKind::Bands(<Vec<
                MasterEqBand,
            > as Patch>::patch(
                data, tail,
            )?))),
            [1, tail @ ..] => Ok(MasterEqNodePatch(MasterEqNodePatchKind::Enabled(
                bool::patch(data, tail)?,
            ))),
            _ => Err(PatchError::InvalidPath),
        }
    }
}

impl<S> Clone for MasterEqNode<S> {
    fn clone(&self) -> Self {
        Self {
            bands: self.bands.clone(),
            enabled: self.enabled,
            config: self.config.clone(),
        }
    }
}

impl<S> MasterEqNode<S> {
    #[must_use]
    pub fn new(config: EqConfig<S>, layout: &[EqBandConfig]) -> Self {
        let bands = layout
            .iter()
            .map(|band| MasterEqBand {
                frequency: band.frequency(),
                gain_db: f32::from(band.gain_db()),
                q_factor: band.q_factor(),
                kind: band.kind() as u8,
            })
            .collect();

        Self {
            bands,
            config,
            enabled: true,
        }
    }

    #[must_use]
    pub fn band_count(&self) -> usize {
        self.bands.len()
    }

    /// The event that moves this node's bands into the running processor.
    /// A failed pooled allocation disables the EQ, as construction does today.
    pub fn layout_event(&self, sample_rate: NonZeroU32) -> NodeEventType
    where
        S: HasPool<f32>,
    {
        let equalizers = match build_equalizers(self, sample_rate) {
            Ok(equalizers) => Some(equalizers),
            Err(error) => {
                warn!(%error, "master EQ disabled because its pooled scratch allocation failed");
                None
            }
        };
        NodeEventType::custom(MasterEqLayout {
            bands: self.bands.clone(),
            equalizers,
        })
    }

    pub fn set_gain(&mut self, index: usize, gain_db: GainDb) {
        if let Some(band) = self.bands.get_mut(index) {
            band.gain_db = f32::from(gain_db);
        }
    }
}

impl<S> AudioNode for MasterEqNode<S>
where
    S: HasPool<f32> + Send + Sync + 'static,
{
    type Configuration = EmptyConfig;

    fn construct_processor(
        &self,
        _config: &Self::Configuration,
        cx: ConstructProcessorContext,
    ) -> impl AudioNodeProcessor {
        MasterEqProcessor::new(self.clone(), cx.stream_info)
    }

    fn info(&self, _config: &Self::Configuration) -> AudioNodeInfo {
        AudioNodeInfo::new()
            .debug_name("master_eq")
            .channel_config(ChannelConfig {
                num_inputs: ChannelCount::STEREO,
                num_outputs: ChannelCount::STEREO,
            })
    }
}

struct MasterEqProcessor<S> {
    params: MasterEqNode<S>,
    sample_rate: NonZeroU32,
    active: Option<(IsolatorEq, IsolatorEq)>,
    retiring: Option<(IsolatorEq, IsolatorEq)>,
    crossover: MixDSP,
    retiring_out: (SampleBuffer, SampleBuffer),
}

impl<S> MasterEqProcessor<S>
where
    S: HasPool<f32>,
{
    fn new(params: MasterEqNode<S>, stream_info: &StreamInfo) -> Self {
        let frames = stream_info.max_block_frames.get().as_();
        let resources = build_equalizers(&params, stream_info.sample_rate).and_then(|equalizers| {
            let left = params.config.pools().get_with_len::<f32>(frames)?;
            let right = params.config.pools().get_with_len::<f32>(frames)?;
            Ok((equalizers, (left, right)))
        });
        let (active, retiring_out) = match resources {
            Ok((equalizers, retiring_out)) => (Some(equalizers), retiring_out),
            Err(error) => {
                warn!(%error, "master EQ disabled because its pooled scratch allocation failed");
                (
                    None,
                    (
                        params.config.pools().get::<f32>(),
                        params.config.pools().get::<f32>(),
                    ),
                )
            }
        };
        let crossover = MixDSP::new(
            Mix::FULLY_WET,
            FadeCurve::Linear,
            params.config.smoothing(),
            stream_info.sample_rate,
        );

        Self {
            params,
            active,
            retiring: None,
            crossover,
            retiring_out,
            sample_rate: stream_info.sample_rate,
        }
    }

    fn sync_gains(&mut self) {
        for (i, band) in self.params.bands.iter().enumerate() {
            if let Some((left, right)) = self.active.as_mut() {
                left.set_gain(i, GainDb::from(band.gain_db));
                right.set_gain(i, GainDb::from(band.gain_db));
            }
        }
    }

    /// Swap the arriving layout in and the retired pair out. A layout that
    /// lands during a crossover retires the fading-in pair and restarts it.
    fn take_layout(&mut self, layout: &mut MasterEqLayout) {
        mem::swap(&mut self.params.bands, &mut layout.bands);
        let incoming = layout.equalizers.take();
        layout.equalizers = self.retiring.take();
        self.retiring = self.active.take();
        self.active = incoming;
        self.crossover.set_mix(Mix::FULLY_DRY, FadeCurve::Linear);
        self.crossover.reset_to_target();
        self.crossover.set_mix(Mix::FULLY_WET, FadeCurve::Linear);
    }
}

fn build_equalizers<S: HasPool<f32>>(
    params: &MasterEqNode<S>,
    sample_rate: NonZeroU32,
) -> Result<(IsolatorEq, IsolatorEq), PoolError> {
    let bands = bands_from_params(params);
    let left = IsolatorEq::new(&params.config, &bands, sample_rate.get())?;
    let right = IsolatorEq::new(&params.config, &bands, sample_rate.get())?;
    Ok((left, right))
}

fn bands_from_params<S>(params: &MasterEqNode<S>) -> Vec<EqBandConfig> {
    params
        .bands
        .iter()
        .map(|b| {
            EqBandConfig::builder()
                .frequency(b.frequency)
                .q_factor(b.q_factor)
                .gain_db(GainDb::from(b.gain_db))
                .kind(b.kind.into())
                .build()
        })
        .collect()
}

impl<S> AudioNodeProcessor for MasterEqProcessor<S>
where
    S: HasPool<f32> + Send + Sync + 'static,
{
    fn new_stream(&mut self, stream_info: &StreamInfo, _context: &mut ProcStreamCtx) {
        self.sample_rate = stream_info.sample_rate;
        if let Some((left, right)) = self.active.as_mut() {
            left.update_sample_rate(self.sample_rate.get());
            right.update_sample_rate(self.sample_rate.get());
        }
        if let Some((left, right)) = self.retiring.as_mut() {
            left.update_sample_rate(self.sample_rate.get());
            right.update_sample_rate(self.sample_rate.get());
        }
        self.crossover.update_sample_rate(self.sample_rate);
    }

    #[kithara::rtsan_forbid_blocking]
    fn process(
        &mut self,
        info: &ProcInfo,
        buffers: ProcBuffers,
        events: &mut ProcEvents,
        _extra: &mut ProcExtra,
    ) -> ProcessStatus {
        /// Minimum stereo channel count for processing.
        const MIN_STEREO: usize = 2;
        let mut dirty = false;
        for mut event in events.drain() {
            if let Some(layout) = event.downcast_mut::<MasterEqLayout>() {
                self.take_layout(layout);
            } else if let Some(patch) = MasterEqNode::<S>::patch_event(&event) {
                self.params.apply(patch);
            } else {
                continue;
            }
            dirty = true;
        }
        if dirty {
            self.sync_gains();
        }

        if buffers.inputs.len() < MIN_STEREO || buffers.outputs.len() < MIN_STEREO {
            return ProcessStatus::Bypass;
        }

        if !self.params.enabled
            || self.active.is_none()
            || info.in_silence_mask.all_channels_silent(MIN_STEREO)
        {
            buffers.outputs[0].copy_from_slice(buffers.inputs[0]);
            buffers.outputs[1].copy_from_slice(buffers.inputs[1]);
            return ProcessStatus::OutputsModifiedWithMask(MaskType::Silence(info.in_silence_mask));
        }

        let in_l = &buffers.inputs[0][..info.frames];
        let in_r = &buffers.inputs[1][..info.frames];
        let Some((out_l_slice, out_r_slice_slice)) = buffers.outputs.split_first_mut() else {
            return ProcessStatus::Bypass;
        };
        let Some(out_r_slice) = out_r_slice_slice.first_mut() else {
            return ProcessStatus::Bypass;
        };
        let out_l = &mut out_l_slice[..info.frames];
        let out_r = &mut out_r_slice[..info.frames];

        let (retiring_l_out, retiring_r_out) = (
            &mut self.retiring_out.0[..info.frames],
            &mut self.retiring_out.1[..info.frames],
        );
        let Some((active_l, active_r)) = self.active.as_mut() else {
            return ProcessStatus::Bypass;
        };
        let fading = !self.crossover.has_settled();
        if let (true, Some((retiring_l, retiring_r))) = (fading, self.retiring.as_mut()) {
            for frame in 0..info.frames {
                retiring_l_out[frame] = retiring_l.process_sample(in_l[frame]);
                retiring_r_out[frame] = retiring_r.process_sample(in_r[frame]);
            }
        }
        for frame in 0..info.frames {
            out_l[frame] = active_l.process_sample(in_l[frame]);
            out_r[frame] = active_r.process_sample(in_r[frame]);
        }
        if fading {
            self.crossover.mix_dry_into_wet_stereo(
                retiring_l_out,
                retiring_r_out,
                out_l,
                out_r,
                info.frames,
            );
        }

        ProcessStatus::OutputsModified
    }
}
