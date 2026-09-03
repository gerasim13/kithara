#![cfg(target_arch = "wasm32")]

use js_sys::Reflect;
use kithara::events::TrackId;
use wasm_bindgen::JsValue;
use wasm_bindgen_test::wasm_bindgen_test;

#[path = "decode.rs"]
mod decode;
#[path = "decode_item.rs"]
mod decode_item;
#[path = "encode.rs"]
mod encode;
#[path = "encode_item.rs"]
mod encode_item;
#[path = "marshal.rs"]
mod marshal;

mod types {
    pub(crate) use kithara_ffi::types::*;
}

use types::{FfiItemEvent, FfiKeySource, FfiPlayerEvent, FfiStretchBackendKind, FfiTrackStatus};

const MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;
const MAX_SAFE_INTEGER_F64: f64 = 9_007_199_254_740_991.0;

fn keys(value: &JsValue) -> Vec<String> {
    let mut keys = Reflect::own_keys(value)
        .expect("wire keys")
        .iter()
        .map(|key| key.as_string().expect("string key"))
        .collect::<Vec<_>>();
    keys.sort_unstable();
    keys
}

#[wasm_bindgen_test]
fn stretch_backend_event_preserves_kind_and_backend() {
    let encoded = encode::encode(&FfiPlayerEvent::DjStretchBackendChanged {
        kind: FfiStretchBackendKind::Bungee,
    });

    let kind = Reflect::get(&encoded, &JsValue::from_str("kind"))
        .expect("event kind")
        .as_string();
    let backend = Reflect::get(&encoded, &JsValue::from_str("backend"))
        .expect("backend payload")
        .as_string();

    assert_eq!(kind.as_deref(), Some("DjStretchBackendChanged"));
    assert_eq!(backend.as_deref(), Some("Bungee"));
    assert!(matches!(
        decode::decode(&encoded),
        Some(FfiPlayerEvent::DjStretchBackendChanged {
            kind: FfiStretchBackendKind::Bungee
        })
    ));
}

#[wasm_bindgen_test]
fn track_status_schema_preserves_max_safe_item_id() {
    let encoded = encode::encode(&FfiPlayerEvent::TrackStatusChanged {
        item_id: TrackId(MAX_SAFE_INTEGER),
        status: FfiTrackStatus::Loaded,
    });

    assert_eq!(keys(&encoded), ["item_id", "kind", "status"]);
    assert_eq!(
        marshal::get_str(&encoded, "kind").as_deref(),
        Some("TrackStatusChanged")
    );
    assert_eq!(
        marshal::get_f64(&encoded, "item_id"),
        Some(MAX_SAFE_INTEGER_F64)
    );
    assert_eq!(marshal::get_f64(&encoded, "status"), Some(3.0));
    assert!(!Reflect::has(&encoded, &JsValue::from_str("reason")).expect("reason presence"));
    assert!(matches!(
        decode::decode(&encoded),
        Some(FfiPlayerEvent::TrackStatusChanged {
            item_id: TrackId(MAX_SAFE_INTEGER),
            status: FfiTrackStatus::Loaded
        })
    ));
}

#[wasm_bindgen_test]
fn drm_key_schema_omits_absent_optional_fields() {
    let encoded = encode_item::encode_item_event(&FfiItemEvent::DrmKeyAcquired {
        key_host: None,
        source: FfiKeySource::DiskCache,
        bytes: MAX_SAFE_INTEGER,
        latency_ms: None,
    });

    assert_eq!(keys(&encoded), ["bytes", "kind", "source"]);
    assert_eq!(
        marshal::get_str(&encoded, "kind").as_deref(),
        Some("DrmKeyAcquired")
    );
    assert_eq!(
        marshal::get_str(&encoded, "source").as_deref(),
        Some("DiskCache")
    );
    assert_eq!(
        marshal::get_f64(&encoded, "bytes"),
        Some(MAX_SAFE_INTEGER_F64)
    );
    assert!(!Reflect::has(&encoded, &JsValue::from_str("key_host")).expect("key host presence"));
    assert!(!Reflect::has(&encoded, &JsValue::from_str("latency_ms")).expect("latency presence"));
    assert!(matches!(
        decode_item::decode_item_event(&encoded),
        Some(FfiItemEvent::DrmKeyAcquired {
            key_host: None,
            source: FfiKeySource::DiskCache,
            bytes: MAX_SAFE_INTEGER,
            latency_ms: None
        })
    ));
}
