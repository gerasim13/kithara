use kithara::platform::thread::set_wasm_shim_name;
use tracing_log::LogTracer;
use tracing_wasm::WASMLayerConfigBuilder;
use wasm_bindgen::{JsValue, prelude::wasm_bindgen};

// wasm-ld synthesizes `__heap_base` and `__heap_end` only when the module does
// not define them. This data-section symbol therefore wins and lies below
// `__heap_base`. std's dlmalloc sees an empty `[__heap_base, __heap_end)` range
// on its first allocation and never donates it to the arena.
//
// wasm-bindgen's post-link threads transform reserves the first 64 KiB above
// the original `__heap_base` for its bootstrap mutex, thread counter, and shared
// bootstrap stack. dlmalloc has already inlined that original base. A donation
// puts its first chunk header on the bootstrap mutex, causing later bootstraps
// to block in `memory.atomic.wait32`: AudioWorklet render threads trap and
// Workers hang.
#[used]
#[unsafe(export_name = "__heap_end")]
static HEAP_END: u8 = 0;

#[cfg_attr(target_family = "wasm", allow(unreachable_pub))]
#[wasm_bindgen(start)]
pub fn setup() -> Result<(), JsValue> {
    console_error_panic_hook::set_once();

    // Worker threads import `<shim>.js` for `initSync`; register our
    // wasm-bindgen output name so the engine worker loads the right shim
    // (auto-detection mis-picks a co-loaded `.js` like coi-serviceworker).
    set_wasm_shim_name(env!("CARGO_PKG_NAME"));

    if web_sys::window().is_none() {
        return Ok(());
    }

    crate::web::bridge::initialize()?;

    let _ = LogTracer::init();
    let config = WASMLayerConfigBuilder::new()
        .set_report_logs_in_timings(false)
        .build();
    tracing_wasm::set_as_global_default_with_config(config);
    Ok(())
}

/// Build revision string: `"version git_hash build_timestamp"`.
#[must_use]
#[wasm_bindgen]
pub fn build_info() -> String {
    format!(
        "v{} {} {}",
        env!("CARGO_PKG_VERSION"),
        env!("BUILD_GIT_HASH"),
        env!("BUILD_TIMESTAMP"),
    )
}
