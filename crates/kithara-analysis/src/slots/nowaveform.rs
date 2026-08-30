use std::num::NonZeroU32;

use kithara_bufpool::{HasPool, PoolError, PoolRegion};

use crate::waveform::bucket::Waveform;

#[derive(Default)]
pub(crate) struct Config;

#[derive(Default)]
pub(crate) struct Slot;

pub(crate) fn build<S>(
    _config: &Config,
    _rate: NonZeroU32,
    _pools: &PoolRegion<S>,
) -> Result<Slot, PoolError>
where
    S: HasPool<f32>,
{
    Ok(Slot)
}

pub(crate) const fn cache_tag(_config: &Config) -> Option<String> {
    None
}

pub(crate) const fn config_is_empty(_config: &Config) -> bool {
    true
}

pub(crate) fn push<S>(
    _slot: &mut Slot,
    _pools: &PoolRegion<S>,
    _pcm: &[f32],
    _channels: usize,
    _at: u64,
) where
    S: HasPool<f32>,
{
}

pub(crate) fn snapshot(_slot: &mut Slot, _extent: Option<u64>) -> Option<Waveform> {
    None
}
