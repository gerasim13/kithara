use kithara::{
    platform::sync::{Arc, Mutex},
    queue::TrackId,
};

use super::settings::Settings;
use crate::{
    item::AudioPlayerItem,
    types::FfiError,
    web::{bridge::WorkerBridge, commands::WorkerCmd, observer::router::Routes},
};

/// Caller-facing ordered queue view: the `(TrackId, item)` pairs the
/// caller inserted, in queue order. The worker owns the canonical
/// [`Queue`](kithara::queue::Queue); this mirror exists because the caller
/// allocates the [`TrackId`](kithara::queue::TrackId) on the main thread
/// and the worker plants the identical id via `*_with_id`, so order is
/// deterministic without a round-trip. Drives `items` / `item_count`
/// exactly as `NativeInner`'s registry + `queue.tracks()` order do on
/// native.
type QueueView = Vec<(TrackId, Arc<AudioPlayerItem>)>;

/// Wasm implementation of the FFI player engine, parallel to
/// [`NativeInner`](crate::native::inner::NativeInner). Exposes the same
/// inherent method set the [`AudioPlayer`](crate::player) facade delegates
/// to, so the single facade body type-checks on both targets.
///
/// The worker owns a canonical Host member and its queue control; `WasmInner`
/// owns the command channel into it plus the main-thread caller-facing state
/// (cached scalar settings + the ordered queue view). Setters write through to
/// both the worker and the local cache so the infallible facade getters can
/// answer synchronously without a worker round-trip.
pub(crate) struct WasmInner {
    pub(super) queue_view: Arc<Mutex<QueueView>>,
    pub(super) settings: Settings,
    pub(super) routes: Routes,
    pub(super) bridge: WorkerBridge,
}

impl Default for WasmInner {
    fn default() -> Self {
        let queue_view: Arc<Mutex<QueueView>> = Arc::new(Mutex::default());
        Self {
            bridge: WorkerBridge::default(),
            routes: Routes::new(Arc::clone(&queue_view)),
            queue_view,
            settings: Settings::default(),
        }
    }
}

/// Wasm `Inner` alias consumed by the cross-platform
/// [`AudioPlayer`](crate::player) facade. Parallel to
/// [`NativeInner`](crate::native::inner::NativeInner) on native.
pub(crate) type Inner = WasmInner;

fn into_internal(err: &wasm_bindgen::JsValue) -> FfiError {
    FfiError::Internal {
        description: err
            .as_string()
            .unwrap_or_else(|| "wasm worker error".into()),
    }
}

pub(super) fn send(bridge: &WorkerBridge, cmd: WorkerCmd) {
    if let Err(err) = bridge.send(cmd) {
        tracing::warn!(?err, "wasm worker command dropped: channel unavailable");
    }
}

pub(super) fn try_send(bridge: &WorkerBridge, cmd: WorkerCmd) -> Result<(), FfiError> {
    bridge.send(cmd).map_err(|err| into_internal(&err))
}
