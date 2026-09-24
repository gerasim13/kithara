//! What the process's Metal device holds: the device's own account of the
//! memory it holds for this process.
//!
//! A renderer's counters see only the resources it asked for; the bulk of a
//! frame's cost is the command-buffer storage the driver allocates underneath
//! them, which appears in no renderer report. The device does know, and
//! answers in bytes.

use objc2::{msg_send, rc::Retained, runtime::AnyObject};

unsafe extern "C" {
    /// The process's default Metal device, retained, or null on a machine that
    /// offers none.
    fn MTLCreateSystemDefaultDevice() -> *mut AnyObject;
}

/// Bytes the process's Metal device currently holds.
///
/// `None` on a machine that offers no Metal device at all, which a caller
/// reports as unmeasured rather than as zero.
#[must_use]
pub fn allocated_bytes() -> Option<u64> {
    // SAFETY: `MTLCreateSystemDefaultDevice` returns either null or a device
    // owned by the caller, which `Retained::from_raw` takes over and releases
    // on drop. `currentAllocatedSize` is a property of `MTLDevice` returning
    // `NSUInteger`, read here as `usize`.
    unsafe {
        let device = Retained::from_raw(MTLCreateSystemDefaultDevice())?;
        let bytes: usize = msg_send![&*device, currentAllocatedSize];
        Some(bytes as u64)
    }
}
