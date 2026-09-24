use kithara_events::TrackId;
use kithara_platform::{
    CancelToken,
    sync::{Arc, Mutex},
    tokio::{
        runtime::Handle,
        task::{spawn_blocking_on, spawn_on},
    },
};
use kithara_sync::{
    AlignmentSource, SyncAdmission, SyncCapability, SyncEffect, SyncError, SyncExecutionReject,
    SyncExecutionStamp, SyncGroup, SyncOperation, SyncPreparation, SyncReceipt, SyncRejected,
    SyncTransition,
};
use kithara_test_utils::kithara;
use kithara_warp::{BeatGridId, WarpPlan, supports_playback_rate};
use tracing::{debug, warn};

use crate::{
    PlayError,
    resource::{StageRequest, StagedLane, StagingError, StagingRecipe},
    session::SessionHandle,
    worker::{Readiness, ReadinessProbe},
};

/// The group owner a staged lane reports its outcome to.
pub(crate) trait ReceiptOwner: Send + Sync {
    /// Whether an owner is bound to take receipts at all.
    fn is_bound(&self) -> bool;

    /// Delivers one receipt; blocks until the owner answers.
    fn acknowledge(&self, receipt: SyncReceipt) -> Result<(), PlayError>;
}

impl<S: 'static> ReceiptOwner for SessionHandle<S>
where
    Self: Send + Sync,
{
    delegate::delegate! {
        to self {
            #[call(dispatcher)]
            #[expr($.is_ok())]
            fn is_bound(&self) -> bool;
            #[call(acknowledge_sync)]
            fn acknowledge(&self, receipt: SyncReceipt) -> Result<(), PlayError>;
        }
    }
}

struct Loaded {
    item: TrackId,
    recipe: Option<StagingRecipe>,
}

struct Held {
    stamp: SyncExecutionStamp,
    item: TrackId,
    cancel: CancelToken,
    handle: Handle,
    lane: Option<StagedLane>,
}

#[derive(Default)]
struct State {
    loaded: Option<Loaded>,
    held: Option<Held>,
}

struct Shared {
    track: BeatGridId,
    /// `None` where the platform has no Warp backend to stage on, so no
    /// preparation is ever admitted.
    owner: Option<Arc<dyn ReceiptOwner>>,
    cancel: CancelToken,
    response_frames: usize,
    state: Mutex<State>,
}

/// Carries out the preparations a player's group issues for its track: at
/// most one staged lane at a time, opened beside the sounding lane and
/// reported to the group owner once its prepared PCM is proven or refused.
///
/// Clones share one executor, so a player and the member the Host holds for
/// it follow the same preparations.
#[derive(Clone)]
pub(crate) struct SyncStaging(Arc<Shared>);

impl SyncStaging {
    pub(crate) fn new(
        track: BeatGridId,
        owner: Option<Arc<dyn ReceiptOwner>>,
        cancel: CancelToken,
        response_frames: usize,
    ) -> Self {
        Self(Arc::new(Shared {
            track,
            owner,
            cancel,
            response_frames,
            state: Mutex::default(),
        }))
    }

    /// Admits `operation` into `group` only if this executor can carry out
    /// the preparation it asks for, then follows what the group issued.
    ///
    /// # Errors
    ///
    /// Returns the group's own refusal, or the executor's when it cannot
    /// stage the track or has no owner to report to.
    pub(crate) fn transact<G: SyncGroup>(
        &self,
        group: &mut G,
        operation: SyncOperation<G::NestedGroup>,
    ) -> Result<SyncAdmission, SyncRejected<G::NestedGroup>> {
        if let Err(error) = self.admit(&operation) {
            return Err(SyncRejected::new(error, operation));
        }
        let relocation = matches!(operation, SyncOperation::Relocate { .. });
        let admission = group.transact(operation)?;
        match &admission {
            SyncAdmission::Prepared(preparation) => self.follow(preparation, relocation),
            SyncAdmission::TopologyChanged { transition, .. }
            | SyncAdmission::StateChanged { transition, .. } => self.follow_transition(transition),
            _ => {}
        }
        Ok(admission)
    }

    /// Follows the preparations one committed change withdrew and issued.
    pub(crate) fn follow_transition(&self, transition: &SyncTransition) {
        for stamp in transition.withdrawn() {
            self.withdraw(*stamp);
        }
        for preparation in transition.issued() {
            self.follow(preparation, false);
        }
    }

    /// The track this player now holds; a preparation staged for another
    /// load is dropped and reported cancelled.
    pub(crate) fn load(&self, item: TrackId, recipe: Option<StagingRecipe>) {
        let mut state = self.0.state.lock();
        let stale = state.held.take_if(|held| held.item != item);
        state.loaded = Some(Loaded { item, recipe });
        drop(state);
        self.cancel_reported(stale);
    }

    /// The player holds no track any more, or is closing.
    pub(crate) fn unload(&self) {
        let mut state = self.0.state.lock();
        state.loaded = None;
        let stale = state.held.take();
        drop(state);
        self.cancel_reported(stale);
    }

    fn admit<G: SyncGroup>(&self, operation: &SyncOperation<G>) -> Result<(), SyncError> {
        let staged = match operation {
            SyncOperation::Relocate { target, .. } => *target == self.0.track,
            SyncOperation::Prepare { target, source, .. } => {
                *target == self.0.track && !matches!(source, AlignmentSource::Audible { .. })
            }
            _ => false,
        };
        if !staged {
            return Ok(());
        }
        let unsupported = SyncError::CapabilityUnavailable {
            capability: SyncCapability::Alignment,
        };
        if !supports_playback_rate() {
            return Err(unsupported);
        }
        if !self.0.owner.as_ref().is_some_and(|owner| owner.is_bound()) {
            return Err(SyncError::OwnerUnavailable);
        }
        let unstageable = self
            .0
            .state
            .lock()
            .loaded
            .as_ref()
            .is_some_and(|loaded| loaded.recipe.is_none());
        if unstageable {
            return Err(unsupported);
        }
        Ok(())
    }

    fn follow(&self, preparation: &SyncPreparation, relocation: bool) {
        let stamp = preparation.stamp();
        if stamp.member().grid_id() != self.0.track {
            return;
        }
        let mut state = self.0.state.lock();
        if state.held.as_ref().is_some_and(|held| held.stamp == stamp) {
            return;
        }
        let superseded = state.held.take();
        let plan = match preparation.effect() {
            SyncEffect::Projection { plan, replaces, .. } if replaces.is_none() || relocation => {
                plan.clone()
            }
            _ => {
                drop(state);
                Self::cancel_silently(superseded);
                return;
            }
        };
        let Some((item, recipe)) = state.loaded.as_ref().and_then(|loaded| {
            let recipe = loaded.recipe.clone()?;
            Some((loaded.item, recipe))
        }) else {
            drop(state);
            Self::cancel_silently(superseded);
            warn!(?stamp, "sync: no loaded track to stage the preparation on");
            return;
        };
        let cancel = self.0.cancel.child();
        let handle = recipe.handle().clone();
        state.held = Some(Held {
            stamp,
            item,
            cancel: cancel.clone(),
            handle: handle.clone(),
            lane: None,
        });
        drop(state);
        Self::cancel_silently(superseded);
        drop(spawn_on(
            &handle,
            stage(Arc::clone(&self.0), recipe, stamp, plan, cancel),
        ));
    }

    fn withdraw(&self, stamp: SyncExecutionStamp) {
        let withdrawn = self.0.state.lock().held.take_if(|held| held.stamp == stamp);
        Self::cancel_silently(withdrawn);
    }

    fn cancel_silently(held: Option<Held>) {
        if let Some(held) = held {
            held.cancel.cancel();
        }
    }

    fn cancel_reported(&self, held: Option<Held>) {
        let Some(held) = held else {
            return;
        };
        held.cancel.cancel();
        let Held { stamp, handle, .. } = held;
        self.0
            .deliver(&handle, stamp, Some(SyncExecutionReject::Cancelled));
    }
}

impl Shared {
    /// Reports the outcome of the lane staged for `stamp`, unless that
    /// preparation was superseded, withdrawn, or unloaded meanwhile.
    fn settle(
        &self,
        stamp: SyncExecutionStamp,
        cancel: &CancelToken,
        outcome: Result<StagedLane, SyncExecutionReject>,
    ) {
        let mut state = self.state.lock();
        let Some(held) = state
            .held
            .as_mut()
            .filter(|held| held.stamp == stamp && !cancel.is_cancelled())
        else {
            return;
        };
        let handle = held.handle.clone();
        let rejected = match outcome {
            Ok(lane) => {
                held.lane = Some(lane);
                None
            }
            Err(reason) => {
                state.held = None;
                Some(reason)
            }
        };
        drop(state);
        self.deliver(&handle, stamp, rejected);
    }

    /// Hands the receipt for `stamp` to the owner off every caller's thread:
    /// the owner may be the very dispatcher that is running this executor's
    /// caller. `rejected` is the reason the lane was dropped, `None` for an
    /// installed lane.
    fn deliver(
        &self,
        handle: &Handle,
        stamp: SyncExecutionStamp,
        rejected: Option<SyncExecutionReject>,
    ) {
        let Some(owner) = self.owner.clone() else {
            return;
        };
        drop(spawn_blocking_on(handle, move || {
            let receipt = rejected.map_or(SyncReceipt::Installed(stamp), |reason| {
                SyncReceipt::Rejected { stamp, reason }
            });
            let answer = owner.acknowledge(receipt);
            kithara::probe_event!(
                sync_receipt_delivered,
                operation = u64::from(stamp.operation()),
                rejected = rejected.map_or(0, reject_code),
                accepted = u64::from(answer.is_ok())
            );
            if let Err(error) = answer {
                debug!(%error, "sync: the owner refused an executor receipt");
            }
        }));
    }
}

/// Probe code of a rejection; `0` stands for an installed lane.
const fn reject_code(reason: SyncExecutionReject) -> u64 {
    match reason {
        SyncExecutionReject::Geometry => 1,
        SyncExecutionReject::Late => 2,
        SyncExecutionReject::Capacity => 3,
        SyncExecutionReject::Cancelled => 4,
        SyncExecutionReject::Media => 5,
        _ => u64::MAX,
    }
}

async fn stage(
    shared: Arc<Shared>,
    recipe: StagingRecipe,
    stamp: SyncExecutionStamp,
    plan: WarpPlan,
    cancel: CancelToken,
) {
    let (probe, verdict) = ReadinessProbe::new(&plan, shared.response_frames);
    let request = StageRequest {
        plan,
        cancel: cancel.clone(),
        probe,
    };
    let outcome = match recipe.open(request).await {
        Ok(lane) => match verdict.await {
            Ok(Readiness::Ready) => Ok(lane),
            Ok(Readiness::Failed) => Err(SyncExecutionReject::Media),
            Err(_) => Err(SyncExecutionReject::Cancelled),
        },
        Err(StagingError::Capacity) => Err(SyncExecutionReject::Capacity),
        Err(StagingError::Cancelled) => Err(SyncExecutionReject::Cancelled),
        Err(StagingError::Media(error)) => {
            warn!(%error, ?stamp, "sync: the staged lane could not be opened");
            Err(SyncExecutionReject::Media)
        }
    };
    shared.settle(stamp, &cancel, outcome);
}
