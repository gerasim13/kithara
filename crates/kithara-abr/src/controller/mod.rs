//! Shared per-player ABR controller with per-peer tick readiness,
//! interval deadlines, and event throttling.

mod core;
mod driver;
mod peer;
mod throttle;
mod tick;

pub use core::{AbrController, AbrPeerId, AbrSettings, AbrSettingsPatch};
