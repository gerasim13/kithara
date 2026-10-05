#[cfg(android_backend)]
pub(super) mod android;
#[cfg(apple_backend)]
pub(super) mod apple;
#[cfg(all(test, apple_backend, feature = "symphonia"))]
mod mp3;
#[cfg(any(apple_backend, all(target_arch = "wasm32", feature = "webcodecs")))]
mod software;
#[cfg(all(target_arch = "wasm32", feature = "webcodecs"))]
pub(super) mod webcodecs;
