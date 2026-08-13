#![forbid(unsafe_code)]
#![expect(
    clippy::unwrap_used,
    reason = "integration test crate - unwraps are acceptable in test code"
)]
//! Manual final-PCM acceptance matrix for SYNC and no-SYNC playback.
//!
//! The suite is excluded from the default workspace gate. Run the built-in
//! synthetic and repository-media rows with:
//!
//! ```text
//! just test sync-acceptance
//! ```
//!
//! Library rows additionally require `KITHARA_SYNC_LIBRARY` and remain
//! individually ignored so an absent local collection cannot look green. The
//! workflow includes them automatically when that environment variable is set.

#[cfg(not(target_arch = "wasm32"))]
#[path = "kithara_queue/sync_behavioral_matrix.rs"]
mod sync_behavioral_matrix;
#[cfg(not(target_arch = "wasm32"))]
#[path = "kithara_queue/sync_latency.rs"]
mod sync_latency;
#[cfg(not(target_arch = "wasm32"))]
#[path = "kithara_queue/sync_library.rs"]
mod sync_library;
#[cfg(not(target_arch = "wasm32"))]
#[path = "kithara_queue/sync_media.rs"]
mod sync_media;
#[cfg(not(target_arch = "wasm32"))]
#[path = "kithara_audio/sync_passthrough.rs"]
mod sync_passthrough;
#[cfg(not(target_arch = "wasm32"))]
#[path = "kithara_queue/sync_rt.rs"]
mod sync_rt;
