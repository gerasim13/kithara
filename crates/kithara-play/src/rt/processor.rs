use std::{
    collections::VecDeque,
    num::{NonZeroU32, NonZeroUsize},
    sync::atomic::Ordering,
};

use firewheel::{
    StreamInfo,
    node::{
        AudioNodeProcessor, ProcBuffers, ProcExtra, ProcInfo, ProcStore, ProcStreamCtx,
        ProcessStatus,
    },
};
use kithara_bufpool::{HasPool, PoolRegion};
use kithara_dsp::param::SmootherConfig;
use kithara_events::TrackId;
use kithara_platform::sync::Arc;
use kithara_sync::TrackDisposal;
use kithara_test_utils::kithara;
use kithara_warp::RenderContext;
use num_traits::cast::AsPrimitive;
use ringbuf::{HeapCons, HeapProd, traits::Producer};
use smallvec::SmallVec;

use super::{context::read_render_context, track::PlayerTrack};
use crate::{
    bridge::{
        NodeInputs, PlaybackShared, PlayerCmd, PlayerNotification, TrackState, TrackTransition,
        sync::{PlaySync, PlayerSync},
    },
    rt::{RenderPass, RenderTargets, TrackSlot, TrackSlots},
    session::SessionError,
};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) enum ContextRequirement {
    #[default]
    Standalone,
    Session,
}

/// The realtime audio processor for the player node.
///
/// Owns the loaded tracks, handles transitions, and renders mixed stereo audio into the Firewheel
/// output buffers.
#[derive(fieldwork::Fieldwork)]
#[fieldwork(opt_in, get)]
pub struct PlayerNodeProcessor {
    #[field(get, deref = false)]
    pub(super) playback: Arc<PlaybackShared>,
    pub(super) crossfade: crate::CrossfadeSettings,
    pub(super) cmd_rx: HeapCons<PlayerCmd>,
    pub(super) notif_tx: HeapProd<PlayerNotification>,
    pub(super) sample_rate: NonZeroU32,
    pub(super) render: RenderPass,
    pub(super) tracks: TrackSlots<{ Self::MAX_TRACKS }>,
    pub(super) tracks_transitions: VecDeque<TrackTransition>,
    pub(super) prefetch_duration: f32,
    context_requirement: ContextRequirement,
    pub(super) trash_tx: HeapProd<PlayerTrack>,
    /// The deck's activation: the pending ticket, the return custody and the
    /// fading tail, with the receipt producer and the member gate it stamps
    /// render evidence with. The receipt producer lives as long as it does.
    pub(super) sync: PlaySync,
    /// Last effective rate successfully delivered to the control thread.
    last_notified_rate: f32,
}

/// Stream dimensions needed to pre-size RT scratch buffers.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StreamShape {
    pub max_block_frames: NonZeroU32,
    pub sample_rate: NonZeroU32,
}

impl StreamShape {
    #[must_use]
    pub const fn new(max_block_frames: NonZeroU32, sample_rate: NonZeroU32) -> Self {
        Self {
            max_block_frames,
            sample_rate,
        }
    }

    /// Compute decoder buffer depths, enforcing an application deadline when supplied.
    ///
    /// # Errors
    /// Returns an error when the geometry overflows or exceeds the budget.
    pub fn playback_buffers(
        self,
        quantum: NonZeroUsize,
        budget: Option<NonZeroUsize>,
    ) -> Result<(NonZeroUsize, NonZeroUsize), SessionError> {
        let output_frames = usize::try_from(self.max_block_frames.get())
            .map_err(|_| SessionError::ResponseGeometryOverflow)?;
        let preload = output_frames.div_ceil(quantum.get());
        let ring = preload
            .checked_add(1)
            .ok_or(SessionError::ResponseGeometryOverflow)?;
        let required_frames = ring
            .checked_add(1)
            .and_then(|chunks| chunks.checked_mul(quantum.get()))
            .and_then(|frames| frames.checked_sub(1))
            .ok_or(SessionError::ResponseGeometryOverflow)?;
        if let Some(budget) = budget
            && required_frames > budget.get()
        {
            return Err(SessionError::ResponseBudgetExceeded {
                required_frames,
                max_block_frames: self.max_block_frames.get(),
                render_quantum_frames: quantum.get(),
                budget_frames: budget.get(),
            });
        }
        Ok((
            NonZeroUsize::new(preload).ok_or(SessionError::ResponseGeometryOverflow)?,
            NonZeroUsize::new(ring).ok_or(SessionError::ResponseGeometryOverflow)?,
        ))
    }
}

impl PlayerNodeProcessor {
    /// Minimum position (seconds) before seeking is allowed on fade-in.
    pub(super) const FADE_IN_SEEK_THRESHOLD: f64 = 0.5;

    /// Maximum number of concurrent tracks per player node.
    pub const MAX_TRACKS: usize = 4;

    /// Create a new processor with the given command receiver and shared state.
    #[must_use]
    pub fn new<S>(
        inputs: NodeInputs,
        shape: StreamShape,
        pools: &PoolRegion<S>,
        gate_smoothing: SmootherConfig,
    ) -> Self
    where
        S: HasPool<f32>,
    {
        Self::with_context_requirement(
            inputs,
            shape,
            pools,
            gate_smoothing,
            ContextRequirement::Standalone,
        )
    }

    /// Clean up finished tracks, dropping `playing` once none is audible.
    ///
    /// When cleanup would empty the slot set after the queue plays out, keeps the track that
    /// reached natural EOF resident so an in-range seek can later revive it; `is_playing()` stays
    /// false until then.
    pub fn cleanup_finished_tracks(&mut self) {
        let finished: SmallVec<[(TrackSlot, bool); Self::MAX_TRACKS]> = self
            .tracks
            .iter()
            .filter(|(_, track)| track.state() == TrackState::Finished)
            .map(|(slot, track)| (slot, track.ended_at_eof()))
            .collect();

        let retain: Option<TrackSlot> = if finished.len() == self.tracks.len() {
            finished
                .iter()
                .find_map(|(slot, ended_at_eof)| ended_at_eof.then_some(*slot))
        } else {
            None
        };

        for (slot, _) in finished.iter().filter(|(slot, _)| Some(*slot) != retain) {
            self.unload_slot(*slot);
        }

        if self.tracks.len() == 0 || retain.is_some() {
            self.playback.playing.store(false, Ordering::SeqCst);
        }
    }

    pub(super) fn evict_tracks_if_needed(&mut self) {
        while self.tracks.is_full() {
            let Some((slot, state)) = self
                .tracks
                .iter()
                .filter(|(_, track)| !track.has_sync_lane() || self.sync.can_return())
                .min_by_key(|(_, track)| super::render::eviction_priority(track.state()))
                .map(|(slot, track)| (slot, track.state()))
            else {
                break;
            };

            if state == TrackState::Playing {
                self.playback.metrics().record_evicted_playing();
            }
            self.unload_slot(slot);
        }
    }

    fn leading_effective_rate(&self) -> Option<f32> {
        self.tracks
            .iter()
            .find_map(|(_, track)| track.state().is_leading().then(|| track.playback_rate()))
    }

    fn publish_effective_rate(&mut self, rate: f32) {
        self.playback.rate.store(rate, Ordering::Relaxed);
        if self.last_notified_rate != rate
            && self
                .notif_tx
                .try_push(PlayerNotification::RateChanged { rate })
                .is_ok()
        {
            self.last_notified_rate = rate;
        }
    }

    pub(super) fn refresh_effective_rate(&mut self) {
        let rate = if self.playback.playing.load(Ordering::SeqCst) {
            self.leading_effective_rate().unwrap_or(0.0)
        } else {
            0.0
        };
        self.publish_effective_rate(rate);
    }

    pub fn render_audio(
        &mut self,
        buffers: &mut ProcBuffers,
        frames: usize,
        is_playing: bool,
    ) -> (bool, Option<(f64, f64)>) {
        self.render_with_context(None, buffers, frames, is_playing)
    }

    fn render_context<'a>(
        &self,
        store: &'a ProcStore,
        info: &ProcInfo,
    ) -> Result<Option<&'a RenderContext>, &'static str> {
        match self.context_requirement {
            ContextRequirement::Standalone => Ok(None),
            ContextRequirement::Session => read_render_context(store, info).map(Some),
        }
    }

    fn render_with_context(
        &mut self,
        context: Option<&RenderContext>,
        buffers: &mut ProcBuffers,
        frames: usize,
        is_playing: bool,
    ) -> (bool, Option<(f64, f64)>) {
        self.render.render_audio(
            context,
            RenderTargets {
                tracks: &mut self.tracks,
                notification_tx: &mut self.notif_tx,
                metrics: self.playback.metrics(),
                seek_epoch: self.playback.seek_epoch.load(Ordering::SeqCst),
                sync: &mut self.sync,
                playback: &self.playback,
            },
            buffers,
            frames,
            is_playing,
        )
    }

    fn set_tracks_host_sample_rate(&mut self, sample_rate: NonZeroU32) {
        self.tracks
            .iter_mut()
            .for_each(|(_, track)| track.set_host_sample_rate(sample_rate));
    }

    /// Remove the track in `slot` once it can leave the callback: a track
    /// holding a sync lane waits for return custody.
    pub(super) fn unload_slot(&mut self, slot: TrackSlot) {
        let Some(how) = self
            .tracks
            .at_mut(slot)
            .and_then(|track| self.sync.disposal(track))
        else {
            return;
        };
        if let Some(track) = self.tracks.remove_at(slot) {
            let item_id = track.item_id();
            let src = Arc::clone(track.src());
            discard_track(&self.playback, &mut self.trash_tx, track, how);
            self.notif_tx
                .try_push(PlayerNotification::Unloaded { src, item_id })
                .ok();
        }
    }

    fn update_host_sample_rate(&mut self, sample_rate: NonZeroU32) {
        let rate_changed = self.sample_rate != sample_rate;
        self.sample_rate = sample_rate;
        self.playback
            .sample_rate
            .store(sample_rate.get(), Ordering::Relaxed);
        if rate_changed {
            self.set_tracks_host_sample_rate(sample_rate);
            self.render.update_sample_rate(sample_rate);
        }
    }

    /// Update `playback.position` / `playback.duration` from the
    /// leading track's last [`TrackReadOutcome`].
    ///
    /// `render_audio` captures the snapshot directly out of the outcome
    /// returned by `PlayerTrack::read`.
    /// Falls back to `track.position()` / `track.duration()` only when no
    /// leading track produced an outcome this cycle (cold start before
    /// the first render block, or every active track was a non-leading
    /// fade-in).
    ///
    /// Both published windows come from the leading track's lock-free snapshots: the decoded
    /// frontier, which is always `>=` position, and the cached span the download side published.
    fn update_position_duration(&self, leading_outcome: Option<(f64, f64)>) {
        for (_, track) in self.tracks.iter() {
            if track.state().is_leading() {
                self.playback
                    .frontier
                    .store(track.decoded_frontier(), Ordering::Relaxed);
                self.playback
                    .cached
                    .store(track.cached_span(), Ordering::Relaxed);
                break;
            }
        }

        if let Some((position, duration)) = leading_outcome {
            self.playback.position.store(position, Ordering::Relaxed);
            self.playback.duration.store(duration, Ordering::Relaxed);
            return;
        }

        for (_, track) in self.tracks.iter() {
            if track.state().is_leading() {
                self.playback
                    .position
                    .store(track.position(), Ordering::Relaxed);
                self.playback
                    .duration
                    .store(track.duration(), Ordering::Relaxed);
                break;
            }
        }
    }

    pub(super) fn with_context_requirement<S>(
        inputs: NodeInputs,
        shape: StreamShape,
        pools: &PoolRegion<S>,
        gate_smoothing: SmootherConfig,
        context_requirement: ContextRequirement,
    ) -> Self
    where
        S: HasPool<f32>,
    {
        let last_notified_rate = inputs.playback.rate.load(Ordering::Relaxed);
        Self {
            last_notified_rate,
            context_requirement,
            cmd_rx: inputs.cmd_rx,
            notif_tx: inputs.notif_tx,
            trash_tx: inputs.trash_tx,
            sync: PlaySync::new(inputs.sync, inputs.sync_receipts, inputs.sync_gate),
            playback: inputs.playback,
            sample_rate: shape.sample_rate,
            render: RenderPass::new(pools, shape, gate_smoothing),
            crossfade: crate::CrossfadeSettings::default(),
            prefetch_duration: 0.0,
            tracks: TrackSlots::default(),
            tracks_transitions: VecDeque::with_capacity(Self::MAX_TRACKS),
        }
    }

    delegate::delegate! {
        to self.tracks {
            /// Look up a track by its queue-item identity.
            #[must_use]
            #[call(get)]
            pub fn track(&self, item_id: TrackId) -> Option<&PlayerTrack>;
            /// Number of tracks currently held in the processor arena.
            #[must_use]
            #[call(len)]
            pub fn track_count(&self) -> usize;
            /// Look up a track by its queue-item identity (mutable).
            #[call(get_mut)]
            pub fn track_mut(&mut self, item_id: TrackId) -> Option<&mut PlayerTrack>;
        }
    }
}

impl AudioNodeProcessor for PlayerNodeProcessor {
    fn new_stream(&mut self, stream_info: &StreamInfo, _context: &mut ProcStreamCtx) {
        self.update_host_sample_rate(stream_info.sample_rate);
        self.render.resize(stream_info.max_block_frames.get().as_());
    }

    #[kithara::rtsan_forbid_blocking]
    fn process(
        &mut self,
        info: &ProcInfo,
        mut buffers: ProcBuffers,
        extra: &mut ProcExtra,
    ) -> ProcessStatus {
        self.playback.process_count.fetch_add(1, Ordering::Relaxed);

        let block = self.sync.begin_block();
        let drained = self.drain_commands();

        self.sync.maintain();

        self.cleanup_finished_tracks();

        let is_playing = self.playback.playing.load(Ordering::SeqCst);

        let context = match self.render_context(&extra.store, info) {
            Ok(context) => context,
            Err(reason) => {
                block.finish(&self.playback.applied_source, drained, false);
                let _ = extra.logger.try_error(reason);
                return ProcessStatus::ClearAllOutputs;
            }
        };

        let (playback_started, leading_outcome_pos_dur) =
            self.render_with_context(context, &mut buffers, info.frames, is_playing);

        self.update_position_duration(leading_outcome_pos_dur);
        self.refresh_effective_rate();
        block.finish(
            &self.playback.applied_source,
            drained,
            playback_started && is_playing,
        );

        if playback_started {
            ProcessStatus::OutputsModified
        } else {
            ProcessStatus::ClearAllOutputs
        }
    }
}

/// Let `track` leave the callback through `how`, deselecting the sync map it
/// rendered.
pub(super) fn discard_track(
    playback: &PlaybackShared,
    trash: &mut HeapProd<PlayerTrack>,
    track: PlayerTrack,
    how: TrackDisposal<'_, PlayerSync>,
) {
    if track
        .sync_map()
        .is_some_and(|map| playback.active_sync_map.load(Ordering::Relaxed) == u64::from(map))
    {
        playback.active_sync_map.store(0, Ordering::Release);
    }
    match how {
        TrackDisposal::Return(room) => room.track(track),
        TrackDisposal::Trash => {
            if trash.try_push(track).is_err() {
                playback.metrics().record_trash_overflow();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use firewheel::{
        clock::InstantSamples,
        dsp::{buffer::ConstSequentialBuffer, declick::DeclickValues},
        log::{RealtimeLoggerConfig, realtime_logger},
        mask::{ConnectedMask, ConstantMask, SilenceMask},
        node::{NUM_SCRATCH_BUFFERS, ProcStore, StreamStatus},
    };
    use kithara_events::TrackId;
    use kithara_platform::{sync::Arc, time::Duration};
    use kithara_signal::{
        OutputContext, SessionEpoch, SessionFrame, SourceSpan, TransportRevision,
    };
    use kithara_sync::{
        LoadGeneration, LoadedMedia, SyncExecutionReject, SyncExecutionStamp, SyncReceipt,
        SyncReceiptTx, mock::MemberOwner, sync_receipts,
    };
    use kithara_warp::{RenderContext, WarpMapRevision};
    use ringbuf::traits::{Consumer, Producer};

    use super::*;
    use crate::{
        bridge::{
            SharedEq, slot_channels,
            sync::{SyncReturn, SyncTicket},
        },
        rt::sync_owner_fixture::{ReaderMode, entry_ticket, fresh_owner, resource},
        test_pools::pools,
    };

    #[kithara::test]
    fn application_deadline_is_optional_but_explicit_geometry_is_enforced() {
        let shape = StreamShape::new(
            NonZeroU32::new(512).expect("fixture block"),
            NonZeroU32::new(48_000).expect("fixture rate"),
        );
        let quantum = NonZeroUsize::new(32).expect("fixture quantum");
        let (preload, ring) = shape
            .playback_buffers(quantum, None)
            .expect("unbounded deadline");
        assert_eq!((preload.get(), ring.get()), (16, 17));
        assert!(matches!(
            shape.playback_buffers(quantum, NonZeroUsize::new(448)),
            Err(SessionError::ResponseBudgetExceeded {
                required_frames: 575,
                max_block_frames: 512,
                render_quantum_frames: 32,
                budget_frames: 448,
            })
        ));
    }

    fn processor() -> (PlayerNodeProcessor, crate::bridge::SlotControl) {
        built_processor(ContextRequirement::Standalone, None)
    }

    fn built_processor(
        requirement: ContextRequirement,
        receipts: Option<SyncReceiptTx>,
    ) -> (PlayerNodeProcessor, crate::bridge::SlotControl) {
        let (mut inputs, control) = slot_channels(SharedEq::new(0));
        inputs.sync_receipts = receipts;
        let shape = StreamShape {
            sample_rate: NonZeroU32::new(44_100).expect("static sample rate"),
            max_block_frames: NonZeroU32::new(512).expect("static block size"),
        };
        (
            PlayerNodeProcessor::with_context_requirement(
                inputs,
                shape,
                &pools(),
                crate::DEFAULT_GATE_SMOOTHING,
                requirement,
            ),
            control,
        )
    }

    /// What one deck return carried, by item.
    #[derive(Debug, PartialEq, Eq)]
    enum Returned {
        Ticket(TrackId),
        Track(TrackId),
        Tail(TrackId),
    }

    impl From<SyncReturn> for Returned {
        fn from(returned: SyncReturn) -> Self {
            match returned {
                SyncReturn::Ticket(ticket) => Self::Ticket(ticket.item()),
                SyncReturn::Track(track) => Self::Track(track.item_id()),
                SyncReturn::Tail(tail) => Self::Tail(tail.item_id),
            }
        }
    }

    /// A playing track at `item_id` switched onto a sync lane, its old
    /// reader's tail dropped.
    fn lane_track(item_id: TrackId) -> PlayerTrack {
        let rate = NonZeroU32::new(44_100).expect("fixture rate");
        let mut track = PlayerTrack::builder()
            .sample_rate(rate)
            .item_id(item_id)
            .load(LoadGeneration::first())
            .build(resource(ReaderMode::Silence, rate, false, true, None));
        track.play();
        drop(track.activate_sync(
            resource(ReaderMode::Silence, rate, false, false, None),
            SourceSpan::new(0, 1, rate, 1).expect("source span"),
            WarpMapRevision::first(),
            rate,
            crate::CrossfadeSettings::default(),
        ));
        track
    }

    /// A ticket for `item_id` with the stamp its permit carries and the
    /// owner of its member.
    fn pending_ticket(item_id: TrackId) -> (SyncTicket, SyncExecutionStamp, MemberOwner) {
        let rate = NonZeroU32::new(44_100).expect("fixture rate");
        let owner = fresh_owner();
        let (ticket, stamp) = entry_ticket(
            owner.clone(),
            LoadedMedia::new(item_id, LoadGeneration::first()),
            resource(ReaderMode::Silence, rate, false, false, None),
            [0.0; 2],
            0,
            rate,
            Some(TransportRevision::first()),
        );
        (ticket, stamp, owner)
    }

    #[kithara::test]
    fn pressured_clear_keeps_command_and_sync_custody_until_host_drains() {
        let (receipt_tx, mut receipt_rx) = sync_receipts();
        let (mut processor, mut control) =
            built_processor(ContextRequirement::Standalone, Some(receipt_tx));
        let fillers = [
            TrackId::allocate(),
            TrackId::allocate(),
            TrackId::allocate(),
        ];
        for item_id in fillers {
            let filler = lane_track(item_id);
            let Some(TrackDisposal::Return(room)) = processor.sync.disposal(&filler) else {
                panic!("a lane track returns while custody has room");
            };
            room.track(filler);
        }
        assert!(!processor.sync.can_return());
        let item_id = TrackId::allocate();
        assert!(processor.tracks.insert(lane_track(item_id)).is_none());
        let (pending, pending_stamp, _) = pending_ticket(item_id);
        control
            .sync
            .room()
            .expect("an empty deck takes a ticket")
            .send(pending);
        assert!(control.cmd_tx.try_push(PlayerCmd::Clear).is_ok());
        assert!(
            control
                .cmd_tx
                .try_push(PlayerCmd::SetPrefetchDuration(0.25))
                .is_ok()
        );
        let mut returned = Vec::new();

        processor.drain_commands();
        assert_eq!(processor.tracks.len(), 1);
        assert!(control.sync.room().is_none());
        assert!(matches!(
            processor.cmd_rx.try_peek(),
            Some(PlayerCmd::Clear)
        ));
        assert!(!processor.playback.playing.load(Ordering::SeqCst));
        assert_eq!(processor.prefetch_duration, 0.0);
        assert!(receipt_rx.next_receipt().is_none());

        returned.push(control.sync.next_return().expect("first return"));
        processor.sync.maintain();
        processor.drain_commands();
        assert_eq!(processor.tracks.len(), 0);
        assert!(control.sync.room().is_none());
        assert!(!processor.sync.can_return());
        assert!(matches!(
            processor.cmd_rx.try_peek(),
            Some(PlayerCmd::Clear)
        ));
        assert_eq!(processor.prefetch_duration, 0.0);

        returned.push(control.sync.next_return().expect("second return"));
        processor.sync.maintain();
        processor.drain_commands();
        assert!(control.sync.room().is_some());
        assert!(!processor.sync.can_return());
        assert!(matches!(
            processor.cmd_rx.try_peek(),
            Some(PlayerCmd::Clear)
        ));
        assert_eq!(processor.prefetch_duration, 0.0);

        returned.push(control.sync.next_return().expect("third return"));
        processor.sync.maintain();
        processor.drain_commands();
        assert!(processor.sync.custody_cleared());
        assert!(processor.cmd_rx.try_peek().is_none());
        assert!(!processor.playback.playing.load(Ordering::SeqCst));
        assert_eq!(processor.prefetch_duration, 0.25);
        assert!(
            matches!(receipt_rx.next_receipt(), Some(SyncReceipt::Rejected {
            stamp,
            reason: SyncExecutionReject::Cancelled,
        }) if stamp == pending_stamp)
        );
        assert!(receipt_rx.next_receipt().is_none());

        while let Some(value) = control.sync.next_return() {
            returned.push(value);
        }
        let returned: Vec<Returned> = returned.into_iter().map(Returned::from).collect();
        assert_eq!(
            returned,
            [
                Returned::Track(fillers[0]),
                Returned::Track(fillers[1]),
                Returned::Track(fillers[2]),
                Returned::Track(item_id),
                Returned::Ticket(item_id),
            ]
        );

        let withdrawn_id = TrackId::allocate();
        let (withdrawn, _, owner) = pending_ticket(withdrawn_id);
        owner
            .revoke()
            .expect("the owner revokes a live permit outside an audio claim");
        control
            .sync
            .room()
            .expect("an empty deck takes a ticket")
            .send(withdrawn);
        assert!(control.cmd_tx.try_push(PlayerCmd::Clear).is_ok());
        processor.drain_commands();
        assert!(control.sync.room().is_some());
        assert!(processor.cmd_rx.try_peek().is_none());
        assert!(
            receipt_rx.next_receipt().is_none(),
            "withdrawal already belongs to the owner"
        );
        assert_eq!(
            control.sync.next_return().map(Returned::from),
            Some(Returned::Ticket(withdrawn_id))
        );
    }

    /// Process one 64-frame block at session output `start`, publishing its
    /// context first.
    fn process_session_block(
        processor: &mut PlayerNodeProcessor,
        extra: &mut ProcExtra,
        start: i64,
    ) {
        let rate = NonZeroU32::new(44_100).expect("static sample rate");
        let output = OutputContext::new(
            SessionFrame::new(start)..SessionFrame::new(start + 64),
            rate,
            SessionEpoch::new(1),
            Some(TransportRevision::first()),
        )
        .expect("invariant: fixture output range is ordered");
        super::super::publish_render_context(
            &mut extra.store,
            RenderContext::new_linear(output, None).expect("invariant: fixture context is valid"),
        )
        .expect("invariant: fixture context slot exists");
        let mut info = proc_info();
        info.frames = 64;
        info.clock_samples = InstantSamples(start);
        let inputs: [&[f32]; 0] = [];
        let mut left = [0.0; 64];
        let mut right = [0.0; 64];
        let mut outputs = [&mut left[..], &mut right[..]];
        let buffers = ProcBuffers {
            inputs: &inputs,
            outputs: &mut outputs,
        };
        let _ = processor.process(&info, buffers, extra);
    }

    #[kithara::test]
    fn process_claims_a_ticket_and_returns_its_tail_through_custody() {
        let rate = NonZeroU32::new(44_100).expect("fixture rate");
        let (receipt_tx, mut receipt_rx) = sync_receipts();
        let (mut processor, mut control) =
            built_processor(ContextRequirement::Session, Some(receipt_tx));
        let item_id = TrackId::allocate();
        let load = LoadGeneration::first();
        let mut track = PlayerTrack::builder()
            .sample_rate(rate)
            .item_id(item_id)
            .load(load)
            .build(resource(ReaderMode::Silence, rate, true, true, None));
        track.play();
        assert!(processor.tracks.insert(track).is_none());
        processor.playback.playing.store(true, Ordering::SeqCst);
        let (ticket, stamp) = entry_ticket(
            fresh_owner(),
            LoadedMedia::new(item_id, load),
            resource(ReaderMode::Silence, rate, true, true, None),
            [0.0; 2],
            0,
            rate,
            None,
        );
        let map = ticket.first().head().activation().revision();
        control
            .sync
            .room()
            .expect("an empty deck takes a ticket")
            .send(ticket);
        let mut store = ProcStore::with_capacity(1);
        super::super::install_render_context(&mut store)
            .expect("invariant: fixture installs one context slot");
        let mut extra = ProcExtra {
            logger: realtime_logger(RealtimeLoggerConfig::default()).0,
            store,
            scratch_buffers: ConstSequentialBuffer::<f32, NUM_SCRATCH_BUFFERS>::new(64),
            declick_values: DeclickValues::new(NonZeroU32::new(16).expect("static declick length")),
        };

        process_session_block(&mut processor, &mut extra, 0);
        let receipts: Vec<SyncReceipt> = std::iter::from_fn(|| receipt_rx.next_receipt()).collect();
        assert!(matches!(
            receipts.as_slice(),
            [SyncReceipt::Armed(armed), SyncReceipt::Presented(applied)]
                if *armed == stamp && applied.stamp() == stamp
        ));
        assert_eq!(
            processor.playback.active_sync_map.load(Ordering::Acquire),
            u64::from(map)
        );
        assert!(!processor.sync.custody_cleared());
        assert!(control.sync.next_return().is_none());

        let mut returned = None;
        for block in 1..16 {
            process_session_block(&mut processor, &mut extra, block * 64);
            returned = control.sync.next_return();
            if returned.is_some() {
                break;
            }
            assert!(
                !processor.sync.custody_cleared(),
                "the tail stays in custody while its fade sounds"
            );
        }
        match returned {
            Some(SyncReturn::Tail(tail)) => {
                assert!(
                    tail.settled(),
                    "the tail returns only once its fade settled"
                );
                assert_eq!(tail.item_id, item_id);
            }
            other => panic!(
                "custody returns the tail, got {:?}",
                other.map(Returned::from)
            ),
        }
        assert!(control.sync.next_return().is_none());
        assert!(processor.sync.custody_cleared());
        assert!(receipt_rx.next_receipt().is_none());
    }

    fn session_processor() -> PlayerNodeProcessor {
        built_processor(ContextRequirement::Session, None).0
    }

    fn proc_info() -> ProcInfo {
        ProcInfo {
            sample_rate: NonZeroU32::new(44_100).expect("static sample rate"),
            frames: 512,
            in_silence_mask: SilenceMask::default(),
            out_silence_mask: SilenceMask::default(),
            in_constant_mask: ConstantMask::default(),
            out_constant_mask: ConstantMask::default(),
            in_connected_mask: ConnectedMask::default(),
            out_connected_mask: ConnectedMask::default(),
            total_cpu_seconds_recip: 1.0,
            process_to_playback_delay: None,
            did_just_unbypass: false,
            last_marker_instant: InstantSamples(0),
            sample_rate_recip: f64::from(44_100).recip(),
            clock_samples: InstantSamples(0),
            duration_since_stream_start: Duration::ZERO,
            stream_status: StreamStatus::empty(),
            dropped_frames: 0,
        }
    }

    #[kithara::test]
    fn session_processors_read_the_same_host_context() {
        let mut store = ProcStore::with_capacity(1);
        super::super::install_render_context(&mut store)
            .expect("invariant: fixture installs one context slot");
        super::super::publish_render_context(
            &mut store,
            RenderContext::new_linear(
                OutputContext::new(
                    SessionFrame::new(0)..SessionFrame::new(512),
                    NonZeroU32::new(44_100).expect("static sample rate"),
                    SessionEpoch::new(3),
                    Some(TransportRevision::first()),
                )
                .expect("invariant: fixture output range is ordered"),
                None,
            )
            .expect("invariant: fixture context is valid"),
        )
        .expect("invariant: fixture context slot exists");
        let info = proc_info();
        let left = session_processor();
        let right = session_processor();
        let left = left
            .render_context(&store, &info)
            .expect("session context")
            .expect("required context");
        let right = right
            .render_context(&store, &info)
            .expect("session context")
            .expect("required context");

        assert!(std::ptr::eq(left, right));
        assert_eq!(left.output().session_epoch(), SessionEpoch::new(3));
        assert_eq!(
            left.output().transport_revision(),
            Some(TransportRevision::first())
        );
    }

    #[kithara::test]
    fn full_notification_ring_retries_latest_effective_rate_once() {
        let (mut processor, mut control) = processor();
        let filler = Arc::from("filler");
        while processor
            .notif_tx
            .try_push(PlayerNotification::Loaded {
                src: Arc::clone(&filler),
            })
            .is_ok()
        {}

        processor.publish_effective_rate(1.25);
        processor.publish_effective_rate(1.5);
        assert_eq!(processor.playback.rate.load(Ordering::Relaxed), 1.5);
        assert_eq!(processor.last_notified_rate, 0.0);

        assert!(control.notif_rx.try_pop().is_some());
        processor.publish_effective_rate(1.5);

        let mut delivered = Vec::new();
        while let Some(notification) = control.notif_rx.try_pop() {
            if let PlayerNotification::RateChanged { rate } = notification {
                delivered.push(rate);
            }
        }
        assert_eq!(delivered, [1.5]);

        processor.publish_effective_rate(1.5);
        assert!(control.notif_rx.try_pop().is_none());
    }
}
