#![forbid(unsafe_code)]

use kithara_bufpool::HasPool;
use kithara_platform::sync::Arc;

use super::{
    AssetLayout, AssetResource, AssetSource, ResourceKey, validate_path, validate_root,
    validate_source,
};
use crate::{error::AssetsResult, store::AssetStore};

/// A store handle bound to one layout-selected asset root.
#[derive(fieldwork::Fieldwork)]
#[fieldwork(opt_in, get)]
#[derive_where::derive_where(Clone; S: HasPool<u8> + Send + Sync + 'static)]
#[derive(derive_more::Debug)]
pub struct AssetScope<S>
where
    S: HasPool<u8> + Send + Sync + 'static,
{
    #[field(get)]
    asset_root: Arc<str>,
    layout: Arc<dyn AssetLayout>,
    #[field(get)]
    #[debug(skip)]
    store: AssetStore<S>,
}

impl<S> AssetScope<S>
where
    S: HasPool<u8> + Send + Sync + 'static,
{
    pub(crate) fn new(
        store: AssetStore<S>,
        source: &AssetSource,
        layout: Arc<dyn AssetLayout>,
    ) -> AssetsResult<Self> {
        validate_source(source)?;
        let asset_root = layout.root(source);
        validate_root(&asset_root)?;
        Ok(Self {
            layout,
            store,
            asset_root: Arc::from(asset_root),
        })
    }

    /// Delete this asset and every resource below its layout-owned root.
    ///
    /// # Errors
    /// Returns an error when the backing asset cannot be removed.
    pub fn delete_asset(&self) -> AssetsResult<()> {
        self.store.delete_asset(&self.asset_root)
    }

    /// Mint a validated key using the scope's selected layout.
    ///
    /// # Errors
    /// Returns [`crate::AssetsError::InvalidKey`] for hostile layout output.
    pub fn key(&self, resource: &AssetResource) -> AssetsResult<ResourceKey> {
        let path = self.layout.path(resource);
        validate_path(&path)?;
        Ok(ResourceKey::relative(Arc::clone(&self.asset_root), path))
    }
}
