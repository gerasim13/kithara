//! Whether a test budget that expired was actually paid for in real seconds.
//!
//! A test's wait budget is measured on the platform clock, which under `flash`
//! is virtual: the quiescence engine advances it in one step whenever every
//! participant parks, so a budget can expire with no real time spent at all. A
//! dump or a panic from that reads exactly like one from a genuine stall, and
//! the two need opposite fixes — one is a wait the engine mis-declared
//! quiescent, the other is work that never finished.

use kithara_platform::time::{Duration, WallInstant};

/// Worded account of what the real clock did while `budget` ran out.
///
/// Worded rather than measured because the stress report clusters diagnostics
/// by text: a raw duration would make every firing its own cluster and hide the
/// very pattern the verdict exists to show.
#[must_use]
pub fn real_clock_verdict(started: WallInstant, budget: Duration) -> &'static str {
    if started.elapsed() < budget {
        "real clock leapt the budget"
    } else {
        "real clock spent the budget"
    }
}
