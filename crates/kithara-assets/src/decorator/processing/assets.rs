use std::path::Path;

use kithara_bufpool::BytePool;
use kithara_platform::{sync::Arc, time::Duration};

use super::{contract::ProcessCtx, reader::ProcessedReader, writer::ProcessedWriter};
use crate::{
    decorator::{Assets, Capabilities},
    error::AssetsResult,
    layout::ResourceKey,
    resource::{AcquisitionResult, AssetResourceState, RequestIdentity},
};

/// Applies optional resource processing to another asset store.
#[derive(Clone)]
pub struct ProcessingAssets<A>
where
    A: Assets,
{
    inner: Arc<A>,
    pool: BytePool,
    /// Mirrors `AssetStore::builder().processing_chunk_size(..)`.
    chunk_size: usize,
    /// Mirrors `AssetStore::builder().processing_gate_poll_interval(..)`.
    gate_poll_interval: Duration,
}

impl<A> ProcessingAssets<A>
where
    A: Assets,
{
    pub const fn new(
        inner: Arc<A>,
        pool: BytePool,
        chunk_size: usize,
        gate_poll_interval: Duration,
    ) -> Self {
        Self {
            inner,
            pool,
            chunk_size,
            gate_poll_interval,
        }
    }

    fn wrap_ready(
        &self,
        inner: A::ReadyRes,
        processor: Option<ProcessCtx>,
    ) -> ProcessedReader<A::ReadyRes> {
        ProcessedReader::wrap_ready(
            inner,
            processor,
            self.pool.clone(),
            self.chunk_size,
            self.gate_poll_interval,
        )
    }
}

impl<A> Assets for ProcessingAssets<A>
where
    A: Assets,
{
    type ActiveRes = ProcessedWriter<A::ActiveRes>;
    type Context = ProcessCtx;
    type IndexRes = A::IndexRes;
    type ReadyRes = ProcessedReader<A::ReadyRes>;

    fn acquire_resource_with_ctx(
        &self,
        key: &ResourceKey,
        identity: Option<&RequestIdentity>,
        ctx: Option<Self::Context>,
    ) -> AssetsResult<AcquisitionResult<Self::ActiveRes, Self::ReadyRes>> {
        match self.inner.acquire_resource(key, identity)? {
            AcquisitionResult::Pending(writer) => {
                Ok(AcquisitionResult::Pending(ProcessedWriter::new(
                    writer,
                    ctx,
                    self.pool.clone(),
                    self.chunk_size,
                    self.gate_poll_interval,
                )))
            }
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
