mod prepare;

use std::io::{Error as IoError, ErrorKind};

use arc_swap::ArcSwap;
use kithara_abr::{PendingAbrClaim, PendingAbrDecision};
use kithara_events::VariantIndex;
use kithara_platform::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use kithara_stream::{
    OpenedVariantReader, ReaderProfile, SourceError, StreamError, StreamResult, VariantPromotion,
    VariantReaderPlan, VariantReaderTake, VariantTransition, VariantTransitionId,
};

use super::{
    coord::{HlsCoord, variant_switch_target_time},
    session::HlsSession,
};
pub(super) struct SessionSlots {
    publication: ArcSwap<ResidentSessions>,
    transition: Mutex<TransitionState>,
}

struct ResidentSessions {
    first: Arc<HlsSession>,
    second: Option<Arc<HlsSession>>,
}

impl ResidentSessions {
    fn one(session: Arc<HlsSession>) -> Self {
        Self {
            first: session,
            second: None,
        }
    }

    fn two(first: Arc<HlsSession>, second: Arc<HlsSession>) -> Self {
        Self {
            first,
            second: Some(second),
        }
    }

    fn find(&self, variant_index: usize) -> Option<&Arc<HlsSession>> {
        if self.first.variant_index() == variant_index {
            return Some(&self.first);
        }
        self.second
            .as_ref()
            .filter(|session| session.variant_index() == variant_index)
    }
}

impl SessionSlots {
    pub(super) fn new(active: Arc<HlsSession>) -> Self {
        Self {
            publication: ArcSwap::from_pointee(ResidentSessions::one(active)),
            transition: Mutex::new(TransitionState { incoming: None }),
        }
    }

    pub(super) fn active(&self, mut selected_variant: impl FnMut() -> usize) -> Arc<HlsSession> {
        loop {
            let before = selected_variant();
            let residents = self.publication.load();
            let resolved = residents.find(before);
            let after = selected_variant();
            if before != after {
                continue;
            }
            return resolved.map_or_else(
                || panic!("exact resident sessions do not contain published ABR variant {before}"),
                Arc::clone,
            );
        }
    }

    fn publish_exact_one(&self, session: Arc<HlsSession>) {
        self.publication
            .store(Arc::new(ResidentSessions::one(session)));
    }

    fn publish_exact_two(&self, first: Arc<HlsSession>, second: Arc<HlsSession>) {
        self.publication
            .store(Arc::new(ResidentSessions::two(first, second)));
    }

    #[cfg(test)]
    pub(super) fn resident_count(&self) -> usize {
        if self.publication.load().second.is_some() {
            2
        } else {
            1
        }
    }
}

struct TransitionState {
    incoming: Option<IncomingSlot>,
}

struct IncomingSlot {
    claim: PendingAbrDecision,
    landing_time: Duration,
    profile: ReaderProfile,
    reader: Option<OpenedVariantReader>,
    session: Arc<HlsSession>,
    transition: VariantTransition,
}

impl HlsCoord {
    fn discard_incoming(&self, state: &mut TransitionState, abort_intent: bool) {
        let Some(slot) = state.incoming.take() else {
            return;
        };
        let active = self.active_session();
        self.sessions.publish_exact_one(active);
        slot.session.abort();
        if abort_intent {
            let _ = self.abr_publisher.abort_pending(slot.claim.ticket());
        }
    }

    pub(super) fn plan_variant_reader(
        &self,
        landing: Option<Duration>,
    ) -> StreamResult<Option<VariantReaderPlan>> {
        let mut state = self.sessions.transition.lock();
        let claim = match self.abr.pending_claim() {
            PendingAbrClaim::Absent => {
                self.discard_incoming(&mut state, true);
                return Ok(None);
            }
            PendingAbrClaim::Locked(claim) => {
                if let Some(slot) = state.incoming.as_ref()
                    && slot.claim == claim
                {
                    let epoch_matches =
                        self.seek_observe().epoch() == slot.transition.id().seek_epoch();
                    let active_matches =
                        self.variant_index() == slot.transition.active_variant().get();
                    if !epoch_matches || !active_matches {
                        self.discard_incoming(&mut state, false);
                        return Ok(None);
                    }
                    return Ok(Some(VariantReaderPlan::new(
                        slot.transition,
                        slot.session.media_info(),
                        slot.landing_time,
                    )));
                }
                self.discard_incoming(&mut state, true);
                return Ok(None);
            }
            PendingAbrClaim::Ready(claim) => claim,
            _ => return Err(unsupported_pending_claim()),
        };
        let transition = VariantTransition::new(
            VariantTransitionId::new(claim.ticket(), self.seek_observe().epoch()),
            VariantIndex::new(self.variant_index()),
            claim.decision().target(),
        );

        if let Some(slot) = state.incoming.as_ref()
            && slot.transition == transition
        {
            return Ok(Some(VariantReaderPlan::new(
                transition,
                slot.session.media_info(),
                slot.landing_time,
            )));
        }
        if state.incoming.is_some() {
            let abort_intent = state
                .incoming
                .as_ref()
                .is_some_and(|slot| slot.claim != claim);
            self.discard_incoming(&mut state, abort_intent);
        }
        drop(state);

        let Some(target) = self.variants.get(transition.incoming_variant().get()) else {
            let _ = self.abr_publisher.abort_pending(claim.ticket());
            return Err(StreamError::Source(SourceError::VariantNotFound(format!(
                "incoming variant {}",
                transition.incoming_variant().get()
            ))));
        };
        let epoch_matches = self.seek_observe().epoch() == transition.id().seek_epoch();
        let claim_matches = match self.abr.pending_claim() {
            PendingAbrClaim::Ready(current) | PendingAbrClaim::Locked(current) => current == claim,
            _ => false,
        };
        if !epoch_matches
            || !claim_matches
            || self.variant_index() != transition.active_variant().get()
        {
            if epoch_matches {
                let _ = self.abr_publisher.abort_pending(claim.ticket());
            }
            return Ok(None);
        }

        let landing_time = variant_switch_target_time(
            self.seek_observe().as_ref(),
            self.playhead_read().as_ref(),
            landing,
        );
        Ok(Some(VariantReaderPlan::new(
            transition,
            target.media_info(),
            landing_time,
        )))
    }

    pub(super) fn take_prepared_variant_reader(
        &self,
        transition: VariantTransition,
    ) -> StreamResult<VariantReaderTake> {
        let mut state = self.sessions.transition.lock();
        let result = if let Some(slot) = state.incoming.as_mut() {
            if slot.transition != transition {
                Ok(VariantReaderTake::Stale)
            } else if self.seek_observe().epoch() != transition.id().seek_epoch() {
                self.discard_incoming(&mut state, false);
                Ok(VariantReaderTake::Stale)
            } else {
                match self.abr.pending_claim() {
                    PendingAbrClaim::Locked(claim)
                        if claim == slot.claim
                            && self.variant_index() == transition.active_variant().get() =>
                    {
                        Ok(VariantReaderTake::Preparing)
                    }
                    PendingAbrClaim::Locked(_) | PendingAbrClaim::Absent => {
                        self.discard_incoming(&mut state, true);
                        Ok(VariantReaderTake::Stale)
                    }
                    PendingAbrClaim::Ready(claim)
                        if claim != slot.claim
                            || self.variant_index() != transition.active_variant().get() =>
                    {
                        self.discard_incoming(&mut state, true);
                        Ok(VariantReaderTake::Stale)
                    }
                    PendingAbrClaim::Ready(_) => match slot.session.is_ready() {
                        Ok(false) => Ok(VariantReaderTake::Preparing),
                        Ok(true) => Ok(slot
                            .reader
                            .take()
                            .map_or(VariantReaderTake::Taken, VariantReaderTake::Ready)),
                        Err(error) => {
                            self.discard_incoming(&mut state, true);
                            Err(error)
                        }
                    },
                    _ => Err(unsupported_pending_claim()),
                }
            }
        } else {
            Ok(VariantReaderTake::Stale)
        };
        drop(state);
        result
    }

    pub(super) fn promote_planned_variant(
        &self,
        transition: VariantTransition,
    ) -> VariantPromotion {
        let mut state = self.sessions.transition.lock();
        let Some(slot) = state.incoming.as_ref() else {
            return VariantPromotion::Stale;
        };
        if slot.transition != transition {
            return VariantPromotion::Stale;
        }
        if self.seek_observe().epoch() != transition.id().seek_epoch() {
            self.discard_incoming(&mut state, false);
            return VariantPromotion::Stale;
        }
        match self.abr.pending_claim() {
            PendingAbrClaim::Locked(claim) if claim == slot.claim => {
                return VariantPromotion::Deferred;
            }
            PendingAbrClaim::Locked(_) | PendingAbrClaim::Absent => {
                self.discard_incoming(&mut state, true);
                return VariantPromotion::Stale;
            }
            PendingAbrClaim::Ready(claim)
                if claim != slot.claim
                    || self.variant_index() != transition.active_variant().get() =>
            {
                self.discard_incoming(&mut state, true);
                return VariantPromotion::Stale;
            }
            PendingAbrClaim::Ready(_) => {}
            _ => return VariantPromotion::Stale,
        }
        if slot.reader.is_some() {
            return VariantPromotion::Deferred;
        }
        let Some(slot) = state.incoming.take() else {
            return VariantPromotion::Stale;
        };
        let outgoing = self.active_session();
        let now = Instant::now();
        let committed = self.commit_if_seek_epoch(transition.id().seek_epoch(), || {
            if !self.abr_publisher.commit_pending(slot.claim, now) {
                return false;
            }
            slot.session.activate();
            self.sessions.publish_exact_one(Arc::clone(&slot.session));
            outgoing.abort();
            true
        });
        match committed {
            None => {
                self.sessions.publish_exact_one(outgoing);
                slot.session.abort();
                return VariantPromotion::Stale;
            }
            Some(false) => {
                let claim_matches = match self.abr.pending_claim() {
                    PendingAbrClaim::Ready(current) | PendingAbrClaim::Locked(current) => {
                        current == slot.claim
                    }
                    _ => false,
                };
                let epoch_matches = self.seek_observe().epoch() == transition.id().seek_epoch();
                let active_matches = self.variant_index() == transition.active_variant().get();
                if claim_matches && epoch_matches && active_matches {
                    state.incoming = Some(slot);
                    return VariantPromotion::Deferred;
                }
                self.sessions.publish_exact_one(outgoing);
                slot.session.abort();
                return VariantPromotion::Stale;
            }
            Some(true) => {}
        }
        drop(state);
        self.abr
            .notify_exact_commit(slot.claim.decision(), transition.active_variant().get());
        self.signal().fire();
        VariantPromotion::Promoted
    }

    pub(super) fn abort_variant(&self, transition: VariantTransition) -> bool {
        let mut state = self.sessions.transition.lock();
        if state
            .incoming
            .as_ref()
            .is_none_or(|slot| slot.transition != transition)
        {
            return false;
        }
        self.discard_incoming(&mut state, true);
        drop(state);
        true
    }

    pub(super) fn cancel_incoming_for_seek(&self) {
        let mut state = self.sessions.transition.lock();
        self.discard_incoming(&mut state, false);
    }

    /// Fetches for the variant that is audible right now.
    pub(crate) fn dispatch_active(
        &self,
        ctx: &crate::variant::PlanCtx,
        budget: usize,
    ) -> Vec<kithara_stream::dl::FetchCmd> {
        self.active_session().dispatch(ctx, budget)
    }

    /// Fetches for the variant a switch is preparing.
    ///
    /// Carries no command priority, exactly like the active session's fetches.
    /// The peer already decides how much of each poll's budget preparation may
    /// take; a priority tag here would overrule that decision inside the
    /// downloader's queue — tagging preparation `High` starves the audible
    /// stream, tagging it `Low` starves the switch behind an active variant
    /// that never stops prefetching.
    pub(crate) fn dispatch_incoming(
        &self,
        ctx: &crate::variant::PlanCtx,
        budget: usize,
    ) -> Vec<kithara_stream::dl::FetchCmd> {
        let session = self
            .sessions
            .transition
            .lock()
            .incoming
            .as_ref()
            .map(|slot| Arc::clone(&slot.session));
        session.map_or_else(Vec::new, |session| session.dispatch(ctx, budget))
    }

    pub(crate) fn has_incoming(&self) -> bool {
        self.sessions.transition.lock().incoming.is_some()
    }

    pub(crate) fn active_session(&self) -> Arc<HlsSession> {
        self.sessions.active(|| {
            self.abr
                .current_variant_index()
                .unwrap_or_else(|| panic!("HLS coordinator lost its stateful ABR selector"))
        })
    }
}

fn unsupported_pending_claim() -> StreamError {
    StreamError::Source(SourceError::Io(IoError::new(
        ErrorKind::InvalidData,
        "unsupported pending ABR claim state",
    )))
}
