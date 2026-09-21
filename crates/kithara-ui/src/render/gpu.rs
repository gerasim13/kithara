//! What the graphics device holds for this process.
//!
//! The renderer's own counters see only the resources it asked for; the bulk
//! of a frame's cost is the command-buffer storage the driver allocates
//! underneath them, which appears in no wgpu report. The device itself does
//! know, and answers in bytes — so a budget on the renderer is expressed
//! against [`allocated_bytes`], not against a resource count.
//!
//! Every backend that cannot answer returns `None`, which a caller reports as
//! "unmeasured" rather than as zero.

/// Bytes the process's graphics device currently holds, when the platform can
/// say.
///
/// `None` means this build has no device to ask — a non-Apple target today,
/// or a machine that offers no Metal device at all.
#[must_use]
pub fn allocated_bytes() -> Option<u64> {
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    {
        kithara_apple::metal::allocated_bytes()
    }
    #[cfg(not(any(target_os = "macos", target_os = "ios")))]
    {
        None
    }
}
