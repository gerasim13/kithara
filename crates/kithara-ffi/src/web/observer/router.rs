use std::mem;

use js_sys::{Function, Object, Reflect};
use kithara::{
    events::TrackId,
    platform::sync::{Arc, Mutex},
};
use send_wrapper::SendWrapper;
use wasm_bindgen::{JsCast, JsValue, prelude::Closure};
use web_sys::{BroadcastChannel, MessageEvent, console};

use super::{decode::decode, decode_item::decode_item_event, marshal::get_id_req};
use crate::{
    item::AudioPlayerItem,
    observer::{ItemObserver, PlayerObserver},
    types::{FfiItemEvent, FfiItemStatus, FfiPlayerEvent, FfiTrackStatus},
    web::{analysis::encode::ANALYSIS_SCOPE, observer::source::EVENT_CHANNEL},
};

type QueueView = Vec<(TrackId, Arc<AudioPlayerItem>)>;

/// Main-thread fan-out of the worker event channel to the player, per-item and
/// analysis sinks.
#[derive(Clone)]
pub(crate) struct Routes {
    queue_view: Arc<Mutex<QueueView>>,
    sinks: Arc<Mutex<Sinks>>,
}

#[derive(Default)]
struct Sinks {
    player: Option<Arc<dyn PlayerObserver>>,
    analysis: Option<SendWrapper<Function>>,
    installed: bool,
}

impl Routes {
    pub(crate) fn new(queue_view: Arc<Mutex<QueueView>>) -> Self {
        Self {
            queue_view,
            sinks: Arc::new(Mutex::default()),
        }
    }

    pub(crate) fn set_analysis(&self, func: Function) {
        self.sinks.lock().analysis = Some(SendWrapper::new(func));
        self.arm();
    }

    pub(crate) fn set_player(&self, observer: Arc<dyn PlayerObserver>) {
        self.sinks.lock().player = Some(observer);
        self.arm();
    }

    fn arm(&self) {
        if self.sinks.lock().installed {
            return;
        }
        if self.install() {
            self.sinks.lock().installed = true;
        }
    }

    fn install(&self) -> bool {
        let Ok(channel) = BroadcastChannel::new(EVENT_CHANNEL) else {
            console::warn_1(&JsValue::from_str(
                "kithara: BroadcastChannel unavailable; observers disabled",
            ));
            return false;
        };
        let routes = self.clone();
        let closure = Closure::wrap(Box::new(move |ev: MessageEvent| {
            routes.dispatch(&ev.data());
        }) as Box<dyn FnMut(MessageEvent)>);
        channel.set_onmessage(Some(closure.as_ref().unchecked_ref()));
        closure.forget();
        mem::forget(channel);
        true
    }

    fn dispatch(&self, data: &JsValue) {
        match scope(data).as_deref() {
            Some("item") => self.route_item_message(data),
            Some(ANALYSIS_SCOPE) => self.route_analysis(data),
            _ => self.route_player(data),
        }
    }

    fn route_analysis(&self, data: &JsValue) {
        let func = self
            .sinks
            .lock()
            .analysis
            .as_ref()
            .map(|func| (*func).clone());
        if let Some(func) = func {
            if let Some(payload) = data.dyn_ref::<Object>() {
                let _ = Reflect::delete_property(payload, &JsValue::from_str(SCOPE_KEY));
            }
            let _ = func.call1(&JsValue::UNDEFINED, data);
        }
    }

    fn route_player(&self, data: &JsValue) {
        let Some(event) = decode(data) else {
            return;
        };
        self.route_to_item(&event);
        let observer = self.sinks.lock().player.clone();
        if let Some(observer) = observer {
            observer.on_event(event);
        }
    }

    fn item(&self, id: TrackId) -> Option<Arc<AudioPlayerItem>> {
        self.queue_view
            .lock()
            .iter()
            .find(|(existing, _)| *existing == id)
            .map(|(_, item)| Arc::clone(item))
    }

    fn route_to_item(&self, event: &FfiPlayerEvent) {
        let FfiPlayerEvent::TrackStatusChanged { item_id, status } = event else {
            return;
        };
        let Some(item) = self.item(*item_id) else {
            return;
        };
        update_item_state(&item, status);
        if let Some(item_obs) = item.observer() {
            dispatch_track_status_to_item(&item_obs, status);
        }
    }

    fn route_item_message(&self, data: &JsValue) {
        let track_id = get_id_req(data, "track_id");
        let item_event = decode_item_event(data);
        let (Some(track_id), Some(item_event)) = (track_id, item_event) else {
            return;
        };
        let Some(item) = self.item(track_id) else {
            return;
        };
        if let Some(obs) = item.observer() {
            obs.on_event(item_event);
        }
    }
}

const SCOPE_KEY: &str = "scope";

fn scope(data: &JsValue) -> Option<String> {
    Reflect::get(data, &JsValue::from_str(SCOPE_KEY))
        .ok()
        .and_then(|value| value.as_string())
}

fn update_item_state(item: &Arc<AudioPlayerItem>, status: &FfiTrackStatus) {
    match status {
        FfiTrackStatus::Loaded => {
            let duration = item.duration_sec();
            item.state.lock().resolve_duration(duration);
        }
        FfiTrackStatus::Failed { .. } => {
            item.state.lock().mark_failed();
        }
        _ => {}
    }
}

fn dispatch_track_status_to_item(observer: &Arc<dyn ItemObserver>, status: &FfiTrackStatus) {
    match status {
        FfiTrackStatus::Loaded => observer.on_event(FfiItemEvent::StatusChanged {
            status: FfiItemStatus::ReadyToPlay,
        }),
        FfiTrackStatus::Failed { reason } => {
            observer.on_event(FfiItemEvent::StatusChanged {
                status: FfiItemStatus::Failed,
            });
            observer.on_event(FfiItemEvent::Error {
                error: reason.clone(),
            });
        }
        _ => {}
    }
}
