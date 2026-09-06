use super::{BudgetSnapshot, PoolBudget, RegionBudget};

#[derive(Clone, Debug)]
pub(crate) struct BudgetPair {
    pool: PoolBudget,
    region: RegionBudget,
}

impl BudgetPair {
    pub(crate) fn new(region: RegionBudget, pool_limit: usize) -> Self {
        Self {
            region,
            pool: PoolBudget::new(pool_limit),
        }
    }

    pub(crate) fn release(&self, amount: usize) {
        if amount == 0 {
            return;
        }
        if self.pool.0.release(amount, "typed pool") {
            let _ = self.region.counter.release(amount, "buffer region");
        }
    }

    pub(crate) fn reserve(&self, amount: usize) -> Result<Reservation<'_>, ReserveFailure> {
        let mut reservation = Reservation {
            amount,
            budgets: self,
            pool_acquired: false,
            region_acquired: false,
        };
        if amount == 0 {
            return Ok(reservation);
        }

        self.region
            .counter
            .try_acquire(amount)
            .map_err(|snapshot| ReserveFailure::Overall { amount, snapshot })?;
        reservation.region_acquired = true;

        self.pool
            .0
            .try_acquire(amount)
            .map_err(|snapshot| ReserveFailure::Pool { amount, snapshot })?;
        reservation.pool_acquired = true;
        Ok(reservation)
    }

    delegate::delegate! {
        to self.pool {
            pub(crate) fn current(&self) -> usize;
            pub(crate) fn limit(&self) -> usize;
        }
        to self.region {
            #[call(current)]
            pub(crate) fn region_current(&self) -> usize;
            #[call(limit)]
            pub(crate) fn region_limit(&self) -> usize;
            #[call(reclaim)]
            pub(crate) fn reclaim_region(&self, requested: usize) -> usize;
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum ReserveFailure {
    Overall {
        amount: usize,
        snapshot: BudgetSnapshot,
    },
    Pool {
        amount: usize,
        snapshot: BudgetSnapshot,
    },
}

#[must_use = "dropping an uncommitted reservation rolls it back"]
pub(crate) struct Reservation<'a> {
    budgets: &'a BudgetPair,
    pool_acquired: bool,
    region_acquired: bool,
    amount: usize,
}

impl Reservation<'_> {
    pub(crate) fn commit(mut self) {
        self.pool_acquired = false;
        self.region_acquired = false;
    }

    pub(crate) fn reduce(&mut self, amount: usize) {
        debug_assert!(amount <= self.amount);
        self.budgets.release(amount);
        self.amount -= amount;
    }
}

impl Drop for Reservation<'_> {
    fn drop(&mut self) {
        if self.pool_acquired {
            if self
                .budgets
                .pool
                .0
                .release(self.amount, "typed pool reservation")
            {
                self.pool_acquired = false;
            } else {
                return;
            }
        }
        if self.region_acquired {
            let _ = self
                .budgets
                .region
                .counter
                .release(self.amount, "buffer region reservation");
            self.region_acquired = false;
        }
    }
}
