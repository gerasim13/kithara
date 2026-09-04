/// Local shim so `#[kithara::mock]` resolves on wasm, which does not
/// depend on the `kithara` facade crate (only on `kithara-test-macros`).
/// On native the real `kithara` crate is in scope and provides the macro,
/// so the shim is wasm-only to avoid an ambiguous-name conflict.
#[cfg(target_arch = "wasm32")]
mod kithara {
    pub(crate) use kithara_test_macros::mock;
}

/// FFI representation of an asset whose resources share one cache root.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Enum))]
pub enum FfiAssetSource {
    Remote {
        url: String,
        discriminator: Option<String>,
    },
    Local {
        path: String,
    },
}

/// FFI representation of one resource within an asset.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Enum))]
pub enum FfiAssetResource {
    /// Direct-file source bytes with their resolved extension.
    Source { extension: String },
    /// A URL-addressed resource such as a playlist, init, segment, or key.
    Url { url: String },
    /// A named derived artifact such as track analysis.
    Named { namespace: String, name: String },
}

/// Domain-scoped query parameters that identify remote media content.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
pub struct FfiCacheIdentityRule {
    /// Exact hosts, `*.domain` subdomain patterns, or `*`.
    pub domains: Vec<String>,
    /// Query parameter names included in the cache identity.
    pub query_parameters: Vec<String>,
}

/// Foreign cache layout callback.
///
/// Implementations must be pure and deterministic, fast, non-blocking,
/// non-throwing, and safe to call from arbitrary background threads. Returned
/// values must not contain query text, credentials, or other secrets.
/// `root` is called once for each store scope being created. `path` is called
/// once for each resource key being minted. Cache operations using that key do
/// not invoke either callback again.
/// Invalid output fails scope or key creation and never falls back to the
/// default layout.
///
/// `root` must return exactly one non-empty component and cannot equal
/// `_index`. `path` must return a non-empty relative path of components
/// separated by `/`; no component may end in `.tmp`. Components are ASCII,
/// at most 96 bytes, never `.` or `..`, do not end in a dot or space, are not
/// Windows device names, and contain neither control bytes nor
/// `< > : " / \ | ? *`. Comparisons for `_index`, `.tmp`, and device names are
/// case-insensitive. The store rejects invalid output instead of rewriting it.
#[kithara::mock(api = FfiAssetLayoutMock)]
#[cfg_attr(feature = "uniffi", uniffi::export(with_foreign))]
pub trait FfiAssetLayout: Send + Sync {
    fn path(&self, resource: FfiAssetResource) -> String;
    fn root(&self, source: FfiAssetSource) -> String;
}
