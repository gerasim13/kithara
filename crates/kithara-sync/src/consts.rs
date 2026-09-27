/// Time constant the tempo fixtures approach a new target with.
#[cfg(test)]
pub(crate) const SMOOTHING_SECONDS: f64 = 0.005;

/// The first session frame no caller can use.
#[cfg(test)]
pub(crate) const OPEN_END: i64 = i64::MAX;

pub(crate) const SECONDS_PER_MINUTE: f64 = 60.0;

/// Output frames between the audio already committed or rendered and an
/// entry's first admissible activation.
pub(crate) const ENTRY_LEAD_FRAMES: i64 = 2_048;

/// Receipts one activation writes: `Armed`, then `Presented`. The mailbox
/// holds exactly one pair, so a pair is reservable only while no receipt of
/// the slot waits for the owner.
pub(crate) const RECEIPT_PAIR: usize = 2;
