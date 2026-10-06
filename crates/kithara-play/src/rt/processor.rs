use std::{
    num::{NonZeroU32, NonZeroUsize},
    ops::Range,
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
use kithara_command::{Inbox, Step};
use kithara_dsp::param::SmootherConfig;
use kithara_events::TrackId;
use kithara_platform::sync::Arc;
use kithara_signal::SessionFrame;
use kithara_test_utils::kithara;
use kithara_warp::RenderContext;
use num_traits::cast::AsPrimitive;
use ringbuf::{HeapProd, traits::Producer};

use super::{DeckMixerConfig, context::read_render_context, track::PlayerTrack};
use crate::{
    bridge::{
        DeckApplied, DeckMixSettings, DeckProtocol, NodeInputs, PlaybackShared, PlayerNotification,
        TrackState,
    },
    rt::{LeadingPlayhead, RenderPass, RenderTargets, TrackSlot, TrackSlots},
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
/// Takes the deck's batches from its inbox at the frames they apply on and renders the deck's
/// tracks between them into the Firewheel output buffers.
pub struct DeckMixer {
    inbox: Inbox<DeckProtocol>,
    deck: Deck,
    context_requirement: ContextRequirement,
}

/// The tracks a deck holds, how they mix, and what the control side reads of them.
pub(super) struct Deck {
    pub(super) playback: Arc<PlaybackShared>,
    /// How loud the deck sounds, as its parts last set it.
    pub(super) mix: DeckMixSettings,
    pub(super) notif_tx: HeapProd<PlayerNotification>,
    pub(super) sample_rate: NonZeroU32,
    pub(super) render: RenderPass,
    pub(super) tracks: TrackSlots,
    /// Media seconds every track consumes per output second.
    pub(super) rate: f32,
    /// The ramp every track starts and stops with.
    pub(super) declick: SmootherConfig,
    trash_tx: HeapProd<PlayerTrack>,
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

impl DeckMixer {
    /// Create a deck over the given channel ends, built as `config` says.
    #[must_use]
    pub fn new<S>(
        inputs: NodeInputs,
        shape: StreamShape,
        pools: &PoolRegion<S>,
        config: DeckMixerConfig,
    ) -> Self
    where
        S: HasPool<f32>,
    {
        Self::with_context_requirement(inputs, shape, pools, config, ContextRequirement::Standalone)
    }

    /// Shared playback state the control side reads.
    #[must_use]
    pub fn playback(&self) -> &Arc<PlaybackShared> {
        &self.deck.playback
    }

    /// Renders one block of `frames` frames starting at `start` on the session clock, without a
    /// render context: the deck's batches apply at their frames, and the tracks render between
    /// them. Returns whether any track was read.
    pub fn render_block(
        &mut self,
        start: SessionFrame,
        buffers: &mut ProcBuffers,
        frames: usize,
    ) -> bool {
        self.render_block_in(None, start, buffers, frames)
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

    fn render_block_in(
        &mut self,
        context: Option<&RenderContext>,
        start: SessionFrame,
        buffers: &mut ProcBuffers,
        frames: usize,
    ) -> bool {
        let Self { inbox, deck, .. } = self;
        let mut read = false;
        let mut leading = None;
        inbox.run_block(start, frames, |step| match step {
            Step::Due(mut due) => {
                let mut applied = DeckApplied::default();
                for part in due.commands_mut().drain(..) {
                    deck.apply(part, &mut applied);
                }
                due.apply(applied);
            }
            Step::Run(range) => {
                deck.cleanup_finished_tracks();
                let (range_read, range_leading) = deck.render_range(context, buffers, range);
                read |= range_read;
                leading = range_leading.or(leading);
            }
        });
        deck.update_position_duration(leading);
        deck.refresh_effective_rate();
        read
    }

    pub(super) fn with_context_requirement<S>(
        inputs: NodeInputs,
        shape: StreamShape,
        pools: &PoolRegion<S>,
        config: DeckMixerConfig,
        context_requirement: ContextRequirement,
    ) -> Self
    where
        S: HasPool<f32>,
    {
        let last_notified_rate = inputs.playback.rate.load();
        let mix = DeckMixSettings::default();
        Self {
            context_requirement,
            inbox: inputs.deck,
            deck: Deck {
                last_notified_rate,
                notif_tx: inputs.notif_tx,
                trash_tx: inputs.trash_tx,
                playback: inputs.playback,
                sample_rate: shape.sample_rate,
                render: RenderPass::new(pools, shape, config, mix.gain()),
                mix,
                rate: 1.0,
                declick: config.declick(),
                tracks: TrackSlots::new(config.slots()),
            },
        }
    }

    delegate::delegate! {
        to self.deck.tracks {
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

impl Deck {
    /// Clean up finished tracks, dropping `playing` once none is audible.
    ///
    /// When cleanup would empty the slot set after the queue plays out, keeps the track that
    /// reached natural EOF resident so an in-range seek can later revive it; `is_playing()` stays
    /// false until then.
    pub(super) fn cleanup_finished_tracks(&mut self) {
        let all_finished = self
            .tracks
            .iter()
            .all(|(_, track)| track.state() == TrackState::Finished);
        let retain: Option<TrackSlot> = if all_finished {
            self.tracks
                .iter()
                .find_map(|(slot, track)| track.ended_at_eof().then_some(slot))
        } else {
            None
        };

        for slot in self.tracks.slots() {
            if Some(slot) != retain
                && self
                    .tracks
                    .at(slot)
                    .is_some_and(|track| track.state() == TrackState::Finished)
            {
                self.unload_slot(slot);
            }
        }

        if self.tracks.len() == 0 || retain.is_some() {
            self.set_playing(false);
        }
    }

    pub(super) fn discard_track(&mut self, track: PlayerTrack) {
        if self.trash_tx.try_push(track).is_err() {
            self.playback.metrics().record_trash_overflow();
        }
    }

    pub(super) fn evict_tracks_if_needed(&mut self) {
        while self.tracks.is_full() {
            let Some((slot, state)) = self
                .tracks
                .iter()
                .min_by_key(|(_, track)| super::render::eviction_priority(track.state()))
                .map(|(slot, track)| (slot, track.state()))
            else {
                break;
            };

            if state == TrackState::Playing {
                self.playback.metrics().record_evicted_playing();
            }
            if let Some(track) = self.tracks.remove_at(slot) {
                let item_id = track.item_id();
                let src = Arc::clone(track.src());
                self.discard_track(track);
                self.notif_tx
                    .try_push(PlayerNotification::Unloaded { src, item_id })
                    .ok();
            }
        }
    }

    fn leading_effective_rate(&self) -> Option<f32> {
        self.tracks
            .iter()
            .find_map(|(_, track)| track.state().is_leading().then(|| track.playback_rate()))
    }

    fn publish_effective_rate(&mut self, rate: f32) {
        self.playback.rate.store(rate);
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

    fn render_range(
        &mut self,
        context: Option<&RenderContext>,
        buffers: &mut ProcBuffers,
        range: Range<usize>,
    ) -> (bool, Option<LeadingPlayhead>) {
        self.render.render_range(
            context,
            RenderTargets {
                tracks: &mut self.tracks,
                notification_tx: &mut self.notif_tx,
                metrics: self.playback.metrics(),
                seek_epoch: self.playback.seek_epoch.load(Ordering::SeqCst),
                publishing: self.playback.publishing(),
            },
            buffers,
            range,
        )
    }

    fn retire(&mut self, track: PlayerTrack) {
        let item_id = track.item_id();
        let src = Arc::clone(track.src());
        self.discard_track(track);
        self.notif_tx
            .try_push(PlayerNotification::Unloaded { src, item_id })
            .ok();
    }

    fn set_tracks_host_sample_rate(&mut self, sample_rate: NonZeroU32) {
        self.tracks
            .iter_mut()
            .for_each(|(_, track)| track.set_host_sample_rate(sample_rate));
    }

    /// Take the track at `slot` off the deck, with every chain that names it: a track attached
    /// again under the same id starts only when told to.
    pub(super) fn unload_slot(&mut self, slot: TrackSlot) {
        if let Some(track) = self.tracks.remove_at(slot) {
            for (_, held) in self.tracks.iter_mut() {
                held.unchain(track.item_id());
            }
            self.retire(track);
        }
    }

    fn update_host_sample_rate(&mut self, sample_rate: NonZeroU32) {
        let rate_changed = self.sample_rate != sample_rate;
        self.sample_rate = sample_rate;
        self.playback.sample_rate.store(sample_rate.get());
        if rate_changed {
            self.set_tracks_host_sample_rate(sample_rate);
            self.render.update_sample_rate(sample_rate);
        }
    }

    /// Publish the playhead from the track that describes it: its last [`TrackReadOutcome`] this
    /// block, or its own position and duration when no range rendered it.
    ///
    /// That track leads under an epoch the deck publishes for (see
    /// [`PublishingEpochs`](crate::bridge::PublishingEpochs)). A successor stitched in under an
    /// epoch the control side has not taken on yet leaves the playhead on the track that ended,
    /// which the control side still calls current; once taken on, the successor is adopted.
    ///
    /// Both published windows come from that track's lock-free snapshots: the decoded frontier,
    /// which is always `>=` position, and the cached span the download side published.
    fn update_position_duration(&self, leading_outcome: Option<LeadingPlayhead>) {
        let publishing = self.playback.publishing();
        let leading = self
            .tracks
            .iter()
            .map(|(_, track)| track)
            .find(|track| track.state().is_leading() && publishing.admit(track.epoch()));
        if let Some(track) = leading {
            self.playback.frontier.store(track.decoded_frontier());
            self.playback.cached.store(track.cached_span());
        }
        let playhead = leading_outcome
            .or_else(|| leading.map(|track| (track.epoch(), track.position(), track.duration())));
        if let Some((epoch, position, duration)) = playhead {
            self.playback.adopt(epoch, position, duration);
        }
    }
}

impl AudioNodeProcessor for DeckMixer {
    fn new_stream(&mut self, stream_info: &StreamInfo, _context: &mut ProcStreamCtx) {
        self.deck.update_host_sample_rate(stream_info.sample_rate);
        self.deck
            .render
            .resize(stream_info.max_block_frames.get().as_());
    }

    #[kithara::rtsan_forbid_blocking]
    fn process(
        &mut self,
        info: &ProcInfo,
        mut buffers: ProcBuffers,
        extra: &mut ProcExtra,
    ) -> ProcessStatus {
        self.deck
            .playback
            .process_count
            .fetch_add(1, Ordering::Relaxed);

        let context = match self.render_context(&extra.store, info) {
            Ok(context) => context,
            Err(reason) => {
                let _ = extra.logger.try_error(reason);
                return ProcessStatus::ClearAllOutputs;
            }
        };
        let start = SessionFrame::new(info.clock_samples.0);
        if self.render_block_in(context, start, &mut buffers, info.frames) {
            ProcessStatus::OutputsModified
        } else {
            ProcessStatus::ClearAllOutputs
        }
    }
}

#[cfg(test)]
mod tests {
    use firewheel::{
        clock::InstantSamples,
        mask::{ConnectedMask, ConstantMask, SilenceMask},
        node::{ProcStore, StreamStatus},
    };
    use kithara_audio::mock::TestPcmReader;
    use kithara_platform::time::Duration;
    use kithara_signal::{AudioSpec, OutputContext, SessionEpoch, SessionFrame, TransportRevision};
    use kithara_test_fixtures::integration_fixtures::constant_half;
    use kithara_warp::RenderContext;
    use ringbuf::traits::{Consumer, Producer};

    use super::*;
    use crate::{
        Resource,
        bridge::{DeckPart, SharedEq, slot_channels},
        rt::track::PlayerResource,
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

    fn processor() -> (DeckMixer, crate::bridge::SlotControl) {
        let (inputs, control) = slot_channels(SharedEq::new(0));
        let shape = StreamShape {
            sample_rate: NonZeroU32::new(44_100).expect("static sample rate"),
            max_block_frames: NonZeroU32::new(512).expect("static block size"),
        };
        (
            DeckMixer::new(inputs, shape, &pools(), DeckMixerConfig::default()),
            control,
        )
    }

    fn session_processor() -> DeckMixer {
        let (inputs, _control) = slot_channels(SharedEq::new(0));
        let shape = StreamShape {
            sample_rate: NonZeroU32::new(44_100).expect("static sample rate"),
            max_block_frames: NonZeroU32::new(512).expect("static block size"),
        };
        DeckMixer::with_context_requirement(
            inputs,
            shape,
            &pools(),
            DeckMixerConfig::default(),
            ContextRequirement::Session,
        )
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

    fn pcm_resource(constant_half: &'static [u8], src: &str, seconds: f64) -> Box<PlayerResource> {
        let spec = AudioSpec::new(2, NonZeroU32::new(44_100).expect("static sample rate"));
        let reader = TestPcmReader::with_pcm(spec, seconds, constant_half);
        Box::new(
            PlayerResource::new(
                Resource::from_reader(reader, None),
                Arc::from(src),
                &pools(),
            )
            .expect("player resource fits the test pool budget"),
        )
    }

    fn render(processor: &mut DeckMixer) {
        let frames = 512;
        let mut left = vec![0.0f32; frames];
        let mut right = vec![0.0f32; frames];
        let inputs: [&[f32]; 0] = [];
        let mut outputs = [&mut left[..], &mut right[..]];
        let mut buffers = ProcBuffers {
            inputs: &inputs,
            outputs: &mut outputs,
        };
        processor.render_block(SessionFrame::default(), &mut buffers, frames);
    }

    /// A successor the deck stitches in leads under the epoch its chain carries: until the
    /// control side takes that epoch on, the playhead describes the track that ended, and from
    /// then on the successor.
    #[kithara::test(tokio)]
    async fn a_stitched_successor_publishes_its_playhead_once_its_epoch_is_taken_on(
        constant_half: &'static [u8],
    ) {
        const ENDING_SECONDS: f64 = 0.005;
        const SUCCESSOR_SECONDS: f64 = 60.0;
        let (mut processor, mut control) = processor();
        let playback = Arc::clone(processor.playback());
        let ending = TrackId::allocate();
        let successor = TrackId::allocate();
        for (src, seconds, item_id) in [
            ("ending", ENDING_SECONDS, ending),
            ("successor", SUCCESSOR_SECONDS, successor),
        ] {
            control
                .send(DeckPart::Attach {
                    resource: pcm_resource(constant_half, src, seconds),
                    item_id,
                })
                .ok();
        }
        let epoch = playback.issue_epoch();
        control
            .send(DeckPart::Chain {
                from: ending,
                to: successor,
                epoch,
            })
            .ok();
        control.send(DeckPart::StartAll).ok();
        render(&mut processor);
        processor
            .track_mut(ending)
            .expect("the ending track is attached")
            .play();

        render(&mut processor);
        assert_eq!(
            processor.track(successor).map(PlayerTrack::state),
            Some(TrackState::Playing),
            "the deck stitched the successor in"
        );
        let stitched = playback.snapshot();
        assert!(
            (stitched.duration() - ENDING_SECONDS).abs() < 1e-3,
            "the ended track still describes the playhead: {stitched:?}"
        );
        assert!(
            (stitched.position() - stitched.duration()).abs() < 1e-3,
            "the ended track reads at its end: {stitched:?}"
        );

        playback.take_on(epoch, SUCCESSOR_SECONDS);
        let taken_on = playback.snapshot();
        assert_eq!(
            (taken_on.position(), taken_on.duration()),
            (0.0, SUCCESSOR_SECONDS)
        );

        render(&mut processor);
        let adopted = playback.snapshot();
        assert!(adopted.position() > 0.0, "{adopted:?}");
        assert!(
            (adopted.duration() - SUCCESSOR_SECONDS).abs() < 1e-3,
            "{adopted:?}"
        );
    }

    /// A successor that starts and ends inside the block it is stitched in still reports its
    /// start, with the epoch it leads under, before its end: the control side takes that epoch
    /// on from the start receipt alone.
    #[kithara::test(tokio)]
    async fn a_successor_ending_in_its_stitch_block_reports_its_start_first(
        constant_half: &'static [u8],
    ) {
        let (mut processor, mut control) = processor();
        let playback = Arc::clone(processor.playback());
        let ending = TrackId::allocate();
        let successor = TrackId::allocate();
        for (src, seconds, item_id) in [("ending", 0.005, ending), ("successor", 0.002, successor)]
        {
            control
                .send(DeckPart::Attach {
                    resource: pcm_resource(constant_half, src, seconds),
                    item_id,
                })
                .ok();
        }
        let epoch = playback.issue_epoch();
        control
            .send(DeckPart::Chain {
                from: ending,
                to: successor,
                epoch,
            })
            .ok();
        control.send(DeckPart::StartAll).ok();
        render(&mut processor);
        processor
            .track_mut(ending)
            .expect("the ending track is attached")
            .play();
        while control.notif_rx.try_pop().is_some() {}

        render(&mut processor);
        let mut receipts = Vec::new();
        while let Some(notification) = control.notif_rx.try_pop() {
            match notification {
                PlayerNotification::PlaybackStarted {
                    item_id,
                    epoch: started,
                    ..
                } if item_id == successor => receipts.push(("started", Some(started))),
                PlayerNotification::PlaybackStopped { item_id, .. } if item_id == successor => {
                    receipts.push(("stopped", None));
                }
                _ => {}
            }
        }
        assert_eq!(receipts, [("started", Some(epoch)), ("stopped", None)]);
    }

    /// Withdrawing a chained successor takes the chain with it: attached again under the same
    /// id, it waits for its own start instead of being stitched in by the stale chain.
    #[kithara::test(tokio)]
    async fn a_withdrawn_successor_is_not_stitched_in_once_attached_again(
        constant_half: &'static [u8],
    ) {
        let (mut processor, mut control) = processor();
        let playback = Arc::clone(processor.playback());
        let ending = TrackId::allocate();
        let successor = TrackId::allocate();
        for (src, seconds, item_id) in [("ending", 0.005, ending), ("successor", 60.0, successor)] {
            control
                .send(DeckPart::Attach {
                    resource: pcm_resource(constant_half, src, seconds),
                    item_id,
                })
                .ok();
        }
        control
            .send(DeckPart::Chain {
                from: ending,
                to: successor,
                epoch: playback.issue_epoch(),
            })
            .ok();
        control.send(DeckPart::Withdraw { item_id: successor }).ok();
        control
            .send(DeckPart::Attach {
                resource: pcm_resource(constant_half, "successor", 60.0),
                item_id: successor,
            })
            .ok();
        control.send(DeckPart::StartAll).ok();
        render(&mut processor);
        processor
            .track_mut(ending)
            .expect("the ending track is attached")
            .play();

        render(&mut processor);
        render(&mut processor);
        assert_eq!(
            processor.track(successor).map(PlayerTrack::state),
            Some(TrackState::Preloading),
            "nothing started the successor attached again"
        );
    }

    #[kithara::test]
    fn full_notification_ring_retries_latest_effective_rate_once() {
        let (mut processor, mut control) = processor();
        let filler = Arc::from("filler");
        while processor
            .deck
            .notif_tx
            .try_push(PlayerNotification::Loaded {
                src: Arc::clone(&filler),
            })
            .is_ok()
        {}

        processor.deck.publish_effective_rate(1.25);
        processor.deck.publish_effective_rate(1.5);
        assert_eq!(processor.deck.playback.rate.load(), 1.5);
        assert_eq!(processor.deck.last_notified_rate, 0.0);

        assert!(control.notif_rx.try_pop().is_some());
        processor.deck.publish_effective_rate(1.5);

        let mut delivered = Vec::new();
        while let Some(notification) = control.notif_rx.try_pop() {
            if let PlayerNotification::RateChanged { rate } = notification {
                delivered.push(rate);
            }
        }
        assert_eq!(delivered, [1.5]);

        processor.deck.publish_effective_rate(1.5);
        assert!(control.notif_rx.try_pop().is_none());
    }
}
