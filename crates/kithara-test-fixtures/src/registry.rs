/// One registered asset case, submitted by `#[kithara::asset]`.
pub(crate) struct AssetDef {
    /// Case name from `#[case::name(...)]`.
    pub(crate) case: &'static str,
    /// MIME type served with the asset.
    pub(crate) content_type: &'static str,
    /// File extension inside the store.
    pub(crate) ext: &'static str,
    /// Generator function name.
    pub(crate) func: &'static str,
    /// Bake the bytes into the binary instead of reading them from the store.
    pub(crate) embed: bool,
    /// Produces the asset's bytes.
    pub(crate) build: fn() -> Vec<u8>,
}

inventory::collect!(AssetDef);

impl AssetDef {
    /// Accessor name: `{func}_{case}`. Unique across the whole registry — every
    /// accessor lands in one flat module.
    pub(crate) fn accessor_name(&self) -> String {
        format!("{}_{}", self.func, self.case)
    }
}
