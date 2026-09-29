use kithara_platform::time::Duration;

/// Watchdog timeout for the network-bound `wait_range_inner`: sized well
/// above the `kithara-net` `inactivity_timeout` (plus retry backoff) so a
/// stalled upstream is failed by the network layer (this wait then returns
/// `Failed`) before the deadlock-watchdog fires. Only a wait that never
/// returns after the fetch resolved is a real deadlock.
pub(crate) const WAIT_HANG_TIMEOUT: Duration = Duration::from_secs(180);

/// What a dead owner left in the tmp a successor reclaims.
#[cfg(test)]
#[cfg(not(target_arch = "wasm32"))]
pub(crate) const DEAD_OWNERS_BYTES: &[u8] = b"stale-from-previous-process";
