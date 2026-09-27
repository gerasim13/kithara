use std::ops::Range;

use kithara_warp::{PresentationFrontier, RenderContext};
use ringbuf::{
    HeapCons,
    traits::{Consumer, Observer},
};

use super::{
    ActivationAudio, ActivationDeck, ActivationResident, PreparedFirst, SyncAttempt, SyncKind,
    SyncReturn, SyncTicket, TrackDisposal, custody::ReturnCustody,
};
use crate::{
    AppliedSource, SourceRevision, SyncApplied, SyncExecutionReject, SyncGateBinding,
    SyncReceiptTx,
    execution::arbiter::{ClaimError, PermitState},
};

/// The audio-thread owner of one deck's activation: the pending ticket, the
/// return custody, the fading tail, the receipt producer and the member
/// gate.
pub struct SyncCallback<K: SyncKind> {
    pending: HeapCons<SyncTicket<K::Item, K::Lane>>,
    custody: ReturnCustody<K>,
    tail: Option<K::Tail>,
    receipts: Option<SyncReceiptTx>,
    gate: Option<SyncGateBinding>,
}

/// The member source one block renders, read before the block drains its
/// commands: every change committed up to it is already queued.
#[must_use]
pub struct BlockSource(Option<SourceRevision>);

impl BlockSource {
    /// Publish the block's source once its render evidence is out, when the
    /// block drained its commands; clear the applied source when the block
    /// did not sound, since the evidence of an earlier block no longer does.
    pub fn finish(self, applied: &AppliedSource, drained: bool, sounded: bool) {
        if !sounded {
            applied.clear();
        } else if drained && let Some(source) = self.0 {
            applied.publish(source);
        }
    }
}

impl<K: SyncKind> SyncCallback<K> {
    /// The callback of the deck whose audio half is `audio`, writing
    /// `receipts` and stamping its evidence with `gate`'s source.
    #[must_use]
    pub fn new(
        audio: ActivationAudio<K>,
        receipts: Option<SyncReceiptTx>,
        gate: Option<SyncGateBinding>,
    ) -> Self {
        Self {
            pending: audio.pending,
            custody: ReturnCustody::new(audio.returns),
            tail: None,
            receipts,
            gate,
        }
    }

    /// Read the member's source before the block drains its commands.
    pub fn begin_block(&self) -> BlockSource {
        BlockSource(self.gate.as_ref().map(SyncGateBinding::source_revision))
    }

    /// Move the held return into the ring, then return a settled tail.
    pub fn maintain(&mut self) {
        self.custody.flush_held();
        if self.tail.as_ref().is_some_and(K::settled)
            && let Some(room) = self.custody.room()
            && let Some(tail) = self.tail.take()
        {
            room.put(SyncReturn::Tail(tail));
        }
    }

    /// Whether one more return fits.
    #[must_use]
    pub fn can_return(&self) -> bool {
        self.custody.can_return()
    }

    /// How `track` can leave the callback now, or `None` while it holds a
    /// lane and custody is full.
    pub fn disposal(&mut self, track: &K::Track) -> Option<TrackDisposal<'_, K>> {
        if K::holds_lane(track) {
            self.custody.room().map(TrackDisposal::Return)
        } else {
            Some(TrackDisposal::Trash)
        }
    }

    /// Hand back everything the callback holds once its deck is cleared:
    /// the tail now, settled or not, then the pending ticket, rejected
    /// `Cancelled` while its permit is current. A withdrawn permit is the
    /// owner's already; a parked one is withdrawn by the source change that
    /// parked it. What does not fit stays for the next call.
    pub fn retire(&mut self) {
        if let Some(room) = self.custody.room()
            && let Some(tail) = self.tail.take()
        {
            room.put(SyncReturn::Tail(tail));
        }
        let Some(room) = self.custody.room() else {
            return;
        };
        let Some(ticket) = self.pending.try_peek() else {
            return;
        };
        if ticket.gate().permit_state(&ticket.permit()) == PermitState::Current {
            let Some(receipts) = self.receipts.as_mut() else {
                return;
            };
            if receipts
                .publish_rejected(ticket.permit().stamp(), SyncExecutionReject::Cancelled)
                .is_err()
            {
                return;
            }
        }
        if let Some(ticket) = self.pending.try_pop() {
            room.put(SyncReturn::Ticket(ticket.into()));
        }
    }

    /// Whether the callback holds no tail, pending ticket or held return.
    #[must_use]
    pub fn custody_cleared(&self) -> bool {
        self.tail.is_none() && self.pending.is_empty() && self.custody.held_is_empty()
    }

    /// Attempt the pending activation in one playing block of `frames`, and
    /// fade the tail out unless the attempt claimed.
    pub fn attempt<D: ActivationDeck<K>>(
        &mut self,
        deck: &mut D,
        context: Option<&RenderContext>,
        frames: usize,
    ) -> SyncAttempt<K::Item, D::Outcome> {
        self.return_settled_tail();
        let attempt = self.try_activation(deck, context, frames);
        if !matches!(attempt, SyncAttempt::Claimed { .. }) {
            self.fade_tail(deck, context, frames);
        }
        attempt
    }

    /// Fade the tail out over one block of `frames` that attempts nothing.
    pub fn fade<D: ActivationDeck<K>>(
        &mut self,
        deck: &mut D,
        context: Option<&RenderContext>,
        frames: usize,
    ) {
        self.return_settled_tail();
        self.fade_tail(deck, context, frames);
    }

    fn fade_tail<D: ActivationDeck<K>>(
        &mut self,
        deck: &mut D,
        context: Option<&RenderContext>,
        frames: usize,
    ) {
        let (Some(context), Some(tail)) = (context, self.tail.as_mut()) else {
            return;
        };
        if !K::settled(tail) {
            deck.render_tail(tail, context, 0..frames);
        }
        self.return_settled_tail();
    }

    fn return_settled_tail(&mut self) {
        if !self.tail.as_ref().is_some_and(K::settled) {
            return;
        }
        let Some(room) = self.custody.ring_room() else {
            return;
        };
        if let Some(tail) = self.tail.take() {
            room.put(SyncReturn::Tail(tail));
        }
    }

    fn reject(&mut self, reason: SyncExecutionReject) {
        let Some(room) = self.custody.ring_room() else {
            return;
        };
        let Some(ticket) = self.pending.try_peek() else {
            return;
        };
        let Some(receipts) = self.receipts.as_mut() else {
            return;
        };
        if receipts
            .publish_rejected(ticket.permit().stamp(), reason)
            .is_err()
        {
            return;
        }
        if let Some(ticket) = self.pending.try_pop() {
            room.put(SyncReturn::Ticket(ticket.into()));
        }
    }

    fn retire_withdrawn(&mut self) {
        let Some(room) = self.custody.ring_room() else {
            return;
        };
        if let Some(ticket) = self.pending.try_pop() {
            room.put(SyncReturn::Ticket(ticket.into()));
        }
    }

    /// Check the pending ticket against this block, in order: a parked
    /// source waits before Late is judged (the owner withdraws the ticket
    /// after the change, or the change aborts and the ticket stays), then
    /// the context, epoch, processed transport, activation window, capacity
    /// and the resident's load, lead and rate.
    fn try_activation<D: ActivationDeck<K>>(
        &mut self,
        deck: &mut D,
        context: Option<&RenderContext>,
        frames: usize,
    ) -> SyncAttempt<K::Item, D::Outcome> {
        let Some(ticket) = self.pending.try_peek() else {
            return SyncAttempt::None;
        };
        match ticket.gate().permit_state(&ticket.permit()) {
            PermitState::Current => {}
            PermitState::Parked => return SyncAttempt::None,
            PermitState::Withdrawn => {
                self.retire_withdrawn();
                return SyncAttempt::None;
            }
        }
        let Some(context) = context else {
            self.reject(SyncExecutionReject::Geometry);
            return SyncAttempt::None;
        };
        let output = context.output();
        let start = i64::from(output.output_frames().start);
        let end = i64::from(output.output_frames().end);
        let head = ticket.first().head();
        if output.session_epoch() != head.epoch() {
            self.reject(SyncExecutionReject::Cancelled);
            return SyncAttempt::None;
        }
        if ticket
            .permit()
            .stamp()
            .output_transport()
            .is_some_and(|revision| output.transport_revision() != Some(revision))
        {
            return SyncAttempt::None;
        }
        let activation = i64::from(head.activation().output());
        if activation >= end {
            return SyncAttempt::None;
        }
        if activation < start {
            self.reject(SyncExecutionReject::Late);
            return SyncAttempt::None;
        }
        let Ok(offset) = usize::try_from(activation - start) else {
            self.reject(SyncExecutionReject::Geometry);
            return SyncAttempt::None;
        };
        if offset >= frames || self.tail.is_some() || !self.custody.ring_is_empty() {
            self.reject(SyncExecutionReject::Capacity);
            return SyncAttempt::None;
        }
        let (item, load) = (ticket.item(), ticket.load());
        let resident = if output.sample_rate() == head.output_rate() {
            deck.resident(item, context)
                .filter(|resident| resident.serves(load, head.output_rate()))
        } else {
            None
        };
        let Some(resident) = resident else {
            self.reject(SyncExecutionReject::Geometry);
            return SyncAttempt::None;
        };
        self.claim(resident, context, offset..frames)
    }

    /// Reserve both receipts and render the resident's prefix before
    /// claiming the first frame; a losing claim resumes that resident at the
    /// offset without replaying prefix audio. The tail and the rest of the
    /// new lane render after the claim is released.
    ///
    /// The one ticket is popped only after a won claim: popping earlier would
    /// free the one-slot ring the control side reads as Capacity. The sole
    /// consumer peeked it in this call, and nothing else can take it.
    fn claim<R: ActivationResident<K>>(
        &mut self,
        mut resident: R,
        context: &RenderContext,
        window: Range<usize>,
    ) -> SyncAttempt<K::Item, R::Outcome> {
        let offset = window.start;
        let frames = window.end;
        let Some(first_context) = context.for_output_range(offset..offset + 1) else {
            self.reject(SyncExecutionReject::Geometry);
            return SyncAttempt::None;
        };
        if context.for_output_range(offset..frames).is_none() {
            self.reject(SyncExecutionReject::Geometry);
            return SyncAttempt::None;
        }
        let Some(ticket) = self.pending.try_peek() else {
            return SyncAttempt::None;
        };
        let first = ticket.first();
        let applied = SyncApplied::builder()
            .stamp(ticket.permit().stamp())
            .frontier(
                PresentationFrontier::builder()
                    .source(first.source().end())
                    .output(first_context.output().output_frames().end)
                    .build()
                    .with_warp_map(Some(first.head().activation().revision())),
            )
            .build();
        let Some(receipts) = self.receipts.as_mut() else {
            return SyncAttempt::None;
        };
        let Some(reservation) = receipts.reserve_pair(applied) else {
            return SyncAttempt::None;
        };
        let gate = ticket.gate().clone();
        let permit = ticket.permit();
        let item_id = ticket.item();

        let prefix = (offset > 0).then(|| resident.render(0..offset));
        if !resident.leads_after(prefix.as_ref()) {
            drop(reservation);
            self.reject(SyncExecutionReject::Late);
            return SyncAttempt::PrefixRendered {
                item_id,
                offset,
                outcome: prefix,
            };
        }
        let claim = match gate.arbiter().try_claim(&permit, gate.cell(), reservation) {
            Ok(claim) => claim,
            Err(ClaimError::SourceParked) => {
                return SyncAttempt::PrefixRendered {
                    item_id,
                    offset,
                    outcome: prefix,
                };
            }
            Err(ClaimError::StalePermit | ClaimError::CellRetired) => {
                self.retire_withdrawn();
                return SyncAttempt::PrefixRendered {
                    item_id,
                    offset,
                    outcome: prefix,
                };
            }
            Err(error) => {
                self.reject(claim_rejection(error));
                return SyncAttempt::PrefixRendered {
                    item_id,
                    offset,
                    outcome: prefix,
                };
            }
        };

        let Some(ticket) = self.pending.try_pop() else {
            unreachable!("the claimed sync ticket was removed without a consumer");
        };
        let (lane, first): (K::Lane, PreparedFirst) = ticket.into();
        let mut old = resident.activate(lane, &first, &first_context, offset);
        claim.finish();

        resident.render_tail(&mut old, offset..frames);
        let (outcome, handover_offset) = resident.finish(offset + 1..frames);
        self.tail = Some(old);
        self.return_settled_tail();
        SyncAttempt::Claimed {
            item_id,
            outcome,
            handover_offset,
        }
    }
}

const fn claim_rejection(error: ClaimError) -> SyncExecutionReject {
    match error {
        ClaimError::Busy => SyncExecutionReject::Late,
        ClaimError::WrongMember => SyncExecutionReject::Geometry,
        ClaimError::Closed
        | ClaimError::CellRetired
        | ClaimError::StalePermit
        | ClaimError::SourceParked => SyncExecutionReject::Cancelled,
    }
}
