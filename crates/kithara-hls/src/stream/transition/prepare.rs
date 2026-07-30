use std::io::{Error as IoError, ErrorKind};

use kithara_abr::PendingAbrClaim;
use kithara_events::VariantIndex;
use kithara_platform::sync::Arc;
use kithara_stream::{
    ByteMap, MediaInfo, OpenedReader, OpenedVariantReader, ReaderProfile, SourceError, StreamError,
    StreamResult, VariantReaderPlan, VariantTransition, VariantTransitionId,
};

use super::{HlsCoord, IncomingSlot, unsupported_pending_claim};
use crate::{
    reader::HlsReaderEventSink,
    stream::{
        coord::variant_switch_target_time,
        session::{HlsSession, HlsSessionReader},
    },
};

impl HlsCoord {
    pub(in crate::stream) fn prepare_planned_variant_reader(
        &self,
        plan: VariantReaderPlan,
        profile: ReaderProfile,
    ) -> StreamResult<Option<VariantTransition>> {
        let mut state = self.sessions.transition.lock();
        if !state.exact {
            return Err(StreamError::Source(SourceError::Io(IoError::new(
                ErrorKind::InvalidInput,
                "exact variant sessions are not enabled",
            ))));
        }
        let transition = plan.transition();
        let claim = match self.abr.pending_claim() {
            PendingAbrClaim::Absent => {
                self.discard_incoming(&mut state, true);
                return Ok(None);
            }
            PendingAbrClaim::Locked(claim) => {
                if let Some(slot) = state.incoming.as_ref()
                    && slot.claim == claim
                {
                    if slot.transition != transition {
                        return Ok(None);
                    }
                    let epoch_matches = self.seek_observe().epoch() == transition.id().seek_epoch();
                    let active_matches = self.variant_index() == transition.active_variant().get();
                    if !epoch_matches || !active_matches {
                        self.discard_incoming(&mut state, false);
                        return Ok(None);
                    }
                    validate_hls_reader_plan(&plan, &slot.session.media_info())?;
                    if slot.profile != profile {
                        return Err(StreamError::Source(SourceError::Io(IoError::new(
                            ErrorKind::InvalidInput,
                            "reader profile changed within one variant transition",
                        ))));
                    }
                    return Ok(Some(slot.transition));
                }
                self.discard_incoming(&mut state, true);
                return Ok(None);
            }
            PendingAbrClaim::Ready(claim) => claim,
            _ => return Err(unsupported_pending_claim()),
        };
        let expected = VariantTransition::new(
            VariantTransitionId::new(claim.ticket(), self.seek_observe().epoch()),
            VariantIndex::new(self.variant_index()),
            claim.decision().target(),
        );
        if transition != expected {
            let abort_intent = state
                .incoming
                .as_ref()
                .filter(|slot| slot.transition == transition)
                .map(|slot| slot.claim != claim);
            if let Some(abort_intent) = abort_intent {
                self.discard_incoming(&mut state, abort_intent);
            }
            return Ok(None);
        }

        if let Some(slot) = state.incoming.as_ref()
            && slot.transition == transition
        {
            validate_hls_reader_plan(&plan, &slot.session.media_info())?;
            if slot.profile != profile {
                return Err(StreamError::Source(SourceError::Io(IoError::new(
                    ErrorKind::InvalidInput,
                    "reader profile changed within one variant transition",
                ))));
            }
            return Ok(Some(transition));
        }
        if state.incoming.is_some() {
            let abort_intent = state
                .incoming
                .as_ref()
                .is_some_and(|slot| slot.claim != claim);
            self.discard_incoming(&mut state, abort_intent);
        }

        let Some(target) = self
            .variants
            .get(transition.incoming_variant().get())
            .cloned()
        else {
            let _ = self.abr_publisher.abort_pending(claim.ticket());
            return Err(StreamError::Source(SourceError::VariantNotFound(format!(
                "incoming variant {}",
                transition.incoming_variant().get()
            ))));
        };
        validate_hls_reader_plan(&plan, &target.media_info())?;
        let session = match HlsSession::incoming(
            self.cancel.child(),
            profile,
            self.seek_observe(),
            self.signal(),
            transition,
            target,
            variant_switch_target_time(self.seek_observe().as_ref(), self.playhead_read().as_ref()),
        ) {
            Ok(session) => Arc::new(session),
            Err(error) => {
                let _ = self.abr_publisher.abort_pending(claim.ticket());
                return Err(error);
            }
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
            session.abort();
            if epoch_matches {
                let _ = self.abr_publisher.abort_pending(claim.ticket());
            }
            return Ok(None);
        }

        let reader = OpenedReader::new(
            HlsSessionReader::new(Arc::clone(&session)),
            session.len(),
            Some(Arc::clone(&session) as Arc<dyn ByteMap>),
            Some(Box::new(HlsReaderEventSink::for_session(
                Arc::clone(&self.emit),
                Arc::clone(&session),
                self.seek_epoch_handle(),
            ))),
        );
        let reader = OpenedVariantReader::new(plan, reader);
        let outgoing = self.active_session();
        self.sessions
            .publish_exact_two(outgoing, Arc::clone(&session));
        state.incoming = Some(IncomingSlot {
            claim,
            profile,
            reader: Some(reader),
            session,
            transition,
        });
        drop(state);
        self.signal().wake_peer();
        Ok(Some(transition))
    }
}

fn validate_hls_reader_plan(plan: &VariantReaderPlan, media_info: &MediaInfo) -> StreamResult<()> {
    if plan.media_info() == media_info && plan.base_offset() == 0 {
        return Ok(());
    }
    Err(StreamError::Source(SourceError::Io(IoError::new(
        ErrorKind::InvalidInput,
        "reader plan does not match the exact incoming variant",
    ))))
}
