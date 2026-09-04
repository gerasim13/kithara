#[cfg(not(target_arch = "wasm32"))]
mod native;
#[cfg(target_arch = "wasm32")]
mod wasm;

use kithara_play::PlayError;
#[cfg(not(target_arch = "wasm32"))]
pub(super) use native::Platform;
#[cfg(target_arch = "wasm32")]
pub(super) use wasm::Platform;

pub(super) trait PlatformResult<T> {
    fn resolve(self) -> Result<T, PlayError>;
}

impl<T> PlatformResult<T> for Result<T, PlayError> {
    fn resolve(self) -> Self {
        self
    }
}
