use kithara::{
    assets::AssetLayout,
    platform::sync::Arc,
    play::policy::{QueryIdentityLayout, QueryIdentityRule},
};

use crate::{
    layout::{FfiAssetLayout, FfiCacheIdentityRule},
    native::layout::NativeLayout,
};

/// Create a Rust-owned query-aware layout.
///
/// Register the returned layout through the ordinary asset-layout registry
/// for each playback protocol that should use it.
#[must_use]
#[cfg_attr(feature = "uniffi", uniffi::export)]
pub fn query_identity_layout(rules: Vec<FfiCacheIdentityRule>) -> Arc<dyn FfiAssetLayout> {
    let rules = rules
        .into_iter()
        .map(|rule| QueryIdentityRule::new(rule.domains, rule.query_parameters));
    let layout = Arc::new(QueryIdentityLayout::new(rules)) as Arc<dyn AssetLayout>;
    Arc::new(NativeLayout::new(layout))
}
