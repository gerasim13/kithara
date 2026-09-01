use kithara_platform::sync::Arc;

use super::gate::ReadinessGate;

pub(super) struct GateGuard {
    readiness: Arc<ReadinessGate>,
    armed: bool,
}

impl GateGuard {
    pub(super) const fn new(readiness: Arc<ReadinessGate>) -> Self {
        Self {
            readiness,
            armed: true,
        }
    }

    pub(super) const fn disarm(&mut self) {
        self.armed = false;
    }

    pub(super) fn shared(&self) -> Arc<ReadinessGate> {
        Arc::clone(&self.readiness)
    }

    delegate::delegate! {
        to self.readiness {
            pub(super) fn fail(&self);
            pub(super) fn is_ready(&self) -> bool;
            pub(super) fn mark_ready(&self);
        }
    }
}

impl Drop for GateGuard {
    fn drop(&mut self) {
        if self.armed {
            self.readiness.fail();
        }
    }
}
