use std::{
    marker::PhantomData,
    num::{NonZeroU32, NonZeroUsize},
};

use firewheel::{
    StreamInfo,
    node::{AudioNodeProcessor, ProcBuffers, ProcExtra, ProcInfo, ProcStreamCtx, ProcessStatus},
};
use kithara_bufpool::{HasPool, PoolError, PoolRegion, SampleBuffer};
use kithara_command::{LevelInbox, ScopeId, Seq, Target};
use kithara_dsp::param::SmootherConfig;
use kithara_signal::{FrameCount, SegmentId, SessionFrame};
use kithara_test_utils::kithara;
use kithara_warp::RenderContext;
use ringbuf::HeapProd;
use triple_buffer::Input;

use super::{
    command::Armed,
    context::read_render_context,
    tail::SlotTail,
    track::{PlayerTrack, RtSink},
};
use crate::{
    bridge::{
        DeckEvent, DeckMixSettings, DeckProtocol, DeckSnapshot, MixerInputs, RtMetrics,
        SessionInbox, Slot, SlotSnapshot, SlotState,
    },
    rt::{RenderPass, TrackSlots},
};

/// The realtime executor of one generation of a session's deck scope.
pub struct DeckMixer<E: SessionInbox> {
    scope: ScopeId,
    deck: Deck,
    scratch: [SampleBuffer; MIN_STEREO],
    capacity: usize,
    snapshot: Input<DeckSnapshot>,
    blocks: u64,
    recycle_per_block: usize,
    retired: bool,
    session: PhantomData<fn() -> E>,
}

const MIN_STEREO: usize = 2;

pub(super) struct Deck {
    pub(super) tracks: TrackSlots,
    pub(super) armed: Vec<Option<Armed>>,
    pub(super) ended: Vec<bool>,
    pub(super) tails: Vec<SlotTail>,
    pub(super) held: Vec<Option<SegmentId>>,
    pub(super) stops: Vec<Option<Seq>>,
    pub(super) interrupted: Vec<Option<(Seq, crate::bridge::SlotMark)>>,
    pub(super) recycle: Vec<usize>,
    pub(super) declick_frames: usize,
    pub(super) evict_frames: usize,
    pub(super) mix: DeckMixSettings,
    pub(super) render: RenderPass,
    events: HeapProd<DeckEvent>,
    pub(super) metrics: RtMetrics,
    pub(super) sample_rate: NonZeroU32,
    pub(super) declick: SmootherConfig,
}

/// Why a stream's geometry cannot size a deck's decoder buffers.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum BufferGeometryError {
    #[error("deck buffer geometry overflowed")]
    Overflow,
    #[error(
        "deck needs {required_frames} response frames for block {max_block_frames} and quantum {render_quantum_frames}, exceeding budget {budget_frames}"
    )]
    BudgetExceeded {
        max_block_frames: u32,
        render_quantum_frames: usize,
        required_frames: usize,
        budget_frames: usize,
    },
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
    ) -> Result<(NonZeroUsize, NonZeroUsize), BufferGeometryError> {
        let output_frames = usize::try_from(self.max_block_frames.get())
            .map_err(|_| BufferGeometryError::Overflow)?;
        let preload = output_frames.div_ceil(quantum.get());
        let ring = preload
            .checked_add(1)
            .ok_or(BufferGeometryError::Overflow)?;
        let required_frames = ring
            .checked_add(1)
            .and_then(|chunks| chunks.checked_mul(quantum.get()))
            .and_then(|frames| frames.checked_sub(1))
            .ok_or(BufferGeometryError::Overflow)?;
        if let Some(budget) = budget
            && required_frames > budget.get()
        {
            return Err(BufferGeometryError::BudgetExceeded {
                required_frames,
                max_block_frames: self.max_block_frames.get(),
                render_quantum_frames: quantum.get(),
                budget_frames: budget.get(),
            });
        }
        Ok((
            NonZeroUsize::new(preload).ok_or(BufferGeometryError::Overflow)?,
            NonZeroUsize::new(ring).ok_or(BufferGeometryError::Overflow)?,
        ))
    }
}

impl<E: SessionInbox> DeckMixer<E> {
    /// Allocates every slot tail and the fixed render scratch before stream processing.
    ///
    /// # Errors
    /// Returns a pool error if either scratch or any required tail cannot be allocated.
    pub fn new<S>(
        inputs: MixerInputs,
        shape: StreamShape,
        pools: &PoolRegion<S>,
    ) -> Result<Self, PoolError>
    where
        S: HasPool<f32>,
    {
        let MixerInputs {
            scope,
            events,
            snapshot,
            config,
        } = inputs;
        let slots = config.slots();
        let capacity = usize::try_from(shape.max_block_frames.get()).unwrap_or(usize::MAX);
        let declick_frames = config.declick_frames(shape.sample_rate).get();
        let evict_frames = config.evict_fade().get();
        let tail_frames = FrameCount::new(declick_frames.max(evict_frames));
        let tails = (0..slots.get())
            .map(|_| SlotTail::new(pools, tail_frames))
            .collect::<Result<Vec<_>, _>>()?;
        let scratch = [
            pools.get_with_len::<f32>(capacity)?,
            pools.get_with_len::<f32>(capacity)?,
        ];
        let mix = config.mix();
        Ok(Self {
            scope,
            snapshot,
            scratch,
            capacity,
            blocks: 0,
            recycle_per_block: config.recycle_per_block().get(),
            retired: false,
            session: PhantomData,
            deck: Deck {
                tracks: TrackSlots::new(slots),
                armed: vec![None; slots.get()],
                ended: vec![false; slots.get()],
                tails,
                held: vec![None; slots.get()],
                stops: vec![None; slots.get()],
                interrupted: vec![None; slots.get()],
                recycle: vec![0; slots.get()],
                declick_frames,
                evict_frames,
                render: RenderPass::new(pools, shape, mix.gain(), config)?,
                mix,
                events,
                metrics: RtMetrics::default(),
                sample_rate: shape.sample_rate,
                declick: config.declick(),
            },
        })
    }

    #[must_use]
    pub fn track(&self, slot: Slot) -> Option<&PlayerTrack> {
        if self.retired {
            None
        } else {
            self.deck.tracks.at(slot)
        }
    }

    fn render_block_in(
        &mut self,
        level: &mut LevelInbox<'_, DeckProtocol>,
        context: Option<&RenderContext>,
        start: SessionFrame,
        buffers: &mut ProcBuffers,
        frames: usize,
    ) -> bool {
        self.deck.recycle.fill(self.recycle_per_block);
        self.deck.arrivals(level);
        let mut sounded = false;
        for cursor in 0..frames {
            let at = SessionFrame::new(
                i64::from(start).saturating_add(i64::try_from(cursor).unwrap_or(i64::MAX)),
            );
            loop {
                let Some(due) = level.next_due(start, cursor.saturating_add(1)) else {
                    break;
                };
                self.deck.take_due(due, context.is_some());
                self.deck.finish_stops(level);
                self.deck.resolve_armed(level, start, at);
            }
            self.deck.maintain();
            self.deck.finish_stops(level);
            if context.is_some() {
                self.deck.observe_ends(cursor, start);
            }
            self.deck.fire_ended(level, start, at, context.is_some());
            self.deck.finish_stops(level);
            sounded |= self.deck.render_frame(
                context,
                buffers,
                &mut self.scratch,
                self.capacity,
                cursor,
                start,
            );
            self.deck.finish_stops(level);
        }
        self.deck.maintain();
        self.deck.finish_stops(level);
        sounded
    }

    fn publish(&mut self, at: SessionFrame) {
        let snapshot = self.snapshot.input_buffer_mut();
        for (entry, slot) in snapshot.slots.iter_mut().zip(self.deck.tracks.slots()) {
            *entry = if self.retired {
                SlotSnapshot::default()
            } else {
                self.deck
                    .tracks
                    .at(slot)
                    .map_or_else(SlotSnapshot::default, |track| slot_snapshot(track, at))
            };
        }
        snapshot.sample_rate = self.deck.sample_rate.get();
        snapshot.blocks = self.blocks;
        snapshot.metrics = self.deck.metrics.snapshot();
        let bands = self.deck.render.read_eq(snapshot.eq.gains_mut());
        snapshot.eq.set_bands(bands);
        self.snapshot.publish();
    }
}

fn slot_snapshot(track: &PlayerTrack, at: SessionFrame) -> SlotSnapshot {
    let position = track.position();
    SlotSnapshot {
        state: track.state(),
        mark: track.mark(at),
        position,
        duration: track.duration(),
        frontier: track.decoded_frontier().max(position),
        cached: track.cached_span(),
        gain: track.gain(),
    }
}

impl Deck {
    fn maintain(&mut self) {
        for (slot, track) in self.tracks.iter_mut() {
            track.recycle_obsolete(&mut self.recycle[slot.index()]);
        }
    }

    fn observe_ends(&mut self, cursor: usize, start: SessionFrame) {
        for (slot, track) in self.tracks.iter_mut() {
            let mut sink = RtSink::new(&mut self.events, &self.metrics, slot, start);
            self.ended[slot.index()] =
                track.poll_end(cursor, &mut self.recycle[slot.index()], &mut sink);
        }
    }

    fn render_frame(
        &mut self,
        context: Option<&RenderContext>,
        buffers: &mut ProcBuffers,
        scratch: &mut [SampleBuffer; MIN_STEREO],
        capacity: usize,
        cursor: usize,
        start: SessionFrame,
    ) -> bool {
        if context.is_none()
            || cursor >= capacity
            || buffers.outputs.len() < MIN_STEREO
            || buffers
                .outputs
                .iter()
                .take(MIN_STEREO)
                .any(|channel| cursor >= channel.len())
        {
            for tail in &mut self.tails {
                tail.advance(1);
            }
            return false;
        }
        if self.render.take_priming() {
            for (_, track) in self.tracks.iter_mut() {
                track.snap_gate();
            }
        }
        let range = cursor..cursor.saturating_add(1);
        let [scratch_left, scratch_right] = scratch;
        let mut read = [
            &mut scratch_left[..capacity],
            &mut scratch_right[..capacity],
        ];
        let (out_left, out_right) = buffers.outputs.split_at_mut(1);
        let mut bus = [&mut out_left[0][..], &mut out_right[0][..]];
        let mut sounded = false;
        for (slot, track) in self.tracks.iter_mut() {
            if track.state() != SlotState::Playing {
                continue;
            }
            let mut sink = RtSink::new(&mut self.events, &self.metrics, slot, start);
            let _ = track.render(
                context,
                &mut read,
                &mut bus,
                range.clone(),
                &mut self.recycle[slot.index()],
                &mut sink,
            );
            sounded = true;
        }
        let [left, right] = &mut bus;
        for tail in &mut self.tails {
            if tail.is_sounding() {
                tail.mix(left, right, range.clone());
                sounded = true;
            }
        }
        if sounded {
            self.render
                .finish(&mut left[range.clone()], &mut right[range]);
        } else {
            self.render.idle();
        }
        sounded
    }

    fn tails_quiet(&self) -> bool {
        self.tails.iter().all(|tail| !tail.is_sounding())
    }

    fn update_host_sample_rate(&mut self, sample_rate: NonZeroU32) {
        if self.sample_rate == sample_rate {
            return;
        }
        self.sample_rate = sample_rate;
        self.declick_frames = super::config::declick_frame_count(self.declick, sample_rate).get();
        for (_, track) in self.tracks.iter_mut() {
            track.set_host_sample_rate(sample_rate);
        }
        self.render.update_sample_rate(sample_rate);
    }
}

impl<E: SessionInbox> AudioNodeProcessor for DeckMixer<E> {
    fn new_stream(&mut self, stream_info: &StreamInfo, _context: &mut ProcStreamCtx) {
        self.deck.update_host_sample_rate(stream_info.sample_rate);
    }

    #[kithara::rtsan_forbid_blocking]
    fn process(
        &mut self,
        info: &ProcInfo,
        mut buffers: ProcBuffers,
        extra: &mut ProcExtra,
    ) -> ProcessStatus {
        for channel in buffers.outputs.iter_mut() {
            channel.fill(0.0);
        }
        let start = SessionFrame::new(info.clock_samples.0);
        let geometry_valid = info.frames <= self.capacity
            && buffers.outputs.len() >= MIN_STEREO
            && buffers
                .outputs
                .iter()
                .take(MIN_STEREO)
                .all(|channel| channel.len() >= info.frames)
            && self.deck.tails.iter().all(|tail| {
                tail.capacity() >= self.deck.declick_frames.max(self.deck.evict_frames)
            });
        let context = read_render_context(&extra.store, info)
            .ok()
            .filter(|_| geometry_valid)
            .cloned();
        let mut sounded = false;
        if !self.retired {
            if let Some(mut level) = extra
                .store
                .try_get_mut::<E>()
                .and_then(|session| session.scope(self.scope))
            {
                sounded = self.render_block_in(
                    &mut level,
                    context.as_ref(),
                    start,
                    &mut buffers,
                    info.frames,
                );
                if level.is_closing() && self.deck.tails_quiet() {
                    level.retire();
                    self.retired = true;
                }
            } else {
                self.retired = true;
            }
            if self.retired {
                for (_, track) in self.deck.tracks.iter_mut() {
                    track.shut();
                }
            }
        }
        self.blocks = self.blocks.saturating_add(1);
        let end = SessionFrame::new(
            i64::from(start).saturating_add(i64::try_from(info.frames).unwrap_or(i64::MAX)),
        );
        self.publish(end);
        if sounded {
            ProcessStatus::OutputsModified
        } else {
            ProcessStatus::ClearAllOutputs
        }
    }
}

#[cfg(test)]
mod tests;
