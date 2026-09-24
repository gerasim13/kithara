use crate::context::BuildContext;

pub(crate) enum AssetBuild {
    Ready(Vec<u8>),
    /// Only a fetching family can fail to produce bytes.
    #[cfg(feature = "remote")]
    Unavailable(String),
}

/// One registered asset case, submitted by `#[kithara::asset]`.
pub(crate) struct AssetDef {
    /// Accessor names that must be materialized before this asset.
    pub(crate) dependencies: &'static [&'static str],
    /// Environment variables that invalidate this producer.
    pub(crate) env: &'static [&'static str],
    /// Case name from `#[case::name(...)]`.
    pub(crate) case: &'static str,
    /// MIME type served with the asset.
    pub(crate) content_type: &'static str,
    /// File extension inside the store.
    pub(crate) ext: &'static str,
    /// Writes a sample of the output format the case is stored in. Its digest
    /// joins the case id, so a format change re-addresses every stored case.
    pub(crate) format: Option<fn() -> Vec<u8>>,
    /// Generator function name.
    pub(crate) func: &'static str,
    /// Bake the bytes into filesystem-free wasm binaries.
    pub(crate) embed: bool,
    /// Keep the build green when the producer reports unavailable.
    pub(crate) optional: bool,
    /// Produces the asset's bytes.
    pub(crate) build: for<'a> fn(BuildContext<'a>, &'a [&'a [u8]]) -> AssetBuild,
}

inventory::collect!(AssetDef);

impl AssetDef {
    /// Accessor name: `{func}_{case}`. Unique across the whole registry — every
    /// accessor lands in one flat module.
    pub(crate) fn accessor_name(&self) -> String {
        format!("{}_{}", self.func, self.case)
    }
}
