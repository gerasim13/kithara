use kithara_platform::time::{Duration, WallInstant};

/// Wait on wall time when asserting that an event has not happened yet.
///
/// A Flash participant blocked on a native mutex can pin virtual time, so a
/// virtual deadline cannot bound that negative assertion.
pub fn wall_sleep(duration: Duration) {
    let deadline = WallInstant::now() + duration;
    while WallInstant::now() < deadline {
        std::thread::yield_now();
    }
}
