use kithara_platform::time::Duration;

/// Default session time before a track ends at which the queue loads its
/// successor.
pub(crate) const DEFAULT_PRELOAD_LEAD: Duration = Duration::from_millis(3_500);
