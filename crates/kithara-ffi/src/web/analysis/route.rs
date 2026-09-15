use js_sys::{Function, Object, Reflect};
use kithara::platform::sync::{Arc, Mutex};
use send_wrapper::SendWrapper;
use wasm_bindgen::{JsCast, JsValue};

use super::encode::ANALYSIS_SCOPE;

#[derive(Clone, Default)]
pub(crate) struct AnalysisRoute {
    sink: Arc<Mutex<Option<SendWrapper<Function>>>>,
}

impl AnalysisRoute {
    pub(crate) fn set(&self, func: Function) {
        *self.sink.lock() = Some(SendWrapper::new(func));
    }

    pub(crate) fn dispatch(&self, scope: Option<&str>, data: &JsValue) -> bool {
        if scope != Some(ANALYSIS_SCOPE) {
            return false;
        }
        let func = self.sink.lock().as_ref().map(|func| (*func).clone());
        if let Some(func) = func {
            if let Some(payload) = data.dyn_ref::<Object>() {
                let _ = Reflect::delete_property(payload, &JsValue::from_str("scope"));
            }
            let _ = func.call1(&JsValue::UNDEFINED, data);
        }
        true
    }
}
