//! Shared per-player ABR registry, coalesced tick readiness and deadlines,
//! and event throttling. The existing downloader run loop drives decisions.

mod core;
mod driver;
mod peer;
mod throttle;
mod tick;

pub use core::{AbrController, AbrPeerId, AbrSettings, AbrSettingsPatch};
