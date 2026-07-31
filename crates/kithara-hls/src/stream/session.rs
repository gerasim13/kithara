mod cancel;
mod reader;

use std::{
    io::{self, ErrorKind},
    ops::Range,
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
};

use kithara_platform::{
    CancelToken,
    sync::{Arc, Mutex},
    time::Duration,
};
use kithara_storage::WaitOutcome;
use kithara_stream::{
    ByteMap, ConstructionGate, PendingReason, ReaderProfile, SeekObserve, SegmentDescriptor,
    SourceError, SourceSeekAnchor, StreamError, StreamResult, VariantTransition,
};

use self::cancel::SessionCancel;
pub(super) use self::reader::HlsSessionReader;
use super::coord::HlsCoord;
use crate::{
    signal::SizeSignal,
    variant::{HlsVariant, ResolvedSeekProjection, VariantReaderPreparation},
};

pub(crate) struct HlsSession {
    active: AtomicBool,
    cancel: SessionCancel,
    construction_gate: ConstructionGate,
    position: AtomicU64,
    readiness: SessionReadiness,
    seek: Arc<dyn SeekObserve>,
    signal: SizeSignal,
    transition: Option<VariantTransition>,
    variant: Arc<HlsVariant>,
    variant_index: usize,
}

enum SessionReadiness {
    Active,
    Profiled {
        preparation: Mutex<VariantReaderPreparation>,
        profile: ReaderProfile,
    },
}

#[derive(Clone, Copy)]
struct SessionPosition {
    byte: u64,
    projection: Option<ResolvedSeekProjection>,
}

impl HlsSession {
    pub(crate) fn active(
        cancel: CancelToken,
        seek: Arc<dyn SeekObserve>,
        signal: SizeSignal,
        variant_index: usize,
        variant: Arc<HlsVariant>,
        position: u64,
    ) -> Self {
        Self {
            active: AtomicBool::new(true),
            cancel: SessionCancel::new(cancel),
            construction_gate: ConstructionGate::default(),
            position: AtomicU64::new(position),
            readiness: SessionReadiness::Active,
            seek,
            signal,
            transition: None,
            variant,
            variant_index,
        }
    }

    pub(crate) fn incoming(
        cancel: CancelToken,
        profile: ReaderProfile,
        seek: Arc<dyn SeekObserve>,
        signal: SizeSignal,
        transition: VariantTransition,
        variant: Arc<HlsVariant>,
        content_time: Duration,
    ) -> StreamResult<Self> {
        let preparation = variant.prepare_reader(profile, content_time)?;
        Ok(Self {
            active: AtomicBool::new(false),
            cancel: SessionCancel::new(cancel),
            construction_gate: ConstructionGate::default(),
            position: AtomicU64::new(0),
            readiness: SessionReadiness::Profiled {
                preparation: Mutex::new(preparation),
                profile,
            },
            seek,
            signal,
            transition: Some(transition),
            variant,
            variant_index: transition.incoming_variant().get(),
        })
    }

    pub(crate) fn abort(&self) {
        self.cancel.abort();
    }

    pub(crate) fn arm_peer(&self) {
        self.signal.arm_peer();
    }

    pub(crate) fn construction_gate(&self) -> ConstructionGate {
        self.construction_gate.clone()
    }

    pub(crate) fn construction_blocking(&self) -> bool {
        self.construction_gate.is_armed()
    }

    /// Session-scoped twin of [`HlsCoord::wait_range`]: `Some(_)` is the
    /// wake-free RT probe, `None` the off-RT construction wait that parks on the
    /// readiness gate. The variant plans nothing by itself — the reader driver
    /// wakes the peer for the range — so the blocking probe notifies the peer
    /// before parking, the same consumer-wake shape `Stream::read` uses off-RT.
    pub(crate) fn wait_range(
        &self,
        range: Range<u64>,
        timeout: Option<Duration>,
    ) -> StreamResult<WaitOutcome> {
        match timeout {
            Some(_) => self.variant.wait_range(range, timeout),
            None => HlsCoord::wait_range_blocking(&self.signal, &self.cancel.root, || {
                let outcome = self.variant.wait_range(range.clone(), Some(Duration::ZERO));
                if matches!(
                    outcome,
                    Err(StreamError::Source(SourceError::WaitBudgetExceeded))
                ) {
                    self.signal.wake_peer();
                }
                outcome
            }),
        }
    }

    /// Fetches for a session that owns a live reader.
    ///
    /// Bounded by that reader's own position and the peer's look-ahead, the
    /// same way the audible session is bounded.
    pub(crate) fn dispatch(
        &self,
        ctx: &crate::variant::PlanCtx,
        budget: usize,
    ) -> Vec<kithara_stream::dl::FetchCmd> {
        self.dispatch_capped(ctx, budget, None)
    }

    /// Fetches for an incoming session whose reader has not been handed to a
    /// decoder yet.
    ///
    /// Capped at the construction window, because until the transfer the
    /// session has no reader position to follow and would otherwise queue the
    /// whole variant against the audible one. The cap ends at the transfer:
    /// that window is sized to *build* a decoder, not to feed one, and a
    /// priming decoder that must stage seconds of audio starves behind it —
    /// its reads stop being served, the staged span stops growing, and the
    /// outgoing frontier it is chasing walks away for good.
    pub(crate) fn dispatch_constructing(
        &self,
        ctx: &crate::variant::PlanCtx,
        budget: usize,
    ) -> Vec<kithara_stream::dl::FetchCmd> {
        let cap = match &self.readiness {
            SessionReadiness::Active => None,
            SessionReadiness::Profiled { preparation, .. } => {
                Some(self.variant.construction_segment_end(&preparation.lock()))
            }
        };
        self.dispatch_capped(ctx, budget, cap)
    }

    fn dispatch_capped(
        &self,
        ctx: &crate::variant::PlanCtx,
        budget: usize,
        construction_segment_end: Option<u32>,
    ) -> Vec<kithara_stream::dl::FetchCmd> {
        if self.cancel.is_cancelled() {
            return Vec::new();
        }
        // A session that is not yet audible is never speculating: every byte it
        // asks for is owed to a decoder being built or primed for a switch the
        // user asked for. The tag has to outlive the construction window — a
        // priming decoder starves behind the audible variant's look-ahead
        // exactly the way a constructing one does.
        //
        // An unresolved seek is the same shape on the audible session: until the
        // reader lands, the queue head is the target the user asked for, not
        // look-ahead. It reverts to speculation the moment the projection
        // retires.
        let demand = !self.active.load(Ordering::Acquire) || self.variant.seek_projection_is_live();
        self.variant.dispatch_from(
            ctx,
            budget,
            self.projected_position().byte,
            construction_segment_end,
            demand,
            self.cancel.handle(),
        )
    }

    pub(crate) fn find_at_offset(&self, byte: u64) -> Option<(u32, u64, u64)> {
        self.variant.find_at_offset(byte)
    }

    pub(crate) fn len(&self) -> Option<u64> {
        self.variant.stream_len()
    }

    /// Whether the construction window this session was prepared for is
    /// readable. The variant plans nothing by itself, so a not-yet-ready answer
    /// wakes the peer for the same reason a not-ready read does: without it an
    /// incoming session is only serviced when the *active* session happens to
    /// ask for bytes, and a switch requested while the active one is fully
    /// cached would never be fed. Off-RT — this is the transition shell, not the
    /// produce core.
    pub(crate) fn is_ready(&self) -> StreamResult<bool> {
        match &self.readiness {
            SessionReadiness::Active => Ok(true),
            SessionReadiness::Profiled { preparation, .. } => {
                let ready = self.variant.reader_is_ready(&preparation.lock())?;
                if !ready {
                    self.signal.wake_peer();
                }
                Ok(ready)
            }
        }
    }

    pub(crate) fn media_info(&self) -> kithara_stream::MediaInfo {
        self.variant.media_info()
    }

    pub(crate) fn position(&self) -> u64 {
        self.projected_position().byte
    }

    pub(crate) fn activate(&self) {
        self.active.store(true, Ordering::Release);
    }

    pub(crate) fn variant(&self) -> Arc<HlsVariant> {
        Arc::clone(&self.variant)
    }

    pub(crate) fn variant_index(&self) -> usize {
        self.variant_index
    }

    pub(crate) fn advance(&self, bytes: u64) {
        let position = self.projected_position();
        self.advance_from(position, bytes);
    }

    fn projected_position(&self) -> SessionPosition {
        let provisional = self.position.load(Ordering::Acquire);
        let Some(projection) = self.variant.resolved_seek_projection(provisional) else {
            return SessionPosition {
                byte: provisional,
                projection: None,
            };
        };
        let exact = projection.exact_anchor();
        self.variant.set_prefetch_anchor(exact);
        SessionPosition {
            byte: exact,
            projection: Some(projection),
        }
    }

    fn advance_from(&self, position: SessionPosition, bytes: u64) {
        let next = position.byte.wrapping_add(bytes);
        self.position.store(next, Ordering::Release);
        if bytes == 0 {
            return;
        }
        if let Some(projection) = position.projection {
            self.variant.retire_seek_projection(projection);
        } else {
            self.variant.retire_seek_projection_if_moved(next);
        }
    }

    fn check_live(&self) -> io::Result<()> {
        if self.cancel.root.is_cancelled() {
            return Err(io::Error::other("HLS reader session cancelled"));
        }
        if !self.active.load(Ordering::Acquire)
            && self
                .transition
                .is_some_and(|transition| self.seek.epoch() != transition.id().seek_epoch())
        {
            return Err(pending(PendingReason::SeekPending));
        }
        Ok(())
    }

    fn prepare_at(&self, position: Duration) -> StreamResult<SourceSeekAnchor> {
        if self.cancel.is_cancelled() {
            return Err(StreamError::Source(SourceError::Cancelled));
        }
        let SessionReadiness::Profiled {
            preparation,
            profile,
        } = &self.readiness
        else {
            return Err(StreamError::Source(SourceError::Io(io::Error::new(
                ErrorKind::Unsupported,
                "active HLS session does not carry a decoder reader profile",
            ))));
        };
        self.cancel.rearm();
        let next = self.variant.prepare_reader(*profile, position)?;
        let anchor = next.anchor();
        *preparation.lock() = next;
        self.set_position(anchor.byte_offset);
        self.arm_peer();
        Ok(anchor)
    }

    pub(crate) fn set_position(&self, position: u64) {
        self.position.store(position, Ordering::Release);
    }

    pub(crate) fn seek_to_byte(&self, position: u64) {
        let previous = self.position.swap(position, Ordering::AcqRel);
        let moved = previous != position;
        self.variant.register_session_seek(position, moved);
    }

    pub(crate) fn seek_time_anchor(
        &self,
        position: Duration,
    ) -> StreamResult<Option<SourceSeekAnchor>> {
        let anchor = self.variant.prepare_seek_time_anchor(position)?;
        if let Some(anchor) = anchor {
            self.set_position(anchor.byte_offset);
        }
        Ok(anchor)
    }
}

impl ByteMap for HlsSession {
    fn anchor_at_time(&self, position: Duration) -> StreamResult<Option<SourceSeekAnchor>> {
        self.prepare_at(position).map(Some)
    }

    fn init_segment_range(&self) -> Range<u64> {
        self.variant.init_byte_range()
    }

    fn len(&self) -> Option<u64> {
        self.variant.stream_len()
    }

    fn segment_after_byte(&self, byte: u64) -> Option<SegmentDescriptor> {
        self.variant.descriptor_after_byte(byte)
    }

    fn segment_at_byte(&self, byte: u64) -> Option<SegmentDescriptor> {
        self.variant.descriptor_at_byte(byte)
    }

    fn segment_at_index(&self, segment_index: u32) -> Option<SegmentDescriptor> {
        self.variant.descriptor(segment_index as usize)
    }

    fn segment_at_time(&self, position: Duration) -> Option<SegmentDescriptor> {
        self.variant.descriptor_at_time(position)
    }

    fn segment_count(&self) -> Option<u32> {
        Some(self.variant.num_segments())
    }
}

fn pending(reason: PendingReason) -> io::Error {
    io::Error::new(ErrorKind::Interrupted, reason)
}
