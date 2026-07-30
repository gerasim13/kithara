mod cancel;
mod reader;

use std::{
    io::{self, ErrorKind},
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
};

use kithara_platform::{
    CancelToken,
    sync::{Arc, Mutex},
    time::Duration,
};
use kithara_stream::{
    ByteMap, PendingReason, ReaderProfile, SeekObserve, SegmentDescriptor, SourceError,
    SourceSeekAnchor, StreamError, StreamResult, VariantTransition,
};

use self::cancel::SessionCancel;
pub(super) use self::reader::HlsSessionReader;
use crate::{
    config::VariantSessionMode,
    signal::SizeSignal,
    variant::{HlsVariant, ResolvedSeekProjection, VariantReaderPreparation},
};

pub(crate) struct HlsSession {
    active: AtomicBool,
    cancel: SessionCancel,
    legacy_cursor_projection: bool,
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
        mode: VariantSessionMode,
        variant_index: usize,
        variant: Arc<HlsVariant>,
        position: u64,
    ) -> Self {
        Self {
            active: AtomicBool::new(true),
            cancel: SessionCancel::new(cancel),
            legacy_cursor_projection: matches!(mode, VariantSessionMode::Legacy),
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
            legacy_cursor_projection: false,
            position: AtomicU64::new(preparation.anchor().byte_offset),
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

    pub(crate) fn dispatch(
        &self,
        ctx: &crate::variant::PlanCtx,
        budget: usize,
    ) -> Vec<kithara_stream::dl::FetchCmd> {
        if self.cancel.is_cancelled() {
            return Vec::new();
        }
        self.variant
            .dispatch_from(ctx, budget, self.position(), self.cancel.handle())
    }

    pub(crate) fn find_at_offset(&self, byte: u64) -> Option<(u32, u64, u64)> {
        self.variant.find_at_offset(byte)
    }

    pub(crate) fn is_ready(&self) -> StreamResult<bool> {
        match &self.readiness {
            SessionReadiness::Active => Ok(true),
            SessionReadiness::Profiled { preparation, .. } => {
                self.variant.reader_is_ready(&preparation.lock())
            }
        }
    }

    pub(crate) fn len(&self) -> Option<u64> {
        self.variant.stream_len()
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
        if self.legacy_cursor_projection {
            self.variant.project_session_seek(position, moved);
        } else {
            self.variant.register_session_seek(position, moved);
        }
    }

    pub(crate) fn seek_time_anchor(
        &self,
        position: Duration,
    ) -> StreamResult<Option<SourceSeekAnchor>> {
        let anchor = if self.legacy_cursor_projection {
            self.variant.seek_time_anchor(position)?
        } else {
            self.variant.prepare_seek_time_anchor(position)?
        };
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

    fn init_segment_range(&self) -> std::ops::Range<u64> {
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
