use core::num::NonZeroU32;

use firewheel::{
    StreamInfo,
    channel_config::{ChannelConfig, ChannelCount},
    diff::{Diff, Patch, PatchError},
    event::{ParamData, ProcEvents},
    mask::MaskType,
    node::{
        AudioNode, AudioNodeInfo, AudioNodeProcessor, ConstructProcessorContext, EmptyConfig,
        ProcBuffers, ProcExtra, ProcInfo, ProcStreamCtx, ProcessStatus,
    },
};
use kithara_bufpool::HasPool;
use kithara_test_utils::kithara;
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
        MasterEqProcessor::new(self.clone(), cx.stream_info.sample_rate)
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
    eq_l: Option<IsolatorEq>,
    eq_r: Option<IsolatorEq>,
}

impl<S> MasterEqProcessor<S>
where
    S: HasPool<f32>,
{
    fn new(params: MasterEqNode<S>, sample_rate: NonZeroU32) -> Self {
        let bands = bands_from_params(&params);
        let equalizers =
            IsolatorEq::new(&params.config, &bands, sample_rate.get()).and_then(|left| {
                IsolatorEq::new(&params.config, &bands, sample_rate.get())
                    .map(|right| (left, right))
            });
        let (mut eq_l, mut eq_r) = match equalizers {
            Ok(equalizers) => (Some(equalizers.0), Some(equalizers.1)),
            Err(error) => {
                warn!(%error, "master EQ disabled because its pooled scratch allocation failed");
                (None, None)
            }
        };

        for (i, band) in params.bands.iter().enumerate() {
            if let Some(eq) = eq_l.as_mut() {
                eq.set_gain(i, GainDb::from(band.gain_db));
            }
            if let Some(eq) = eq_r.as_mut() {
                eq.set_gain(i, GainDb::from(band.gain_db));
            }
        }

        Self {
            params,
            sample_rate,
            eq_l,
            eq_r,
        }
    }

    fn sync_gains(&mut self) {
        for (i, band) in self.params.bands.iter().enumerate() {
            if let Some(eq) = self.eq_l.as_mut() {
                eq.set_gain(i, GainDb::from(band.gain_db));
            }
            if let Some(eq) = self.eq_r.as_mut() {
                eq.set_gain(i, GainDb::from(band.gain_db));
            }
        }
    }
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
        if let Some(eq) = self.eq_l.as_mut() {
            eq.update_sample_rate(self.sample_rate.get());
        }
        if let Some(eq) = self.eq_r.as_mut() {
            eq.update_sample_rate(self.sample_rate.get());
        }
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
        for patch in events.drain_patches::<MasterEqNode<S>>() {
            self.params.apply(patch);
            dirty = true;
        }
        if dirty {
            self.sync_gains();
        }

        if buffers.inputs.len() < MIN_STEREO || buffers.outputs.len() < MIN_STEREO {
            return ProcessStatus::Bypass;
        }

        if !self.params.enabled
            || self.eq_l.is_none()
            || self.eq_r.is_none()
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

        let (Some(eq_l), Some(eq_r)) = (self.eq_l.as_mut(), self.eq_r.as_mut()) else {
            return ProcessStatus::Bypass;
        };
        for frame in 0..info.frames {
            out_l[frame] = eq_l.process_sample(in_l[frame]);
            out_r[frame] = eq_r.process_sample(in_r[frame]);
        }

        ProcessStatus::OutputsModified
    }
}
