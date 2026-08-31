use std::path::Path;

use kithara_bufpool::{HasPool, PoolRegion};
use kithara_platform::{sync::Arc, time::Duration};

use super::{contract::ProcessCtx, reader::ProcessedReader, writer::ProcessedWriter};
use crate::{
    decorator::{Assets, Capabilities},
    error::AssetsResult,
    layout::ResourceKey,
    resource::{AcquisitionResult, AssetResourceState, RequestIdentity},
};

/// Applies optional resource processing to another asset store.
pub struct ProcessingAssets<A, S>
where
    A: Assets,
{
    inner: Arc<A>,
    pools: PoolRegion<S>,
    /// `AssetStore::builder(pools).processing_chunk_size(..)`, unset when the
    /// caller left the processing layer's own default in place.
    chunk_size: Option<usize>,
    /// `AssetStore::builder(pools).processing_gate_poll_interval(..)`, unset when
    /// the caller left the processing layer's own default in place.
    gate_poll_interval: Option<Duration>,
}

impl<A, S> Clone for ProcessingAssets<A, S>
where
    A: Assets,
{
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
            pools: self.pools.clone(),
            chunk_size: self.chunk_size,
            gate_poll_interval: self.gate_poll_interval,
        }
    }
}

impl<A, S> ProcessingAssets<A, S>
where
    A: Assets,
    S: HasPool<u8>,
{
    pub fn new(
        inner: Arc<A>,
        pools: PoolRegion<S>,
        chunk_size: Option<usize>,
        gate_poll_interval: Option<Duration>,
    ) -> Self {
        Self {
            inner,
            pools,
            chunk_size,
            gate_poll_interval,
        }
    }

    fn wrap_ready(
        &self,
        inner: A::ReadyRes,
        processor: Option<ProcessCtx>,
    ) -> ProcessedReader<A::ReadyRes, S> {
        ProcessedReader::wrap_ready()
            .inner(inner)
            .maybe_processor(processor)
            .pools(self.pools.clone())
            .maybe_chunk_size(self.chunk_size)
            .maybe_gate_poll_interval(self.gate_poll_interval)
            .call()
    }
}

impl<A, S> Assets for ProcessingAssets<A, S>
where
    A: Assets,
    S: HasPool<u8> + Send + Sync + 'static,
{
    type ActiveRes = ProcessedWriter<A::ActiveRes, S>;
    type Context = ProcessCtx;
    type IndexRes = A::IndexRes;
    type ReadyRes = ProcessedReader<A::ReadyRes, S>;

    fn acquire_resource_with_ctx(
        &self,
        key: &ResourceKey,
        identity: Option<&RequestIdentity>,
        ctx: Option<Self::Context>,
    ) -> AssetsResult<AcquisitionResult<Self::ActiveRes, Self::ReadyRes>> {
        match self.inner.acquire_resource(key, identity)? {
            AcquisitionResult::Pending(writer) => Ok(AcquisitionResult::Pending(
                ProcessedWriter::builder()
                    .inner(writer)
                    .maybe_processor(ctx)
                    .pools(self.pools.clone())
                    .maybe_chunk_size(self.chunk_size)
                    .maybe_gate_poll_interval(self.gate_poll_interval)
                    .build(),
            )),
            AcquisitionResult::Ready(reader) => {
                Ok(AcquisitionResult::Ready(self.wrap_ready(reader, ctx)))
            }
        }
    }

    fn open_resource_with_ctx(
        &self,
        key: &ResourceKey,
        identity: Option<&RequestIdentity>,
        ctx: Option<Self::Context>,
    ) -> AssetsResult<Self::ReadyRes> {
        Ok(self.wrap_ready(self.inner.open_resource(key, identity)?, ctx))
    }

    delegate::delegate! {
        to self.inner {
            fn capabilities(&self) -> Capabilities;
            fn root_dir(&self) -> &Path;
            fn open_pins_index_resource(&self) -> AssetsResult<Self::IndexRes>;
            fn open_lru_index_resource(&self) -> AssetsResult<Self::IndexRes>;
            fn resource_state(&self, key: &ResourceKey) -> AssetsResult<AssetResourceState>;
            fn delete_asset(&self, asset_root: &str) -> AssetsResult<()>;
            fn remove_resource(&self, key: &ResourceKey) -> AssetsResult<()>;
        }
    }
}
