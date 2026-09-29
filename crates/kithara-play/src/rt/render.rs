use std::{num::NonZeroU32, ops::Range, sync::atomic::Ordering};

use firewheel::node::ProcBuffers;
use kithara_bufpool::{HasPool, PoolRegion, SampleBuffer};
use kithara_dsp::{
    fade::FadeCurve,
    param::{Mix, MixDSP, SmootherConfig},
};
use kithara_events::TrackId;
use kithara_sync::{
    ActivationDeck, ActivationResident, LoadGeneration, PreparedFirst, SyncAttempt,
};
use kithara_warp::RenderContext;
use num_traits::cast::AsPrimitive;
use ringbuf::HeapProd;
use smallvec::SmallVec;
use tracing::warn;

use super::{
    processor::{PlayerNodeProcessor, StreamShape},
    track::{PlayerResource, PlayerTrack, RtSink, SyncFadeTail, TrackReadOutcome},
};
use crate::{
    bridge::{
        PlaybackShared, PlayerNotification, RtMetrics, TrackState,
        sync::{PlaySync, PlayerSync},
    },
    consts,
    rt::{TrackSlot, TrackSlots},
};

type ActiveTrackEntry = (usize, TrackSlot, bool);

/// The callback's pre-switch track order, including its original leaders.
struct TrackOrder {
    loaded: SmallVec<[(TrackSlot, TrackState); PlayerNodeProcessor::MAX_TRACKS]>,
    active: SmallVec<[ActiveTrackEntry; PlayerNodeProcessor::MAX_TRACKS]>,
}

impl TrackOrder {
    fn capture(tracks: &TrackSlots<{ PlayerNodeProcessor::MAX_TRACKS }>) -> Self {
        let loaded: SmallVec<[(TrackSlot, TrackState); PlayerNodeProcessor::MAX_TRACKS]> = tracks
            .iter()
            .map(|(idx, track)| (idx, track.state()))
            .collect();
        let active = loaded
            .iter()
            .enumerate()
            .filter(|(_, (_, state))| state.is_playing())
            .map(|(loaded_idx, (idx, state))| (loaded_idx, *idx, state.is_leading()))
            .collect();
        Self { loaded, active }
    }
}

#[derive(Clone, Copy)]
struct Handover {
    offset: usize,
}

pub(crate) struct RenderTargets<'a> {
    pub(crate) notification_tx: &'a mut HeapProd<PlayerNotification>,
    pub(crate) metrics: &'a RtMetrics,
    pub(crate) tracks: &'a mut TrackSlots<{ PlayerNodeProcessor::MAX_TRACKS }>,
    /// Slot seek epoch published when this block started rendering.
    pub(crate) seek_epoch: u64,
    pub(crate) sync: &'a mut PlaySync,
    pub(crate) playback: &'a PlaybackShared,
}

type BlockAttempt = SyncAttempt<TrackId, TrackReadOutcome>;

/// One block's tracks and buffers, as the sync callback renders them.
struct PlayDeck<'a, 'b> {
    tracks: &'a mut TrackSlots<{ PlayerNodeProcessor::MAX_TRACKS }>,
    read_bufs: &'a mut [&'b mut [f32]],
    bus_bufs: &'a mut [&'b mut [f32]],
    sink: &'a mut RtSink<'b>,
    playback: &'a PlaybackShared,
}

/// The resident track one activation attempt renders.
struct PlayResident<'r, 'b> {
    track: &'r mut PlayerTrack,
    read_bufs: &'r mut [&'b mut [f32]],
    bus_bufs: &'r mut [&'b mut [f32]],
    sink: &'r mut RtSink<'b>,
    context: &'r RenderContext,
    playback: &'r PlaybackShared,
}

impl<'b> ActivationDeck<PlayerSync> for PlayDeck<'_, 'b> {
    type Outcome = TrackReadOutcome;
    type Resident<'r>
        = PlayResident<'r, 'b>
    where
        Self: 'r;

    fn resident<'r>(
        &'r mut self,
        item: TrackId,
        context: &'r RenderContext,
    ) -> Option<PlayResident<'r, 'b>> {
        Some(PlayResident {
            track: self.tracks.get_mut(item)?,
            read_bufs: self.read_bufs,
            bus_bufs: self.bus_bufs,
            sink: self.sink,
            context,
            playback: self.playback,
        })
    }

    fn render_tail(
        &mut self,
        tail: &mut SyncFadeTail,
        context: &RenderContext,
        range: Range<usize>,
    ) {
        tail.render(
            context,
            self.read_bufs,
            self.bus_bufs,
            range,
            self.sink.metrics(),
        );
    }
}

impl ActivationResident<PlayerSync> for PlayResident<'_, '_> {
    type Outcome = TrackReadOutcome;

    fn serves(&self, load: LoadGeneration, output_rate: NonZeroU32) -> bool {
        self.track.load() == Some(load)
            && self.track.state().is_leading()
            && self.track.output_sample_rate() == output_rate.get()
    }

    fn render(&mut self, range: Range<usize>) -> TrackReadOutcome {
        self.track.render(
            Some(self.context),
            self.read_bufs,
            self.bus_bufs,
            range,
            self.sink,
        )
    }

    fn leads_after(&self, prefix: Option<&TrackReadOutcome>) -> bool {
        self.track.state().is_leading()
            && prefix.is_none_or(|outcome| matches!(outcome, TrackReadOutcome::Full { .. }))
    }

    /// The selected map is stored with Release after the first frame renders
    /// and before its receipts are written, so it is visible whenever the
    /// Host consumes Presented.
    fn activate(
        &mut self,
        lane: Box<PlayerResource>,
        first: &PreparedFirst,
        first_context: &RenderContext,
        at: usize,
    ) -> SyncFadeTail {
        let map = first.head().activation().revision();
        let old = self.track.activate_sync(
            lane,
            first.source(),
            map,
            first.head().output_rate(),
            consts::SYNC_FADE,
        );
        self.track.render_first(
            first,
            first_context,
            self.read_bufs,
            self.bus_bufs,
            at,
            self.sink,
        );
        self.playback
            .active_sync_map
            .store(u64::from(map), Ordering::Release);
        old
    }

    fn render_tail(&mut self, tail: &mut SyncFadeTail, range: Range<usize>) {
        tail.render(
            self.context,
            self.read_bufs,
            self.bus_bufs,
            range,
            self.sink.metrics(),
        );
    }

    fn finish(&mut self, suffix: Range<usize>) -> (TrackReadOutcome, Option<usize>) {
        if suffix.is_empty() {
            return (
                TrackReadOutcome::Full {
                    position: self.track.position(),
                    frames: suffix.start,
                    duration: self.track.duration(),
                    frames_until_eof: self.track.frames_until_eof(),
                },
                None,
            );
        }
        let base = suffix.start;
        let outcome = self.render(suffix);
        extend_outcome(base, outcome)
    }
}

pub(crate) struct RenderPass {
    gate: MixDSP,
    scratch_bufs: [SampleBuffer; Self::SCRATCH_BUF_COUNT],
    priming: bool,
    capacity: usize,
}

impl RenderPass {
    const GATE_CURVE: FadeCurve = FadeCurve::Linear;
    const MIN_STEREO: usize = 2;
    const SCRATCH_BUF_COUNT: usize = 4;

    pub(crate) fn new<S>(
        pools: &PoolRegion<S>,
        shape: StreamShape,
        gate_smoothing: SmootherConfig,
    ) -> Self
    where
        S: HasPool<f32>,
    {
        let mut pass = Self {
            scratch_bufs: std::array::from_fn(|_| pools.get::<f32>()),
            capacity: 0,
            priming: true,
            gate: MixDSP::new(
                Mix::FULLY_WET,
                Self::GATE_CURVE,
                gate_smoothing,
                shape.sample_rate,
            ),
        };
        pass.resize(shape.max_block_frames.get().as_());
        pass
    }

    /// Render audio for all active tracks into the output buffers.
    ///
    /// Frames are clamped rather than grown, since growing a pooled buffer here would allocate on
    /// the audio thread; frames past the clamp are already silence-filled.
    pub(crate) fn render_audio(
        &mut self,
        context: Option<&RenderContext>,
        targets: RenderTargets<'_>,
        buffers: &mut ProcBuffers,
        frames: usize,
        is_playing: bool,
    ) -> (bool, Option<(f64, f64)>) {
        let mut playback_started = false;

        if buffers.outputs.len() < Self::MIN_STEREO {
            return (false, None);
        }

        for ch_buffer in buffers.outputs.iter_mut() {
            ch_buffer[..frames].fill(0.0);
        }

        let frames = frames.min(self.capacity);

        self.gate.set_mix(
            if is_playing {
                Mix::FULLY_DRY
            } else {
                Mix::FULLY_WET
            },
            Self::GATE_CURVE,
        );
        if self.priming {
            self.priming = false;
            self.gate.reset_to_target();
        }
        if !is_playing && self.gate.has_settled() {
            return (false, None);
        }

        let (read, bus) = self.scratch_bufs.split_at_mut(Self::MIN_STEREO);
        let (read_buf0, read_buf1) = read.split_at_mut(1);
        let (bus_buf0, bus_buf1) = bus.split_at_mut(1);
        let mut read_bufs = [&mut read_buf0[0][..frames], &mut read_buf1[0][..frames]];
        let mut bus_bufs = [&mut bus_buf0[0][..frames], &mut bus_buf1[0][..frames]];
        for ch_buffer in &mut bus_bufs {
            ch_buffer.fill(0.0);
        }
        let tracks = targets.tracks;
        let mut sink = RtSink::new(targets.notification_tx, targets.metrics, targets.seek_epoch);
        // Keep the ordinary leading traversal from the start of the block:
        // the old prefix can reach EOF before the attempted physical switch.
        let order = TrackOrder::capture(tracks);
        let mut attempt = {
            let mut deck = PlayDeck {
                tracks: &mut *tracks,
                read_bufs: &mut read_bufs,
                bus_bufs: &mut bus_bufs,
                sink: &mut sink,
                playback: targets.playback,
            };
            if is_playing {
                targets.sync.attempt(&mut deck, context, frames)
            } else {
                targets.sync.fade(&mut deck, context, frames);
                SyncAttempt::None
            }
        };
        playback_started |= matches!(&attempt, SyncAttempt::Claimed { .. });
        let (rendered, leading_outcome_pos_dur) = render_active_tracks(
            context,
            tracks,
            &order,
            &mut attempt,
            &mut read_bufs,
            &mut bus_bufs,
            &mut sink,
        );
        playback_started |= rendered;

        let (out_left, out_right) = buffers.outputs.split_at_mut(1);
        self.gate.mix_dry_into_wet_stereo(
            bus_bufs[0],
            bus_bufs[1],
            &mut out_left[0][..frames],
            &mut out_right[0][..frames],
            frames,
        );

        (playback_started, leading_outcome_pos_dur)
    }

    pub(crate) fn resize(&mut self, max_frames: usize) {
        let mut capacity = usize::MAX;
        for buf in &mut self.scratch_bufs {
            if buf.ensure_len(max_frames).is_err() {
                warn!(
                    max_frames,
                    held = buf.len(),
                    "sample pool budget cannot afford the render scratch; blocks are clamped"
                );
            }
            buf.fill(0.0);
            capacity = capacity.min(buf.len());
        }
        self.capacity = capacity;
    }

    pub(crate) fn update_sample_rate(&mut self, sample_rate: NonZeroU32) {
        self.gate.update_sample_rate(sample_rate);
    }
}

/// Traverse the original leaders and handovers after any Sync cutover.
fn render_active_tracks(
    context: Option<&RenderContext>,
    tracks: &mut TrackSlots<{ PlayerNodeProcessor::MAX_TRACKS }>,
    order: &TrackOrder,
    attempt: &mut BlockAttempt,
    read_bufs: &mut [&mut [f32]],
    bus_bufs: &mut [&mut [f32]],
    sink: &mut RtSink<'_>,
) -> (bool, Option<(f64, f64)>) {
    let frames = read_bufs[0].len();
    let mut playback_started = false;
    let mut leading_outcome_pos_dur = None;
    let mut active_slots = [false; PlayerNodeProcessor::MAX_TRACKS];
    for (loaded_idx, _, _) in &order.active {
        active_slots[*loaded_idx] = true;
    }
    let mut skip_tracks = [false; PlayerNodeProcessor::MAX_TRACKS];

    for (track_idx, (_arena_slot, track_handle, was_leading)) in order.active.iter().enumerate() {
        if skip_tracks[track_idx] {
            continue;
        }

        let (mut read_outcome, handover_offset) = {
            let Some(track) = tracks.at_mut(*track_handle) else {
                continue;
            };
            let result = match (attempt.take_claimed(track.item_id()), &mut *attempt) {
                (Some(claimed), _) => claimed,
                (
                    None,
                    SyncAttempt::PrefixRendered {
                        item_id,
                        offset,
                        outcome,
                    },
                ) if *item_id == track.item_id() => {
                    let prefix = outcome.take();
                    match prefix {
                        Some(TrackReadOutcome::Full { .. }) => {
                            let suffix =
                                track.render(context, read_bufs, bus_bufs, *offset..frames, sink);
                            extend_outcome(*offset, suffix)
                        }
                        Some(other) => (other, None),
                        None => (
                            track.render(context, read_bufs, bus_bufs, 0..frames, sink),
                            None,
                        ),
                    }
                }
                _ => (
                    track.render(context, read_bufs, bus_bufs, 0..frames, sink),
                    None,
                ),
            };
            playback_started = true;
            result
        };

        if *was_leading {
            if let Some(snapshot) = outcome_position_duration(&read_outcome) {
                leading_outcome_pos_dur = Some(snapshot);
            }

            let mut handover = handover_offset
                .map(|offset| Handover { offset })
                .or_else(|| initial_handover(&read_outcome));

            for (next_idx, (_, next_handle, next_is_leading)) in order.active.iter().enumerate() {
                let Some(handoff) = handover else {
                    break;
                };
                let offset = handoff.offset;
                if next_idx == track_idx || skip_tracks[next_idx] || !*next_is_leading {
                    continue;
                }
                if offset >= frames {
                    break;
                }

                let Some(outcome) = tracks
                    .at_mut(*next_handle)
                    .map(|track| track.render(context, read_bufs, bus_bufs, offset..frames, sink))
                else {
                    continue;
                };
                read_outcome = outcome;
                skip_tracks[next_idx] = true;

                if let Some(snapshot) = outcome_position_duration(&read_outcome) {
                    leading_outcome_pos_dur = Some(snapshot);
                }

                handover = next_handover(&read_outcome, offset);
            }

            if let Some(handoff) = handover
                && handoff.offset < frames
            {
                let mut offset = handoff.offset;
                for (next_arena_idx, (next_handle, next_state)) in order.loaded.iter().enumerate() {
                    if *next_state != TrackState::Preloading || active_slots[next_arena_idx] {
                        continue;
                    }

                    let Some(next_track) = tracks.at_mut(*next_handle) else {
                        continue;
                    };
                    next_track.play();
                    let outcome =
                        next_track.render(context, read_bufs, bus_bufs, offset..frames, sink);
                    if let Some(snapshot) = outcome_position_duration(&outcome) {
                        leading_outcome_pos_dur = Some(snapshot);
                    }
                    match next_handover(&outcome, offset) {
                        Some(next) if next.offset < frames => offset = next.offset,
                        _ => break,
                    }
                }
            }
        }
    }

    (playback_started, leading_outcome_pos_dur)
}

/// Account for the PCM already rendered before a suffix read, so the normal
/// leading-track handover code sees one block-relative outcome.
fn extend_outcome(base: usize, suffix: TrackReadOutcome) -> (TrackReadOutcome, Option<usize>) {
    match suffix {
        TrackReadOutcome::Full {
            position,
            frames,
            duration,
            frames_until_eof,
        } => (
            TrackReadOutcome::Full {
                position,
                frames: base.saturating_add(frames),
                duration,
                frames_until_eof,
            },
            None,
        ),
        TrackReadOutcome::Partial { frames, duration } => (
            TrackReadOutcome::Partial {
                frames: base.saturating_add(frames),
                duration,
            },
            None,
        ),
        other @ (TrackReadOutcome::Eof | TrackReadOutcome::Failed(_)) if base > 0 => {
            (other, Some(base))
        }
        other => (other, None),
    }
}

const fn initial_handover(read_outcome: &TrackReadOutcome) -> Option<Handover> {
    match read_outcome {
        TrackReadOutcome::Partial { frames, .. } => Some(Handover { offset: *frames }),
        TrackReadOutcome::Eof | TrackReadOutcome::Failed(_) => Some(Handover { offset: 0 }),
        TrackReadOutcome::Full { .. } => None,
    }
}

const fn next_handover(read_outcome: &TrackReadOutcome, offset: usize) -> Option<Handover> {
    match read_outcome {
        TrackReadOutcome::Full { .. } => None,
        TrackReadOutcome::Partial { frames, .. } => Some(Handover {
            offset: offset.saturating_add(*frames),
        }),
        TrackReadOutcome::Eof | TrackReadOutcome::Failed(_) => Some(Handover { offset }),
    }
}

const fn outcome_position_duration(outcome: &TrackReadOutcome) -> Option<(f64, f64)> {
    match *outcome {
        TrackReadOutcome::Full {
            position, duration, ..
        } => Some((position, duration)),
        TrackReadOutcome::Partial { duration, .. } => Some((duration, duration)),
        TrackReadOutcome::Eof | TrackReadOutcome::Failed(_) => None,
    }
}

pub(super) const fn eviction_priority(state: TrackState) -> u8 {
    const EVICT_PRELOADING: u8 = 2;
    const EVICT_FADING_IN: u8 = 3;
    const EVICT_PLAYING: u8 = 4;

    match state {
        TrackState::Finished => 0,
        TrackState::FadingOut => 1,
        TrackState::Preloading => EVICT_PRELOADING,
        TrackState::FadingIn => EVICT_FADING_IN,
        TrackState::Playing => EVICT_PLAYING,
    }
}

#[cfg(test)]
mod sync_tests {
    use std::{num::NonZeroU32, sync::atomic::Ordering};

    use kithara_platform::sync::Arc;
    use kithara_signal::{OutputContext, SessionEpoch, SessionFrame, TransportRevision};
    use kithara_sync::{LoadedMedia, SyncReceipt, SyncReturn, activation_channels, sync_receipts};
    use kithara_test_utils::kithara;
    use ringbuf::{HeapRb, traits::Split};

    use super::*;
    use crate::{
        bridge::PlaybackShared,
        rt::sync_owner_fixture::{ReaderMode, entry_ticket, fresh_owner, resource},
    };

    fn activation_with_output(
        old_mode: ReaderMode,
        new_mode: ReaderMode,
        dependency: Option<TransportRevision>,
        actual_revision: Option<TransportRevision>,
        output_start: i64,
        revoke_during_prefix: bool,
    ) -> (BlockAttempt, [f32; 128], Vec<SyncReceipt>, bool) {
        let rate = NonZeroU32::new(48_000).expect("fixture rate");
        let can_reach_activation = output_start < 32
            && dependency.is_none_or(|revision| actual_revision == Some(revision));
        let new_read_required = can_reach_activation
            && !revoke_during_prefix
            && matches!(old_mode, ReaderMode::Silence);
        let new_duration_required = new_read_required && matches!(new_mode, ReaderMode::Silence);
        let item_id = TrackId::allocate();
        let load = LoadGeneration::first();
        let owner = fresh_owner();
        let revoke: Option<Arc<dyn Fn() + Send + Sync>> = revoke_during_prefix.then(|| {
            let owner = owner.clone();
            Arc::new(move || {
                owner
                    .revoke()
                    .expect("prefix runs before an audio claim, on a live permit");
            }) as Arc<dyn Fn() + Send + Sync>
        });
        let (ticket, _) = entry_ticket(
            owner,
            LoadedMedia::new(item_id, load),
            resource(
                new_mode,
                rate,
                new_read_required,
                new_duration_required,
                None,
            ),
            [0.0; 2],
            0,
            rate,
            dependency,
        );
        let map = ticket.first().head().activation().revision();
        let mut track = PlayerTrack::builder()
            .sample_rate(rate)
            .item_id(item_id)
            .load(load)
            .build(resource(old_mode, rate, can_reach_activation, true, revoke));
        track.play();
        let mut tracks = TrackSlots::<{ PlayerNodeProcessor::MAX_TRACKS }>::default();
        assert!(tracks.insert(track).is_none());
        let (mut control, audio) = activation_channels::<PlayerSync>();
        assert!(
            matches!(control.hand(ticket), Ok(None)),
            "an empty deck takes a ticket"
        );
        let (mut notification_tx, _notification_rx) = HeapRb::<PlayerNotification>::new(16).split();
        let (receipt_tx, mut receipt_rx) = sync_receipts();
        let mut sync = PlaySync::new(audio, Some(receipt_tx), None);
        let playback = PlaybackShared::default();
        let metrics = RtMetrics::default();
        let output = OutputContext::new(
            SessionFrame::new(output_start)..SessionFrame::new(output_start + 128),
            rate,
            SessionEpoch::new(1),
            actual_revision,
        )
        .expect("fixture output");
        let context = RenderContext::new_linear(output, None).expect("fixture context");
        let mut read_left = [0.0; 128];
        let mut read_right = [0.0; 128];
        let mut bus_left = [0.0; 128];
        let mut bus_right = [0.0; 128];
        let mut read = [&mut read_left[..], &mut read_right[..]];
        let mut bus = [&mut bus_left[..], &mut bus_right[..]];
        let mut sink = RtSink::new(&mut notification_tx, &metrics, 0);
        let mut deck = PlayDeck {
            tracks: &mut tracks,
            read_bufs: &mut read,
            bus_bufs: &mut bus,
            sink: &mut sink,
            playback: &playback,
        };
        let attempt = sync.attempt(&mut deck, Some(&context), 128);
        if matches!(&attempt, SyncAttempt::Claimed { .. }) {
            assert_eq!(
                playback.active_sync_map.load(Ordering::Acquire),
                u64::from(map)
            );
        }
        let mut delivered = Vec::new();
        while let Some(receipt) = receipt_rx.next_receipt() {
            delivered.push(receipt);
        }
        let pending_remains = !matches!(&attempt, SyncAttempt::Claimed { .. })
            && !matches!(control.next_return(), Some(SyncReturn::Ticket(_)));
        (attempt, bus_left, delivered, pending_remains)
    }

    fn activation(
        old_mode: ReaderMode,
        new_mode: ReaderMode,
    ) -> (BlockAttempt, [f32; 128], Vec<SyncReceipt>) {
        let (attempt, pcm, receipts, _) = activation_with_output(
            old_mode,
            new_mode,
            Some(TransportRevision::first()),
            Some(TransportRevision::first()),
            0,
            false,
        );
        (attempt, pcm, receipts)
    }

    #[kithara::test]
    fn host_ticket_waits_for_its_processed_revision_before_late_check() {
        let later = TransportRevision::first().checked_next().expect("revision");
        let (attempt, pcm, receipts, pending) = activation_with_output(
            ReaderMode::Silence,
            ReaderMode::Silence,
            Some(TransportRevision::first()),
            Some(later),
            128,
            false,
        );
        assert!(matches!(attempt, SyncAttempt::None));
        assert!(
            pending,
            "the owner must reissue or withdraw this parked ticket"
        );
        assert!(receipts.is_empty());
        assert_eq!(pcm, [0.0; 128]);
    }

    #[kithara::test]
    fn output_independent_ticket_can_claim_after_a_global_revision_change() {
        let later = TransportRevision::first().checked_next().expect("revision");
        let (attempt, _, receipts, pending) = activation_with_output(
            ReaderMode::Silence,
            ReaderMode::Silence,
            None,
            Some(later),
            0,
            false,
        );
        assert!(matches!(attempt, SyncAttempt::Claimed { .. }));
        assert!(!pending);
        assert_first_span_pair(&receipts);
    }

    #[kithara::test]
    fn owner_withdrawal_during_the_old_prefix_returns_ticket_without_stale_receipt() {
        let (attempt, _, receipts, pending) = activation_with_output(
            ReaderMode::Silence,
            ReaderMode::Silence,
            None,
            Some(TransportRevision::first()),
            0,
            true,
        );
        assert!(matches!(
            attempt,
            SyncAttempt::PrefixRendered { offset: 32, .. }
        ));
        assert!(!pending, "the withdrawn ticket leaves the callback ring");
        assert!(
            receipts.is_empty(),
            "the owner already withdrew this exact preparation"
        );
    }

    fn assert_first_span_pair(receipts: &[SyncReceipt]) {
        let [SyncReceipt::Armed(stamp), SyncReceipt::Presented(applied)] = receipts else {
            panic!("one consumed first frame must publish exactly Armed and Presented");
        };
        assert_eq!(*stamp, applied.stamp());
        assert_eq!(applied.frontier().source(), 1);
        assert_eq!(applied.frontier().output(), SessionFrame::new(33));
    }

    #[kithara::test]
    fn suffix_decode_failure_keeps_failure_position_and_exact_receipt_pair() {
        let (attempt, pcm, receipts) = activation(ReaderMode::Silence, ReaderMode::Failure);
        let SyncAttempt::Claimed {
            outcome,
            handover_offset,
            ..
        } = attempt
        else {
            panic!("first PCM must claim");
        };
        assert!(matches!(outcome, TrackReadOutcome::Failed(_)));
        assert_eq!(handover_offset, Some(33));
        assert_eq!(outcome_position_duration(&outcome), None);
        assert_eq!(pcm, [0.0; 128]);
        assert_first_span_pair(&receipts);
    }

    #[kithara::test]
    #[case(ReaderMode::Eof)]
    #[case(ReaderMode::Failure)]
    fn preclaim_prefix_end_keeps_old_audio_and_never_presents(#[case] old: ReaderMode) {
        let (attempt, pcm, receipts) = activation(old, ReaderMode::Silence);
        assert!(matches!(attempt, SyncAttempt::PrefixRendered { .. }));
        assert_eq!(pcm, [0.0; 128]);
        assert!(matches!(
            receipts.as_slice(),
            [SyncReceipt::Rejected { .. }]
        ));
    }

    #[kithara::test]
    fn preclaim_prefix_partial_keeps_existing_pcm_without_claim() {
        let (attempt, pcm, receipts) = activation(ReaderMode::ShortThenEof, ReaderMode::Silence);
        assert!(matches!(
            attempt,
            SyncAttempt::PrefixRendered {
                outcome: Some(TrackReadOutcome::Partial { frames: 16, .. }),
                ..
            }
        ));
        assert!(pcm[..16].iter().all(|&sample| sample == 0.5));
        assert!(pcm[16..].iter().all(|&sample| sample == 0.0));
        assert!(matches!(
            receipts.as_slice(),
            [SyncReceipt::Rejected { .. }]
        ));
    }

    #[kithara::test]
    fn suffix_eof_keeps_handover_after_consumed_first_frame() {
        let (attempt, _, receipts) = activation(ReaderMode::Silence, ReaderMode::Eof);
        let SyncAttempt::Claimed {
            outcome,
            handover_offset,
            ..
        } = attempt
        else {
            panic!("first PCM must claim");
        };
        assert!(matches!(outcome, TrackReadOutcome::Eof));
        assert_eq!(handover_offset, Some(33));
        assert_eq!(outcome_position_duration(&outcome), None);
        assert_first_span_pair(&receipts);
    }
}
