//! Memory budget for the drawing hosts, alone in its own binary.
//!
//! The graphics device counts bytes for the whole process, so any other test
//! drawing beside this one is counted into its readings. That makes the answer
//! a property of the test schedule rather than of the host, which is why this
//! is a binary of its own holding a single test: nothing else runs while it
//! measures, and the two hosts are asked one after the other rather than at
//! once.

use kithara_ui_gallery::{app, capture, cli, fixture};
#[cfg(feature = "masonry")]
use kithara_ui_gallery::{custom, demo, host};

#[cfg(any(target_os = "macos", target_os = "ios"))]
mod checks;
