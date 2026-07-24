use kithara_assets::{AssetStore, AssetsError, ProcessCtx, ResourceAcquisition, ResourceKey};
use kithara_platform::CancelToken;
use kithara_stream::MediaInfo;

pub(crate) struct EngineIdentity {
    pub(crate) key: ResourceKey,
    pub(crate) store: AssetStore,
    pub(crate) process: Option<ProcessCtx>,
    pub(crate) cancel: CancelToken,
}

impl EngineIdentity {
    pub(crate) fn acquire(&self) -> Result<ResourceAcquisition, AssetsError> {
        self.store
            .acquire_resource_with_ctx(&self.key, None, self.process.clone())
    }
}

pub(crate) trait LifecycleSink: Send + Sync {
    fn metadata_resolved(&self, info: Option<MediaInfo>);
    fn total_bytes_resolved(&self, total_bytes: u64);
    fn complete(&self, final_len: Option<u64>);
    fn error(&self, reason: &str);
}
