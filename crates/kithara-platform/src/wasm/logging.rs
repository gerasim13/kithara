use wasm_bindgen::JsValue;
use web_sys::console;

/// Emit an error-level diagnostic line through the platform-appropriate sink.
///
/// On native this routes through `tracing`. On wasm it writes to the browser
/// `console` directly rather than through the global `tracing` subscriber: on
/// a non-main wasm instance (e.g. the audio worklet) that subscriber is a
/// `dyn` object whose vtable lives in the main instance's function table, so
/// dispatching to it cross-instance would trap. `console.error` is a per-realm
/// import that is valid in every scope, including `AudioWorkletGlobalScope`.
pub fn log_error(msg: &str) {
    console::error_1(&JsValue::from_str(msg));
}
