pub(super) const OPEN: u64 = 0;
pub(super) const CONTROL: u64 = 1;
pub(super) const AUDIO_CLAIMED: u64 = 2;
pub(super) const CLOSED: u64 = 3;

/// Low bits of a cell's source word: the unreconciled change flags.
pub(super) const CHANGE_SHIFT: u32 = 2;
pub(super) const CHANGE_MASK: u64 = (1 << CHANGE_SHIFT) - 1;
pub(super) const MAX_REVISION: u64 = u64::MAX >> CHANGE_SHIFT;

pub(super) const UNCHANGED: u64 = 0;
pub(super) const TIMING: u64 = 0b01;
pub(super) const DISCONTINUITY: u64 = 0b10;
