#[cfg(target_arch = "wasm32")]
use futures::lock::{Mutex, MutexGuard};

#[cfg(target_arch = "wasm32")]
static WASM_SERIAL: Mutex<()> = Mutex::new(());

/// Serialize one `#[kithara::test(serial)]` body in a WASM test binary.
#[cfg(target_arch = "wasm32")]
pub async fn wasm_serial_guard() -> MutexGuard<'static, ()> {
    WASM_SERIAL.lock().await
}
