//! Deliberate pacing in test bodies and spawned blocking work.

use kithara_platform::time::Duration;

use crate::kithara;

/// Sleep on the test's clock, including in work spawned outside the test body.
///
/// Use only for *deliberate* time advance — real-time playback pacing or
/// simulated network latency, where the duration itself is the thing under test.
/// This is NOT a substitute for waiting on program state; for "wait until X
/// happens" use [`crate::wait::wait_until`].
/// `no_block`: deliberate pace; the sleep duration is the behavior under test.
#[kithara::allow_block]
#[kithara::flash(true)]
pub fn pace(duration: Duration) {
    kithara_platform::thread::sleep(duration);
}
