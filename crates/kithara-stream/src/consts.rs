use kithara_platform::time::Duration;

/// Watchdog budget for the blocking [`Read`](std::io::Read) adapter. The
/// source's own give-up authority (the network layer's inactivity timeout and
/// retry budget) must fail a stalled range first, so only a read that neither
/// progresses nor fails is a hang; sized like the storage and HLS
/// blocking-wait watchdogs.
pub(crate) const READ_HANG_TIMEOUT: Duration = Duration::from_secs(180);
