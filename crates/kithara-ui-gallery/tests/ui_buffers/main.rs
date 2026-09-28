//! What the gallery asks of the renderer's buffers, alone in its own binary.
//!
//! It draws, so it cannot share a binary with the memory budget beside it: the
//! graphics device counts bytes for the whole process, and two drawing tests in
//! one process read each other's allocations.

use kithara_ui_gallery::{capture, custom, demo, fixture, host};

mod checks;
