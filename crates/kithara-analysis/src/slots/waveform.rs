use std::num::NonZeroU32;

use kithara_bufpool::{HasPool, PoolError, PoolRegion};
use kithara_waveform::WaveformResume;
use tracing::warn;

use crate::{BlobError, Waveform, analyzer::WaveformPass};

#[derive(Default)]
pub(crate) struct Slot(Option<WaveformPass>);

/// Builds the pass that fills `buckets` at most. The caller decides whether a
/// waveform is wanted at all; reaching here means it is.
impl<S> TryFrom<(usize, NonZeroU32, &PoolRegion<S>)> for Slot
where
    S: HasPool<f32>,
{
    type Error = PoolError;

    fn try_from(
        (buckets, rate, pools): (usize, NonZeroU32, &PoolRegion<S>),
    ) -> Result<Self, Self::Error> {
        WaveformPass::new(rate.get(), buckets, pools).map(|pass| Self(Some(pass)))
    }
}

/// The tag of a waveform built to `buckets`. No bucket ceiling is no waveform,
/// and so no tag.
pub(crate) fn cache_tag(buckets: Option<usize>) -> Option<String> {
    buckets.map(|buckets| format!("wave:native:max{buckets}:v1"))
}

pub(crate) fn push<S>(slot: &mut Slot, pools: &PoolRegion<S>, pcm: &[f32], channels: usize, at: u64)
where
    S: HasPool<f32>,
{
    let failure = slot
        .0
        .as_mut()
        .and_then(|analyzer| analyzer.push(pools, pcm, channels, at).err());
    if let Some(error) = failure {
        warn!(
            ?error,
            "waveform analysis buffer allocation failed; waveform disabled"
        );
        slot.0 = None;
    }
}

pub(crate) fn snapshot(slot: &mut Slot, extent: Option<u64>) -> Option<Waveform> {
    slot.0.as_mut().map(|analyzer| analyzer.snapshot(extent))
}

pub(crate) fn write_resume(slot: &Slot) -> Option<Vec<u8>> {
    slot.0.as_ref().map(|analyzer| {
        let mut out = Vec::new();
        analyzer.write_resume(&mut out);
        out
    })
}

pub(crate) fn restore<S>(
    slot: &mut Slot,
    pools: &PoolRegion<S>,
    resume: Option<WaveformResume>,
) -> Result<(), BlobError>
where
    S: HasPool<f32>,
{
    match (slot.0.as_mut(), resume) {
        (Some(analyzer), Some(resume)) => analyzer.restore(pools, resume),
        (None, None) => Ok(()),
        (Some(_), None) | (None, Some(_)) => Err(BlobError::Corrupt),
    }
}
