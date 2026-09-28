#[cfg(not(target_arch = "wasm32"))]
mod native;
#[cfg(not(target_arch = "wasm32"))]
use native as platform;

#[cfg(target_arch = "wasm32")]
mod wasm;
#[cfg(target_arch = "wasm32")]
use wasm as platform;

#[cfg(not(target_arch = "wasm32"))]
mod panic_dump;

#[cfg(not(target_arch = "wasm32"))]
mod threads;

mod shared;

#[cfg(not(target_arch = "wasm32"))]
mod detector_native;

#[cfg(target_arch = "wasm32")]
mod detector_wasm;

#[cfg(not(target_arch = "wasm32"))]
pub use detector_native::HangDetector;
#[cfg(target_arch = "wasm32")]
pub use detector_wasm::HangDetector;
#[cfg(not(target_arch = "wasm32"))]
pub use panic_dump::{install_panic_dump, suppress_expected_panic_dumps};
#[doc(hidden)]
pub use platform::{PreKillGuard, record_test_hang};
#[cfg(all(test, not(target_arch = "wasm32")))]
pub(crate) use platform::{parse_timeout_secs, resolve_dump_dir, sanitize_label, write_dump};
pub use shared::{HangDump, NoContext, TimeoutOverride, default_timeout, override_timeout};

#[cfg(test)]
mod tests;
