use std::collections::BTreeSet;

use bon::Builder;

#[cfg(any(feature = "render", feature = "vello"))]
use crate::draw::DrawBuffers;

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
    /// Hard byte limit shared by every draw buffer kind.
    #[builder(default = 64 * 1024 * 1024)]
    pub max_bytes: usize,
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

/// Compiled screens one host keeps while a document turns between its pages.
///
/// Measured against the gallery's own pages: seven of them cost more than two
/// milliseconds to compile, which is a hitch on every return visit, and eight
/// covers every page a document offers today without a package of hundreds
/// growing the cache without bound.
pub const SCREEN_CACHE: usize = 8;

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
    /// Compiled screens a host keeps while a document turns between its pages.
    ///
    /// A page is compiled when it is first shown, and kept so that turning
    /// back to it costs nothing. The screen being shown counts as one of
    /// these, so a depth of one keeps no page a host has left.
    #[builder(default = SCREEN_CACHE)]
    pub screen_cache: usize,
    /// The pools every document compiled against this configuration draws
    /// from.
    ///
    /// Shared on purpose. A host compiles one screen per layout and compiles
    /// them all again whenever the skin changes; a pool family per compiled
    /// document would keep as many sets of retained buffers as there are
    /// pages, and throw every one of them away at each redress. One family,
    /// cloned into each compiled document, is what makes a retained buffer
    /// retained. Build the configuration once and compile every screen against
    /// it; the default builds a family of its own, which is one host drawing
    /// one page.
    #[cfg(any(feature = "render", feature = "vello"))]
    #[builder(default)]
    pub draw_buffers: DrawBuffers,
}

impl Default for UiConfig {
    fn default() -> Self {
        Self::builder().build()
    }
}
