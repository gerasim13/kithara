use kithara_assets::{AssetResource, AssetScope, AssetSource, AssetStore, ResourceKey};
use kithara_decode::DecodeError;
use kithara_events::EventBus;
use kithara_file::File;
use kithara_hls::Hls;
use kithara_platform::CancelToken;

use super::{ResourceConfig, ResourceSrc, SourceType};

impl<B: Default> ResourceConfig<B> {
    /// Returns the logical asset bound to the store's protocol-selected layout.
    ///
    /// # Errors
    ///
    /// Returns an error when source detection or layout validation fails.
    pub fn asset_scope(&self) -> Result<AssetScope, DecodeError> {
        let source_type = SourceType::detect(&self.src)?;
        let source = self.asset_source_for(&source_type);
        self.asset_scope_for(&source_type, &source)
    }

    /// Mint a layout-owned key for a playback or derived resource.
    ///
    /// # Errors
    ///
    /// Returns an error when source detection or layout validation fails.
    pub fn asset_key(&self, resource: &AssetResource) -> Result<ResourceKey, DecodeError> {
        let scope = self.asset_scope()?;
        scope.key(resource).map_err(DecodeError::backend)
    }

    fn asset_scope_for(
        &self,
        source_type: &SourceType,
        source: &AssetSource,
    ) -> Result<AssetScope, DecodeError> {
        match source_type {
            SourceType::RemoteFile(_) | SourceType::LocalFile(_) => {
                self.store.scope::<File>(source)
            }
            SourceType::HlsStream(_) => self.store.scope::<Hls>(source),
        }
        .map_err(DecodeError::backend)
    }

    fn asset_source_for(&self, source_type: &SourceType) -> AssetSource {
        match source_type {
            SourceType::RemoteFile(url) | SourceType::HlsStream(url) => AssetSource::Remote {
                discriminator: self.discriminator.clone(),
                url: url.clone(),
            },
            SourceType::LocalFile(path) => AssetSource::Local { path: path.clone() },
        }
    }

    /// Event bus attached to this resource, when one was configured.
    #[must_use]
    pub const fn bus(&self) -> Option<&EventBus> {
        self.bus.as_ref()
    }

    /// Per-track parent cancel token, when one was configured.
    #[must_use]
    pub const fn cancel(&self) -> Option<&CancelToken> {
        self.cancel.as_ref()
    }

    /// Optional cache discriminator.
    #[must_use]
    pub fn discriminator(&self) -> Option<&str> {
        self.discriminator.as_deref()
    }

    /// Preferred peak bitrate cap for normal networks.
    #[must_use]
    pub const fn preferred_peak_bitrate(&self) -> f64 {
        self.preferred_peak_bitrate
    }

    /// Replace the event bus attached to this resource.
    pub fn set_bus(&mut self, bus: EventBus) {
        self.bus = Some(bus);
    }

    /// Replace the parent cancel token for this resource.
    pub fn set_cancel(&mut self, cancel: CancelToken) {
        self.cancel = Some(cancel);
    }

    /// Source parsed for this resource.
    #[must_use]
    pub const fn source(&self) -> &ResourceSrc {
        &self.src
    }

    /// Shared asset store for this resource.
    #[must_use]
    pub const fn store(&self) -> &AssetStore {
        &self.store
    }
}

#[cfg(test)]
mod tests {
    use kithara_assets::{
        AssetLayout, AssetLayoutRegistry, AssetResource, AssetSource, AssetStore,
    };
    use kithara_bufpool::{BytePool, PcmPool};
    use kithara_file::File;
    use kithara_hls::Hls;
    use kithara_platform::sync::Arc;
    use kithara_test_utils::kithara;

    use super::ResourceConfig;

    #[derive(Debug)]
    struct FixedLayout(&'static str);

    impl AssetLayout for FixedLayout {
        fn path(&self, _resource: &AssetResource) -> String {
            "resource".to_owned()
        }

        fn root(&self, _source: &AssetSource) -> String {
            self.0.to_owned()
        }
    }

    fn config(source: &str, store: AssetStore) -> ResourceConfig {
        let src = ResourceConfig::parse_src(source).expect("invariant: fixture source is valid");
        ResourceConfig::for_src(src)
            .store(store)
            .byte_pool(BytePool::default())
            .pcm_pool(PcmPool::default())
            .build()
    }

    #[kithara::test]
    fn resource_config_uses_the_protocol_selected_asset_layout() {
        let layouts = AssetLayoutRegistry::default()
            .with::<File>(Arc::new(FixedLayout("file-root")))
            .with::<Hls>(Arc::new(FixedLayout("hls-root")));
        let store = AssetStore::builder().layouts(layouts).build();
        let file = config("https://example.com/track.flac", store.clone());
        let hls = config("https://example.com/track.m3u8", store);

        assert_eq!(
            file.asset_scope()
                .expect("invariant: file scope is valid")
                .asset_root(),
            "file-root"
        );
        assert_eq!(
            hls.asset_scope()
                .expect("invariant: HLS scope is valid")
                .asset_root(),
            "hls-root"
        );
    }
}
