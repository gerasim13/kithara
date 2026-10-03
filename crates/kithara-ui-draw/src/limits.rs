use kithara_config::Config;
use kithara_derive::Patch;

/// Memory retained by the draw pools between frames.
#[derive(Config, Clone, Copy, Debug, PartialEq, Eq, Patch)]
#[config(
    default,
    builder(state_mod(vis = "pub")),
    fields(value, builder(default = 128))
)]
#[non_exhaustive]
pub struct DrawPoolLimits {
    /// Command slots retained by one returned draw-list buffer.
    #[config(builder(default = 512))]
    pub command_capacity: usize,
    /// Maximum reusable buffers kept by each pool. Zero is treated as one.
    #[config(builder(default = 64))]
    pub max_buffers: usize,
    /// Hard byte limit shared by every draw buffer kind.
    #[config(builder(default = 64 * 1024 * 1024))]
    pub max_bytes: usize,
    /// Vector verbs retained by one returned path buffer.
    pub path_capacity: usize,
    /// UTF-8 bytes retained by one returned text buffer.
    pub text_capacity: usize,
}
