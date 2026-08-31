mod counter;
mod pair;

use counter::BudgetCounter;
pub(crate) use pair::{BudgetPair, Reservation, ReserveFailure};

/// Hard byte limit shared by every pool in one region.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct OverallBudget(pub usize);

/// Percentage of the overall budget available to one physical pool.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Percent(pub u8);

impl Percent {
    /// The selected pool may compete for the entire region budget.
    pub const FULL: Self = Self(100);

    pub(crate) const fn is_valid(self) -> bool {
        self.0 <= Self::FULL.0
    }
}

#[derive(Clone, Debug)]
pub(crate) struct RegionBudget(BudgetCounter);

impl RegionBudget {
    pub(crate) fn new(limit: usize) -> Self {
        Self(BudgetCounter::new(limit))
    }

    pub(crate) fn same_region(&self, other: &Self) -> bool {
        self.0.same_counter(&other.0)
    }

    delegate::delegate! {
        to self.0 {
            pub(crate) fn current(&self) -> usize;
            pub(crate) fn limit(&self) -> usize;
            pub(crate) fn peak(&self) -> usize;
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct PoolBudget(BudgetCounter);

impl PoolBudget {
    pub(crate) fn new(limit: usize) -> Self {
        Self(BudgetCounter::new(limit))
    }

    delegate::delegate! {
        to self.0 {
            pub(crate) fn current(&self) -> usize;
            pub(crate) fn limit(&self) -> usize;
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct BudgetSnapshot {
    pub(crate) current: usize,
    pub(crate) limit: usize,
}

#[cfg(test)]
mod tests {
    use kithara_test_utils::kithara;

    use super::{BudgetCounter, BudgetPair, RegionBudget};

    #[kithara::test]
    fn uncommitted_reservation_rolls_back_both_counters() {
        let pair = BudgetPair::new(RegionBudget::new(16), 16);
        let reservation = pair.reserve(8).unwrap_or_else(|error| panic!("{error:?}"));
        assert_eq!(pair.region_current(), 8);
        assert_eq!(pair.current(), 8);

        drop(reservation);

        assert_eq!(pair.region_current(), 0);
        assert_eq!(pair.current(), 0);
    }

    #[kithara::test]
    fn underflow_release_keeps_the_charge() {
        let counter = BudgetCounter::new(16);
        counter
            .try_acquire(8)
            .unwrap_or_else(|snapshot| panic!("{snapshot:?}"));

        assert!(!counter.release(9, "test"));
        assert_eq!(counter.current(), 8);
    }
}
