use std::collections::BTreeSet;

use bon::Builder;

#[derive(Builder, Clone, Debug)]
#[non_exhaustive]
pub struct Limits {
    #[builder(default = 256 * 1024)]
    pub max_bytes: usize,
    #[builder(default = 8)]
    pub max_depth: usize,
    #[builder(default = 10_000)]
    pub max_nodes: usize,
}

/// Memory retained by the draw pools between frames.
#[derive(Builder, Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct DrawPoolLimits {
    /// Maximum reusable buffers kept by each pool. Zero is treated as one.
    #[builder(default = 64)]
    pub max_buffers: usize,
    /// Command slots retained by one returned draw-list buffer.
    #[builder(default = 512)]
    pub command_capacity: usize,
    /// Vector verbs retained by one returned path buffer.
    #[builder(default = 128)]
    pub path_capacity: usize,
    /// UTF-8 bytes retained by one returned text buffer.
    #[builder(default = 128)]
    pub text_capacity: usize,
}

impl Default for DrawPoolLimits {
    fn default() -> Self {
        Self::builder().build()
    }
}

impl Default for Limits {
    fn default() -> Self {
        Self::builder().build()
    }
}

/// Canonical compile configuration and its resource limits.
#[derive(Builder, Clone, Debug)]
#[builder(state_mod(vis = "pub"))]
#[non_exhaustive]
pub struct UiConfig {
    /// The extension kinds the application registers with its hosts.
    ///
    /// A document naming a `Custom` kind absent from this set is refused while
    /// it compiles, so no host is ever handed an extension it cannot mount.
    #[builder(default)]
    pub custom_kinds: BTreeSet<String>,
    #[builder(default)]
    pub limits: Limits,
    #[builder(default = 64 * 1024)]
    pub max_arena_bytes: usize,
    #[builder(default)]
    pub draw_pools: DrawPoolLimits,
}

impl Default for UiConfig {
    fn default() -> Self {
        Self::builder().build()
    }
}
