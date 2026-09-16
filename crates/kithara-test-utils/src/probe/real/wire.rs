use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
};

use url::Url;

use super::super::IntoProbeArg;

impl IntoProbeArg for &Url {
    fn into_probe_arg(self) -> u64 {
        let mut hasher = DefaultHasher::new();
        self.as_str().hash(&mut hasher);
        hasher.finish()
    }
}

impl<T: IntoProbeArg> IntoProbeArg for Option<T> {
    fn into_probe_arg(self) -> u64 {
        self.map_or(u64::MAX, |value| {
            let raw = value.into_probe_arg();
            debug_assert!(
                raw != u64::MAX,
                "Option<T>::None sentinel collides with Some(value) producing u64::MAX"
            );
            raw
        })
    }
}

/// Register the macOS `DTrace` probes embedded in the binary. Other targets use
/// the tracing USDT backend and do not require registration.
pub fn register_probes() {
    imp::register();
}

#[cfg(all(target_os = "macos", feature = "usdt", not(miri)))]
mod imp {
    use std::sync::OnceLock;

    static REGISTERED: OnceLock<()> = OnceLock::new();

    pub(super) fn register() {
        REGISTERED.get_or_init(|| {
            let _ = usdt::register_probes();
        });
    }
}

#[cfg(any(not(target_os = "macos"), not(feature = "usdt"), miri))]
mod imp {
    pub(super) fn register() {}
}
