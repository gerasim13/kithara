use kithara_platform::{sync::Arc, time::Duration};
use kithara_warp::BeatGridId;
use tracing::warn;

use super::{EnteredCut, InboxAt, RootError, RootPort, inputs::queue_gate_failure};
use crate::{
    ControlEnterError, GroupState, PermitCell, SyncArbiter, SyncGateBinding, SyncGroup,
    SyncReceipt, SyncReceiptInbox,
};

/// How long entering the root waits for an audio claim in flight before it
/// reports the owner busy.
pub const DEFAULT_OWNER_WAIT: Duration = Duration::from_millis(18);

/// The session root's owner policy.
#[derive(Clone, Copy, Debug, Eq, PartialEq, bon::Builder)]
pub struct SyncRootConfig {
    /// How long entering the root waits for an audio claim in flight.
    #[builder(default = DEFAULT_OWNER_WAIT)]
    owner_wait: Duration,
}

/// One attached member's permit cell, and the terminal gate rejection the
/// owner keeps for it when an install reached a busy owner.
pub(crate) struct RegisteredCell {
    pub(super) group: BeatGridId,
    pub(super) cell: Arc<PermitCell>,
    pub(super) pending_gate_receipt: Option<SyncReceipt>,
}

impl RegisteredCell {
    /// The deck group the member plays in.
    #[must_use]
    pub(crate) const fn group(&self) -> BeatGridId {
        self.group
    }

    /// The track member the cell gates.
    #[must_use]
    pub(crate) fn member(&self) -> BeatGridId {
        self.cell.member()
    }
}

/// The session's root group, the gate it shares with every audio callback,
/// and the member cells behind that gate. The root changes only inside an
/// owner cut this value enters.
pub struct SyncRoot<G: SyncGroup<NestedGroup = G>> {
    group: GroupState<G>,
    arbiter: Arc<SyncArbiter>,
    cells: Vec<RegisteredCell>,
    config: SyncRootConfig,
}

impl<G: SyncGroup<NestedGroup = G>> SyncRoot<G> {
    /// Owns `group` as the session root behind a fresh open gate.
    #[must_use]
    pub fn new(group: GroupState<G>, config: SyncRootConfig) -> Self {
        Self {
            group,
            config,
            arbiter: Arc::new(SyncArbiter::new()),
            cells: Vec::new(),
        }
    }

    /// The root group as the last cut left it.
    #[must_use]
    pub const fn group(&self) -> &GroupState<G> {
        &self.group
    }

    /// Every registered member cell.
    #[must_use]
    pub(super) fn cells(&self) -> &[RegisteredCell] {
        &self.cells
    }

    /// Registers the cell of `member`, which plays in the deck `group`, and
    /// returns the gate its player's audio callback claims through.
    ///
    /// # Errors
    ///
    /// Returns [`RootError::MemberAlreadyRegistered`] when the deck or the
    /// member already has a cell.
    pub fn register(
        &mut self,
        group: BeatGridId,
        member: BeatGridId,
    ) -> Result<SyncGateBinding, RootError> {
        if self
            .cells
            .iter()
            .any(|entry| entry.group == group || entry.member() == member)
        {
            return Err(RootError::MemberAlreadyRegistered(member));
        }
        let cell = Arc::new(PermitCell::new(member));
        self.cells.push(RegisteredCell {
            group,
            cell: Arc::clone(&cell),
            pending_gate_receipt: None,
        });
        Ok(SyncGateBinding::new(Arc::clone(&self.arbiter), cell))
    }

    /// The gate of the member registered under `group`: its source revision
    /// stamps the render evidence of that deck's slots.
    #[must_use]
    pub fn gate(&self, group: BeatGridId) -> Option<SyncGateBinding> {
        self.cells
            .iter()
            .find(|entry| entry.group == group)
            .map(|entry| SyncGateBinding::new(Arc::clone(&self.arbiter), Arc::clone(&entry.cell)))
    }

    /// Enters an owner cut once the audio claim in flight completes.
    ///
    /// # Errors
    ///
    /// Returns `Busy` when the claim outlasts the configured owner wait, and
    /// `Closed` after [`Self::close`].
    pub fn enter(&mut self) -> Result<EnteredCut<'_, G>, ControlEnterError> {
        let Self {
            group,
            arbiter,
            cells,
            config,
        } = self;
        arbiter
            .enter_host_control(config.owner_wait)
            .map(|control| EnteredCut::new(control, group, cells))
    }

    /// Enters an owner cut to acknowledge an executor receipt. An owner too
    /// busy to take an install keeps its terminal rejection for the next cut
    /// instead, and the executor drops that lane.
    ///
    /// # Errors
    ///
    /// Returns `Enter(Busy)` once that rejection is kept, the reason it could
    /// not be kept, or `Enter(Closed)` after [`Self::close`].
    pub fn enter_to_acknowledge(
        &mut self,
        receipt: SyncReceipt,
    ) -> Result<EnteredCut<'_, G>, RootError> {
        let Self {
            group,
            arbiter,
            cells,
            config,
        } = self;
        let error = match arbiter.enter_host_control(config.owner_wait) {
            Ok(control) => return Ok(EnteredCut::new(control, group, cells)),
            Err(error) => error,
        };
        if error == ControlEnterError::Busy {
            queue_gate_failure(cells, receipt)?;
        }
        Err(RootError::Enter(error))
    }

    /// The owner's last word, once the audio callback and every command have
    /// quiesced: records what each inbox still holds and every kept gate
    /// rejection, publishes the root once, and closes the gate for good. A
    /// receipt the root refuses now is only logged, since nothing can retry
    /// it.
    pub fn close<P: RootPort<G>>(&mut self, port: &mut P) {
        for deck in 0..port.decks() {
            for slot in 0..port.slots(deck) {
                self.record_final(port.inbox(InboxAt::Live { deck, slot }));
            }
        }
        for index in 0..port.retiring_len() {
            self.record_final(port.inbox(InboxAt::Retiring(index)));
        }
        for cell in &mut self.cells {
            if let Some(receipt) = cell.pending_gate_receipt.take()
                && let Err(error) = self.group.acknowledge(receipt)
            {
                warn!(
                    ?error,
                    ?receipt,
                    "final gate rejection could not be recorded"
                );
            }
        }
        port.publish(&self.group);
        self.arbiter.close_quiescent();
    }

    fn record_final(&mut self, inbox: Option<&mut SyncReceiptInbox>) {
        let Some(inbox) = inbox else {
            return;
        };
        while let Some(receipt) = inbox.next_receipt() {
            if let Err(error) = self.group.acknowledge(receipt) {
                warn!(?error, ?receipt, "final sync receipt could not be recorded");
            }
        }
    }
}
