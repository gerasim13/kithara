#![cfg_attr(all(rtsan, not(rtsan_standalone)), feature(sanitize))]
#![forbid(unsafe_code)]

#[cfg(not(target_arch = "wasm32"))]
mod analysis_offer_is_realtime_safe;
