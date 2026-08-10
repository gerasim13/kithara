#[cfg(target_arch = "wasm32")]
use kithara_platform::{AsyncMutex, AsyncMutexGuard};

#[cfg(target_arch = "wasm32")]
static WASM_SERIAL: AsyncMutex<()> = AsyncMutex::new(());

/// Serialize one `#[kithara::test(serial)]` body in a WASM test binary.
#[cfg(target_arch = "wasm32")]
pub async fn wasm_serial_guard() -> AsyncMutexGuard<'static, ()> {
    WASM_SERIAL.lock().await
}
