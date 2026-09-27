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

/// Tickets one deck holds at once: the ring stays occupied until the
/// callback claims or returns its ticket, so a full ring means the deck is
/// still busy with an activation.
pub(crate) const TICKET_RING: usize = 1;

/// Returns one deck holds for the control thread: a claim needs the whole
/// ring free, so the old reader's tail and one returned track always fit.
pub(crate) const RETURN_RING: usize = 2;
