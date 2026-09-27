use std::num::NonZeroU32;

use firewheel::FirewheelContext;
use kithara_play::{SessionSampleRate, StreamShape};
use kithara_signal::SessionFrame;
use kithara_sync::{
    ClockRefusal, EntryPort, GroupState, InboxAt, ProcessedTransport, RootPort, SyncReceiptInbox,
};
use kithara_warp::BeatGridId;

use super::{
    state::{GraphRegistry, RetiringSlot, RootView},
    transport::{self, SessionGridGeneration, SessionTransportState, TransportControl},
};
use crate::PlayerMember;

/// What the session's stream reports about itself, as its root publication
/// reads it.
pub(super) struct StreamFacts<'a> {
    pub(super) ctx: Option<&'a FirewheelContext>,
    pub(super) stream_needs_restart: bool,
    pub(super) requested_max_block_frames: Option<NonZeroU32>,
    pub(super) sample_rate_hint: u32,
}

impl StreamFacts<'_> {
    /// The shape of the stream the session is actually running on, if it is
    /// running on one. Firewheel keeps a deactivated context's stream
    /// description until the processor comes back, so a session awaiting a
    /// restart would otherwise keep reporting the route it has already
    /// disowned as measured.
    fn measured(&self) -> Option<StreamShape> {
        if self.stream_needs_restart {
            return None;
        }
        self.ctx
            .and_then(FirewheelContext::stream_info)
            .map(|info| StreamShape::new(info.max_block_frames, info.sample_rate))
    }

    pub(super) fn sample_rate(&self) -> SessionSampleRate {
        let measured = self.measured().map(|shape| shape.sample_rate.get());
        SessionSampleRate::new(measured, self.sample_rate_hint)
    }

    pub(super) fn shape(&self) -> Option<StreamShape> {
        self.measured().or_else(|| {
            Some(StreamShape::new(
                self.requested_max_block_frames?,
                NonZeroU32::new(self.sample_rate_hint)?,
            ))
        })
    }
}

/// The Host's side of an owner cut: the session fields the sync root reads
/// and drains, borrowed apart from the root itself.
pub(super) struct OwnerPort<'a, S> {
    pub(super) graph: &'a mut GraphRegistry<S>,
    pub(super) retiring: &'a mut Vec<RetiringSlot>,
    pub(super) transport: &'a mut SessionTransportState,
    pub(super) transport_control: &'a mut Option<TransportControl>,
    pub(super) reserved_session_grid: Option<SessionGridGeneration>,
    pub(super) stream: StreamFacts<'a>,
    pub(super) view: &'a RootView,
}

impl<S> RootPort<PlayerMember> for OwnerPort<'_, S> {
    delegate::delegate! {
        to self.graph {
            #[call(len)]
            fn decks(&self) -> usize;
            #[expr($.map_or(0, |deck| deck.slots.len()))]
            #[call(deck)]
            fn slots(&self, deck: usize) -> usize;
            #[expr($.is_some())]
            #[call(index_by_grid)]
            fn is_projected(&self, id: BeatGridId) -> bool;
        }
        to self.retiring {
            #[call(len)]
            fn retiring_len(&self) -> usize;
            #[expr($.group)]
            #[call(remove)]
            fn remove_retiring(&mut self, index: usize) -> BeatGridId;
        }
    }

    fn inbox(&mut self, at: InboxAt) -> Option<&mut SyncReceiptInbox> {
        let slot = match at {
            InboxAt::Live { deck, slot } => self
                .graph
                .deck_mut(deck)
                .and_then(|deck| deck.slots.get_mut(slot)),
            InboxAt::Retiring(index) => self
                .retiring
                .get_mut(index)
                .map(|retiring| &mut retiring.slot),
        };
        slot.map(|slot| &mut slot.sync_receipts)
    }

    fn is_quiesced(&self, group: BeatGridId) -> bool {
        self.retiring.iter().all(|retiring| retiring.group != group)
            && self
                .graph
                .index_by_grid(group)
                .and_then(|deck| self.graph.deck(deck))
                .is_some_and(|deck| deck.slots.is_empty())
    }

    fn publish(&self, root: &GroupState<PlayerMember>) {
        self.view
            .publish(root, self.stream.shape(), self.stream.sample_rate());
    }
}

impl<S> EntryPort for OwnerPort<'_, S> {
    fn processed(&mut self) -> Option<ProcessedTransport> {
        let snapshot = self.transport_control.as_mut()?.observation().snapshot()?;
        Some(
            ProcessedTransport::builder()
                .revision(snapshot.revision())
                .session_epoch(snapshot.session_epoch())
                .sample_rate(snapshot.session_grid().axis().sample_rate())
                .build(),
        )
    }

    fn commit_boundary(&self) -> Result<SessionFrame, ClockRefusal> {
        transport::clock_boundary(self.stream.ctx, self.transport).map(|(boundary, _)| boundary)
    }
}
