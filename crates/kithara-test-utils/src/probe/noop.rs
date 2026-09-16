use url::Url;

use super::IntoProbeArg;

impl IntoProbeArg for &Url {
    fn into_probe_arg(self) -> u64 {
        0
    }
}

impl<T: IntoProbeArg> IntoProbeArg for Option<T> {
    fn into_probe_arg(self) -> u64 {
        self.map_or(u64::MAX, IntoProbeArg::into_probe_arg)
    }
}

pub fn register_probes() {}

pub fn fire_0(_operation: u64) {}
pub fn fire_1(_operation: u64, _a0: u64) {}
pub fn fire_2(_operation: u64, _a0: u64, _a1: u64) {}
pub fn fire_3(_operation: u64, _a0: u64, _a1: u64, _a2: u64) {}
pub fn fire_4(_operation: u64, _a0: u64, _a1: u64, _a2: u64, _a3: u64) {}
pub fn fire_5(_operation: u64, _a0: u64, _a1: u64, _a2: u64, _a3: u64, _a4: u64) {}
