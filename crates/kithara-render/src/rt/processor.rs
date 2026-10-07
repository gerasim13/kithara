use std::{
    num::{NonZeroU32, NonZeroUsize},
    ops::Range,
};

use firewheel::{
    StreamInfo,
    node::{
        AudioNodeProcessor, ProcBuffers, ProcExtra, ProcInfo, ProcStore, ProcStreamCtx,
        ProcessStatus,
    },
};
use kithara_bufpool::{HasPool, PoolRegion, SampleBuffer};
use kithara_command::{Inbox, Seq, Target};
use kithara_dsp::param::SmootherConfig;
use kithara_signal::SessionFrame;
use kithara_test_utils::kithara;
use kithara_warp::RenderContext;
use num_traits::cast::AsPrimitive;
use ringbuf::HeapProd;
use tracing::warn;
use triple_buffer::Input;

use super::{
    command::Orphan,
    context::read_render_context,
    tail::SlotTail,
    track::{PlayerTrack, RtSink},
};
use crate::{
    bridge::{
        DeckEvent, DeckMixSettings, DeckProtocol, DeckSnapshot, MixerInputs, RtMetrics, Slot,
        SlotSnapshot, SlotState,
    },
    rt::{RenderPass, TrackSlots},
};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) enum ContextRequirement {
    #[default]
    Standalone,
    Session,
}

/// The realtime mixer of one deck.
///
/// Takes the deck's batches from its inbox at the frames they apply on, renders the tracks of
/// its slots between them into the Firewheel output buffers, reports what its slots did as
/// events, and publishes a snapshot once per block.
pub struct DeckMixer {
    inbox: Inbox<DeckProtocol>,
    deck: Deck,
    /// Per-channel buffers a track reads into before it is mixed.
    scratch: [SampleBuffer; MIN_STEREO],
    /// Frames the scratch holds; a longer block is clamped.
    capacity: usize,
    snapshot: Input<DeckSnapshot>,
    blocks: u64,
    context_requirement: ContextRequirement,
}

const MIN_STEREO: usize = 2;

/// The tracks a deck's slots hold, how they mix, and what the mixer reports of them.
pub(super) struct Deck {
    pub(super) tracks: TrackSlots,
    /// The batch chained behind each slot's end, and the slot it starts.
    pub(super) chains: Vec<Option<(Slot, Seq)>>,
    /// What each slot still sounds of a consumer a `Replace` took out of it.
    pub(super) tails: Vec<Option<SlotTail>>,
    /// Chained batches a detach left without their slot, answered once the batch is.
    pub(super) orphans: Vec<Orphan>,
    /// Which slots hold a track as a batch's parts are checked, one entry per slot.
    pub(super) held: Vec<bool>,
    /// Which slots rendered in the current range, one entry per slot.
    rendered: Vec<bool>,
    /// How loud the deck sounds, as its parts last set it.
    pub(super) mix: DeckMixSettings,
    pub(super) render: RenderPass,
    events: HeapProd<DeckEvent>,
    pub(super) metrics: RtMetrics,
    pub(super) sample_rate: NonZeroU32,
    /// The ramp every track starts and stops with.
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

impl DeckMixer {
    /// Create a mixer over the channel ends `inputs`, built as their config says.
    #[must_use]
    pub fn new<S>(inputs: MixerInputs, shape: StreamShape, pools: &PoolRegion<S>) -> Self
    where
        S: HasPool<f32>,
    {
        Self::with_context_requirement(inputs, shape, pools, ContextRequirement::Standalone)
    }

    pub(super) fn with_context_requirement<S>(
        inputs: MixerInputs,
        shape: StreamShape,
        pools: &PoolRegion<S>,
        context_requirement: ContextRequirement,
    ) -> Self
    where
        S: HasPool<f32>,
    {
        let MixerInputs {
            inbox,
            events,
            snapshot,
            config,
        } = inputs;
        let slots = config.slots();
        let mix = config.mix();
        let tails = (0..slots.get())
            .map(|_| {
                SlotTail::new(pools, config.evict_fade())
                    .inspect_err(|error| {
                        warn!(%error, "sample pool budget cannot afford a slot tail; replace cuts");
                    })
                    .ok()
            })
            .collect();
        let mut mixer = Self {
            inbox,
            snapshot,
            context_requirement,
            scratch: std::array::from_fn(|_| pools.get::<f32>()),
            capacity: 0,
            blocks: 0,
            deck: Deck {
                tracks: TrackSlots::new(slots),
                chains: vec![None; slots.get()],
                tails,
                orphans: Vec::with_capacity(slots.get()),
                held: Vec::with_capacity(slots.get()),
                rendered: vec![false; slots.get()],
                render: RenderPass::new(pools, shape, mix.gain()),
                mix,
                events,
                metrics: RtMetrics::default(),
                sample_rate: shape.sample_rate,
                declick: config.declick(),
            },
        };
        mixer.resize(shape.max_block_frames.get().as_());
        mixer
    }

    /// Renders one block of `frames` frames starting at `start` on the session clock, without a
    /// render context: the deck's batches apply at their frames, and the slots render between
    /// them. Returns whether any slot sounded.
    pub fn render_block(
        &mut self,
        start: SessionFrame,
        buffers: &mut ProcBuffers,
        frames: usize,
    ) -> bool {
        self.render_block_in(None, start, buffers, frames)
    }

    /// The track `slot` holds.
    #[must_use]
    pub fn track(&self, slot: Slot) -> Option<&PlayerTrack> {
        self.deck.tracks.at(slot)
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
        let Self {
            inbox,
            deck,
            scratch,
            capacity,
            ..
        } = self;
        inbox.drain();
        let mut reached = 0;
        let mut sounded = false;
        loop {
            let due = inbox
                .frames_until_due(start)
                .map_or(frames, |due| usize::try_from(due).unwrap_or(usize::MAX));
            let boundary = due.clamp(reached, frames);
            if boundary > reached {
                sounded |= deck.render_range(
                    context,
                    buffers,
                    (scratch, *capacity),
                    reached..boundary,
                    (inbox, start),
                );
                reached = boundary;
            }
            if reached >= frames {
                break;
            }
            let Some(due) = inbox.next_due(start, frames) else {
                continue;
            };
            let at = due.at();
            deck.take_due(due);
            deck.resolve_orphans(inbox, at);
        }
        self.blocks = self.blocks.saturating_add(1);
        self.publish();
        sounded
    }

    /// Publish what the slots stand at after the block.
    fn publish(&mut self) {
        let snapshot = self.snapshot.input_buffer_mut();
        for (entry, slot) in snapshot.slots.iter_mut().zip(self.deck.tracks.slots()) {
            *entry = self
                .deck
                .tracks
                .at(slot)
                .map_or_else(SlotSnapshot::default, slot_snapshot);
        }
        snapshot.sample_rate = self.deck.sample_rate.get();
        snapshot.blocks = self.blocks;
        snapshot.metrics = self.deck.metrics.snapshot();
        self.snapshot.publish();
    }

    fn resize(&mut self, max_frames: usize) {
        let mut capacity = usize::MAX;
        for buf in &mut self.scratch {
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
}

fn slot_snapshot(track: &PlayerTrack) -> SlotSnapshot {
    let position = track.position();
    SlotSnapshot {
        state: track.state(),
        position,
        duration: track.duration(),
        frontier: track.decoded_frontier().max(position),
        cached: track.cached_span(),
        gain: track.gain(),
        rate: track.playback_rate(),
    }
}

impl Deck {
    /// Render the playing slots over `range` of the block into the same frames of the output
    /// buffers, firing the chain behind each slot that ends inside it.
    ///
    /// Frames are clamped rather than grown, since growing a pooled buffer here would allocate on
    /// the audio thread; frames past the clamp are already silence-filled.
    fn render_range(
        &mut self,
        context: Option<&RenderContext>,
        buffers: &mut ProcBuffers,
        (scratch, capacity): (&mut [SampleBuffer; MIN_STEREO], usize),
        range: Range<usize>,
        (inbox, start): (&mut Inbox<DeckProtocol>, SessionFrame),
    ) -> bool {
        if buffers.outputs.len() < MIN_STEREO {
            return false;
        }
        for channel in buffers.outputs.iter_mut() {
            channel[range.clone()].fill(0.0);
        }
        let frames = range.end.min(capacity);
        let begin = range.start.min(frames);
        if self.render.take_priming() {
            for (_, track) in self.tracks.iter_mut() {
                track.snap_gate();
            }
        }
        let [scratch_left, scratch_right] = scratch;
        let mut read = [&mut scratch_left[..frames], &mut scratch_right[..frames]];
        let (out_left, out_right) = buffers.outputs.split_at_mut(1);
        let mut bus = [&mut out_left[0][..frames], &mut out_right[0][..frames]];
        self.rendered.fill(false);
        let mut sounded = false;

        for first in self.tracks.slots() {
            let mut slot = first;
            let mut span = begin..frames;
            loop {
                if self.rendered.get(slot.index()).copied().unwrap_or(true) {
                    break;
                }
                let Some(track) = self
                    .tracks
                    .at_mut(slot)
                    .filter(|track| track.state() == SlotState::Playing)
                else {
                    break;
                };
                if let Some(rendered) = self.rendered.get_mut(slot.index()) {
                    *rendered = true;
                }
                let mut sink = RtSink::new(&mut self.events, &self.metrics, slot, start);
                let outcome = track.render(context, &mut read, &mut bus, span.clone(), &mut sink);
                sounded = true;
                let Some(end) = outcome.ended_at(&span) else {
                    break;
                };
                let at = sink.at(end);
                let Some(to) = self.fire_chain(inbox, slot, at) else {
                    break;
                };
                if end >= span.end {
                    break;
                }
                slot = to;
                span = end..span.end;
            }
        }

        let [bus_left, bus_right] = &mut bus;
        for tail in self.tails.iter_mut().flatten() {
            if tail.is_sounding() {
                tail.mix(bus_left, bus_right, begin..frames);
                sounded = true;
            }
        }
        if sounded {
            self.render
                .finish(&mut bus_left[begin..], &mut bus_right[begin..]);
        } else {
            self.render.idle();
        }
        sounded
    }

    fn update_host_sample_rate(&mut self, sample_rate: NonZeroU32) {
        if self.sample_rate == sample_rate {
            return;
        }
        self.sample_rate = sample_rate;
        for (_, track) in self.tracks.iter_mut() {
            track.set_host_sample_rate(sample_rate);
        }
        self.render.update_sample_rate(sample_rate);
    }
}

impl AudioNodeProcessor for DeckMixer {
    fn new_stream(&mut self, stream_info: &StreamInfo, _context: &mut ProcStreamCtx) {
        self.deck.update_host_sample_rate(stream_info.sample_rate);
        self.resize(stream_info.max_block_frames.get().as_());
    }

    #[kithara::rtsan_forbid_blocking]
    fn process(
        &mut self,
        info: &ProcInfo,
        mut buffers: ProcBuffers,
        extra: &mut ProcExtra,
    ) -> ProcessStatus {
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
mod tests;
