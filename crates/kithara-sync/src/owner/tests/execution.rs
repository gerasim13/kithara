use kithara_platform::{
    CancelToken,
    maybe_send::MaybeSendFuture,
    sync::{Arc, Mutex, mpsc as blocking},
    tokio::{
        runtime::Handle,
        sync::{mpsc, oneshot},
    },
};
use kithara_signal::TransportRevision;
use kithara_test_utils::kithara;
use kithara_warp::{BeatGridId, WarpPlan};

use super::{
    TestGroup,
    modes::{Group, synced_deck},
    preparation::{asset_grid, attach_grid, cue, window},
};
use crate::{
    LoadGeneration, ReceiptSink, StagePort, SyncAdmission, SyncCapability, SyncError,
    SyncExecutionReject, SyncExecutionStamp, SyncExecutor, SyncOperation, SyncReceipt,
};

/// The first session frame no caller can use.
const OPEN_END: i64 = i64::MAX;

/// A lane that reports when it is released.
struct Lane(Option<oneshot::Sender<()>>);

impl Drop for Lane {
    fn drop(&mut self) {
        if let Some(released) = self.0.take() {
            let _ = released.send(());
        }
    }
}

/// Stages every plan at once and announces each staged lane.
#[derive(Clone)]
struct Port {
    runtime: Handle,
    staged: mpsc::UnboundedSender<oneshot::Receiver<()>>,
}

impl StagePort for Port {
    type Media = u64;
    type Lane = Lane;

    fn runtime(&self) -> &Handle {
        &self.runtime
    }

    fn stage(
        self,
        _plan: WarpPlan,
        _cancel: CancelToken,
    ) -> impl MaybeSendFuture<Output = Result<Lane, SyncExecutionReject>> + 'static {
        async move {
            let (released, release) = oneshot::channel();
            let _ = self.staged.send(release);
            Ok(Lane(Some(released)))
        }
    }
}

/// How the fake owner answers receipts.
#[derive(Clone, Copy)]
enum Answer {
    Record,
    RefuseInstalled,
}

/// An owner that records every receipt it is handed; the first answer waits
/// for `gate` when one is set.
struct Owner {
    answer: Answer,
    bound: bool,
    gate: Mutex<Option<blocking::Receiver<()>>>,
    heard: mpsc::UnboundedSender<SyncReceipt>,
}

impl ReceiptSink for Owner {
    fn is_bound(&self) -> bool {
        self.bound
    }

    fn acknowledge(&self, receipt: SyncReceipt) -> bool {
        let _ = self.heard.send(receipt);
        if let Some(gate) = self.gate.lock().take() {
            let _ = gate.recv();
        }
        !matches!(
            (self.answer, receipt),
            (Answer::RefuseInstalled, SyncReceipt::Installed(_))
        )
    }
}

struct Fixture {
    group: Group,
    track: BeatGridId,
    executor: SyncExecutor<Port>,
    heard: mpsc::UnboundedReceiver<SyncReceipt>,
    staged: mpsc::UnboundedReceiver<oneshot::Receiver<()>>,
}

impl Fixture {
    /// A synced deck whose loaded track the executor stages, reporting to
    /// an owner that answers with `answer`.
    fn new(answer: Answer, gate: Option<blocking::Receiver<()>>) -> Self {
        let mut group = synced_deck();
        let track = BeatGridId::allocate().expect("grid id");
        let _ = attach_grid(&mut group, asset_grid(track, 960_000, 24_000));
        let (heard_tx, heard) = mpsc::unbounded_channel();
        let owner = Owner {
            answer,
            bound: true,
            gate: Mutex::new(gate),
            heard: heard_tx,
        };
        let executor = SyncExecutor::new(track, Some(Arc::new(owner)), CancelToken::root());
        let (staged_tx, staged) = mpsc::unbounded_channel();
        executor.load(
            1,
            Some(Port {
                runtime: Handle::current(),
                staged: staged_tx,
            }),
        );
        Self {
            group,
            track,
            executor,
            heard,
            staged,
        }
    }

    /// Prepares the track from `frame`; returns the preparation's stamp once
    /// its lane is staged.
    async fn prepare(&mut self, frame: u64) -> (SyncExecutionStamp, oneshot::Receiver<()>) {
        let admission = self
            .executor
            .transact(&mut self.group, cue_at(self.track, frame))
            .expect("the cue is admitted");
        let SyncAdmission::Prepared(preparation) = admission else {
            panic!("expected a preparation, got {admission:?}");
        };
        let released = self.staged.recv().await.expect("the lane is staged");
        (preparation.stamp(), released)
    }

    async fn next_receipt(&mut self) -> SyncReceipt {
        self.heard.recv().await.expect("the owner hears a receipt")
    }
}

fn cue_at(track: BeatGridId, frame: u64) -> SyncOperation<TestGroup> {
    SyncOperation::Prepare {
        target: track,
        load: LoadGeneration::first(),
        transport: TransportRevision::first(),
        source: cue(frame),
        window: window(0, OPEN_END),
    }
}

fn cancelled(stamp: SyncExecutionStamp) -> SyncReceipt {
    SyncReceipt::Rejected {
        stamp,
        reason: SyncExecutionReject::Cancelled,
    }
}

#[kithara::test(tokio)]
async fn an_installed_lane_the_owner_refuses_is_dropped_and_reported_cancelled() {
    let mut fixture = Fixture::new(Answer::RefuseInstalled, None);

    let (stamp, released) = fixture.prepare(24_000).await;

    assert_eq!(fixture.next_receipt().await, SyncReceipt::Installed(stamp));
    assert_eq!(
        fixture.next_receipt().await,
        cancelled(stamp),
        "an owner that kept the preparation pending hears that its lane is gone"
    );
    assert_eq!(released.await, Ok(()), "the refused lane is released");
}

#[kithara::test(tokio)]
async fn a_lane_dropped_before_its_turn_is_reported_only_by_its_cancellation() {
    let (open, gate) = blocking::channel();
    let mut fixture = Fixture::new(Answer::Record, Some(gate));

    let (first, _) = fixture.prepare(24_000).await;
    assert_eq!(fixture.next_receipt().await, SyncReceipt::Installed(first));
    // The owner is still answering `first`: what follows queues behind it.
    let (superseded, _) = fixture.prepare(48_000).await;
    let (dropped, _) = fixture.prepare(72_000).await;
    fixture.executor.unload();
    let _ = open.send(());

    assert_eq!(
        fixture.next_receipt().await,
        cancelled(dropped),
        "neither the superseded {superseded:?} nor the dropped lane is reported installed"
    );
}

#[kithara::test(tokio)]
async fn a_staged_preparation_needs_an_owner_and_a_stageable_load() {
    let mut group = synced_deck();
    let track = BeatGridId::allocate().expect("grid id");
    let _ = attach_grid(&mut group, asset_grid(track, 960_000, 24_000));
    let (heard, _) = mpsc::unbounded_channel();
    let unbound = Owner {
        answer: Answer::Record,
        bound: false,
        gate: Mutex::new(None),
        heard,
    };
    let refused = |executor: &SyncExecutor<Port>, group: &mut Group| {
        executor
            .transact(group, cue_at(track, 24_000))
            .expect_err("the executor refuses the cue")
            .error()
            .clone()
    };

    let orphan = SyncExecutor::<Port>::new(track, None, CancelToken::root());
    assert_eq!(refused(&orphan, &mut group), SyncError::OwnerUnavailable);
    let detached = SyncExecutor::<Port>::new(track, Some(Arc::new(unbound)), CancelToken::root());
    assert_eq!(refused(&detached, &mut group), SyncError::OwnerUnavailable);

    let (heard, _) = mpsc::unbounded_channel();
    let owner = Owner {
        answer: Answer::Record,
        bound: true,
        gate: Mutex::new(None),
        heard,
    };
    let unstageable = SyncExecutor::<Port>::new(track, Some(Arc::new(owner)), CancelToken::root());
    unstageable.load(1, None);
    assert_eq!(
        refused(&unstageable, &mut group),
        SyncError::CapabilityUnavailable {
            capability: SyncCapability::Alignment,
        }
    );
}
