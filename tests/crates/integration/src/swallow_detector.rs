use kithara::{platform::time::Duration, usdt::operation_id};

use crate::usdt_observer::ProbeRecord;

const WRITE_PLAYHEAD: u64 = operation_id("write_playhead");

pub fn assert_committed_reached(records: &[ProbeRecord], min: Duration) {
    let reached = records
        .iter()
        .filter(|record| record.operation == WRITE_PLAYHEAD)
        .map(|record| record.payload[0])
        .max()
        .is_some_and(|position| position >= u64::try_from(min.as_nanos()).unwrap_or(u64::MAX));
    assert!(reached, "committed playhead did not reach {min:?}");
}

pub fn assert_no_committed_swallow(records: &[ProbeRecord], maximum_step: Duration) {
    let mut previous = None;
    let maximum = u64::try_from(maximum_step.as_nanos()).unwrap_or(u64::MAX);
    for record in records
        .iter()
        .filter(|record| record.operation == WRITE_PLAYHEAD)
    {
        let current = record.payload[0];
        if let Some(previous) = previous {
            assert!(
                current.saturating_sub(previous) <= maximum,
                "committed playhead jumped from {previous} ns to {current} ns"
            );
        }
        previous = Some(current);
    }
    assert!(previous.is_some(), "zero write_playhead USDT records");
}
