use kithara_platform::{
    CancelToken,
    sync::{Arc, mpsc},
};
use tracing::warn;

use super::{Node, RtPolicy, SchedulerWake, ServiceClass};

pub(crate) type SlotId = u64;

pub(crate) enum SchedulerCmd<N> {
    Register(SlotId, N),
    Unregister(SlotId),
    Shutdown,
}

pub(crate) struct Slot<N> {
    pub(crate) node: N,
    pub(crate) rt_policy: RtPolicy,
    pub(crate) service_class: ServiceClass,
    pub(crate) id: SlotId,
    pub(crate) is_terminal: bool,
}

pub(crate) struct SchedulerHandle<N> {
    inner: Arc<SchedulerInner<N>>,
}

impl<N> Clone for SchedulerHandle<N> {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

struct SchedulerInner<N> {
    wake: Arc<SchedulerWake>,
    cancel: CancelToken,
    cmd_tx: mpsc::Sender<SchedulerCmd<N>>,
}

impl<N> Drop for SchedulerInner<N> {
    fn drop(&mut self) {
        shutdown_inner(self);
    }
}

impl<N: Node> SchedulerHandle<N> {
    pub(crate) fn new(
        wake: Arc<SchedulerWake>,
        cancel: CancelToken,
        cmd_tx: mpsc::Sender<SchedulerCmd<N>>,
    ) -> Self {
        Self {
            inner: Arc::new(SchedulerInner {
                wake,
                cancel,
                cmd_tx,
            }),
        }
    }

    #[must_use]
    pub(crate) fn register(&self, id: SlotId, node: N) -> bool {
        if self.inner.cancel.is_cancelled() {
            return false;
        }
        if self
            .inner
            .cmd_tx
            .send(SchedulerCmd::Register(id, node))
            .is_err()
        {
            warn!(slot_id = id, "register: scheduler channel closed");
            return false;
        }
        self.inner.wake.wake();
        !self.inner.cancel.is_cancelled()
    }

    pub(crate) fn shutdown(&self) {
        shutdown_inner(&self.inner);
    }

    pub(crate) fn unregister(&self, id: SlotId) {
        if self.inner.cancel.is_cancelled() {
            return;
        }
        if self
            .inner
            .cmd_tx
            .send(SchedulerCmd::Unregister(id))
            .is_err()
        {
            warn!(slot_id = id, "unregister: scheduler channel closed");
        }
        self.inner.wake.wake();
    }

    delegate::delegate! {
        to self.inner.wake {
            pub(crate) fn wake(&self);
            /// Publish a coalesced worker pass without unparking from the caller.
            #[call(defer)]
            pub(crate) fn defer_wake(&self);
        }
    }
}

fn shutdown_inner<N>(inner: &SchedulerInner<N>) {
    inner.cmd_tx.send(SchedulerCmd::Shutdown).ok();
    inner.cancel.cancel();
    inner.wake.wake();
}

#[cfg(test)]
mod tests {
    use kithara_platform::{CancelToken, thread, time::Duration};
    use kithara_test_utils::kithara;

    use super::super::{Node, Scheduler, SchedulerEvent, SchedulerObserver, TickResult};

    struct TestObserver;

    impl SchedulerObserver for TestObserver {
        fn on_event(&mut self, _event: SchedulerEvent) {}
    }

    struct DummyNode;

    impl Node for DummyNode {
        fn tick(&mut self) -> TickResult {
            TickResult::Done
        }
    }

    #[kithara::test]
    fn scheduler_creates_and_drops_cleanly() {
        let handle = Scheduler::<DummyNode, TestObserver>::start(
            "test-worker".into(),
            TestObserver,
            CancelToken::never(),
        );
        thread::sleep(Duration::from_millis(10));
        handle.shutdown();
        thread::sleep(Duration::from_millis(50));
    }
}
