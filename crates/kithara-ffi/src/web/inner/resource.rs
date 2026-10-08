use kithara::platform::sync::Arc;

use super::WasmInner;
use crate::{
    observer::FfiKeyProcessor,
    types::{FfiAbrMode, FfiKeyRule},
    web::commands::WorkerCmd,
};
impl WasmInner {
    pub(crate) fn set_abr_mode(&self, mode: FfiAbrMode) {
        let variant_index = match mode {
            FfiAbrMode::Auto => None,
            FfiAbrMode::Manual { variant_index } => Some(variant_index),
        };
        self.send(WorkerCmd::SetAbrMode { variant_index });
    }

    pub(crate) fn setup_hls_aes(&self, processor: Arc<dyn FfiKeyProcessor>) {
        let salt = crate::web::interop::generate_salt();
        let rule = FfiKeyRule {
            processor,
            headers: None,
            query_params: None,
            domains: vec!["*".to_string()],
            salt: Some(salt),
        };
        self.setup_hls_aes_with_rule(rule);
    }

    pub(crate) fn setup_hls_aes_with_rule(&self, rule: FfiKeyRule) {
        crate::web::keys::install_main_processor(Arc::clone(&rule.processor));
        let salt = rule.salt.unwrap_or_else(crate::web::interop::generate_salt);
        self.send(WorkerCmd::SetupHlsAes {
            salt,
            domains: rule.domains,
            headers: rule.headers,
            query_params: rule.query_params,
        });
    }

    pub(crate) fn setup_network(&self, auth_token: String) {
        self.send(WorkerCmd::AuthToken { token: auth_token });
    }

    pub(crate) fn update_peak_bitrate(&self, wifi_bps: f64, cellular_bps: f64) {
        self.send(WorkerCmd::PeakBitrate {
            wifi_bps,
            cellular_bps,
        });
    }
}
