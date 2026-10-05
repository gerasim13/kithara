use std::{
    num::{NonZeroU32, NonZeroUsize},
    ops::Range,
};

use firewheel::node::ProcBuffers;
use kithara_bufpool::{HasPool, PoolRegion, SampleBuffer};
use kithara_dsp::param::{SmoothedParam, SmootherConfig};
use kithara_warp::RenderContext;
use num_traits::cast::AsPrimitive;
use ringbuf::HeapProd;
use tracing::warn;

use super::{
    DeckMixerConfig,
    processor::StreamShape,
    track::{PlayerTrack, RtSink, TrackReadOutcome},
};
use crate::{
    bridge::{PlayerNotification, RtMetrics, TrackState},
    rt::{TrackSlot, TrackSlots},
};

type ActiveTrackEntry = (TrackSlot, bool);

#[derive(Clone, Copy)]
struct Handover {
    offset: usize,
}

pub(crate) struct RenderTargets<'a> {
    pub(crate) notification_tx: &'a mut HeapProd<PlayerNotification>,
    pub(crate) metrics: &'a RtMetrics,
    pub(crate) tracks: &'a mut TrackSlots,
    /// Slot seek epoch published when this block started rendering.
    pub(crate) seek_epoch: u64,
}

pub(crate) struct RenderPass {
    /// The deck's output gain, ramped to each new target from the frame it is set on.
    gain: SmoothedParam,
    scratch_bufs: [SampleBuffer; Self::MIN_STEREO],
    range_tracks: RangeTracks,
    /// Set until the first range renders, which moves every ramp to its target at once.
    priming: bool,
    capacity: usize,
}

impl RenderPass {
    /// A deck's gain runs from silence to unity.
    const GAIN_SPAN: f32 = 1.0;
    const MIN_STEREO: usize = 2;

    pub(crate) fn new<S>(
        pools: &PoolRegion<S>,
        shape: StreamShape,
        config: DeckMixerConfig,
        gain: f32,
    ) -> Self
    where
        S: HasPool<f32>,
    {
        let mut pass = Self {
            gain: SmoothedParam::new(
                gain,
                Self::GAIN_SPAN,
                SmootherConfig::default(),
                shape.sample_rate,
            ),
            scratch_bufs: std::array::from_fn(|_| pools.get::<f32>()),
            range_tracks: RangeTracks::new(config.slots()),
            capacity: 0,
            priming: true,
        };
        pass.resize(shape.max_block_frames.get().as_());
        pass
    }

    /// Render the playing, unstopped tracks over `range` of the block into the same frames of the
    /// output buffers.
    ///
    /// Frames are clamped rather than grown, since growing a pooled buffer here would allocate on
    /// the audio thread; frames past the clamp are already silence-filled.
    pub(crate) fn render_range(
        &mut self,
        context: Option<&RenderContext>,
        targets: RenderTargets<'_>,
        buffers: &mut ProcBuffers,
        range: Range<usize>,
    ) -> (bool, Option<(f64, f64)>) {
        let mut playback_started = false;
        let mut leading_outcome_pos_dur: Option<(f64, f64)> = None;

        if buffers.outputs.len() < Self::MIN_STEREO {
            return (false, None);
        }

        for ch_buffer in buffers.outputs.iter_mut() {
            ch_buffer[range.clone()].fill(0.0);
        }

        let frames = range.end.min(self.capacity);
        let start = range.start.min(frames);
        let tracks = targets.tracks;
        if self.priming {
            self.priming = false;
            for (_, track) in tracks.iter_mut() {
                track.snap_gate();
            }
            self.gain.reset_to_target();
        }
        self.range_tracks.refill(tracks);
        if self.range_tracks.active.is_empty() {
            self.gain.reset_to_target();
            return (false, None);
        }

        let [read_buf0, read_buf1] = &mut self.scratch_bufs;
        let mut read_bufs = [&mut read_buf0[..frames], &mut read_buf1[..frames]];
        let (out_left, out_right) = buffers.outputs.split_at_mut(1);
        let mut bus_bufs = [&mut out_left[0][..frames], &mut out_right[0][..frames]];
        let mut sink = RtSink::new(targets.notification_tx, targets.metrics, targets.seek_epoch);
        let RangeTracks {
            active: active_tracks,
            skip: skip_tracks,
        } = &mut self.range_tracks;

        for (track_idx, (track_handle, was_leading)) in active_tracks.iter().enumerate() {
            if skip_tracks[track_idx] {
                continue;
            }

            let mut read_outcome = {
                let Some(outcome) = tracks.at_mut(*track_handle).map(|track| {
                    track.render(
                        context,
                        &mut read_bufs,
                        &mut bus_bufs,
                        start..frames,
                        &mut sink,
                    )
                }) else {
                    continue;
                };
                playback_started = true;
                outcome
            };

            if *was_leading {
                if let Some(snapshot) = outcome_position_duration(&read_outcome) {
                    leading_outcome_pos_dur = Some(snapshot);
                }

                let mut handover = next_handover(&read_outcome, start);
                let mut ending = *track_handle;

                for (next_idx, (next_handle, next_is_leading)) in active_tracks.iter().enumerate() {
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

                    let Some(outcome) = tracks.at_mut(*next_handle).map(|track| {
                        track.render(
                            context,
                            &mut read_bufs,
                            &mut bus_bufs,
                            offset..frames,
                            &mut sink,
                        )
                    }) else {
                        continue;
                    };
                    read_outcome = outcome;
                    skip_tracks[next_idx] = true;
                    ending = *next_handle;

                    if let Some(snapshot) = outcome_position_duration(&read_outcome) {
                        leading_outcome_pos_dur = Some(snapshot);
                    }

                    handover = next_handover(&read_outcome, offset);
                }

                if let Some(handoff) = handover
                    && handoff.offset < frames
                {
                    let mut offset = handoff.offset;
                    while let Some(next_handle) = tracks
                        .at(ending)
                        .and_then(PlayerTrack::successor)
                        .and_then(|successor| tracks.slot_of(successor))
                    {
                        let Some(next_track) = tracks
                            .at_mut(next_handle)
                            .filter(|track| track.state() == TrackState::Preloading)
                        else {
                            break;
                        };
                        next_track.play();
                        let outcome = next_track.render(
                            context,
                            &mut read_bufs,
                            &mut bus_bufs,
                            offset..frames,
                            &mut sink,
                        );
                        if let Some(snapshot) = outcome_position_duration(&outcome) {
                            leading_outcome_pos_dur = Some(snapshot);
                        }
                        match next_handover(&outcome, offset) {
                            Some(next) if next.offset < frames => {
                                offset = next.offset;
                                ending = next_handle;
                            }
                            _ => break,
                        }
                    }
                }
            }
        }

        let [bus_left, bus_right] = &mut bus_bufs;
        self.apply_gain(&mut bus_left[start..], &mut bus_right[start..]);

        (playback_started, leading_outcome_pos_dur)
    }

    delegate::delegate! {
        to self.gain {
            /// Ramp the deck's output gain to `gain` from the next frame rendered.
            #[call(set_value)]
            pub(crate) fn set_gain(&mut self, gain: f32);
            pub(crate) fn update_sample_rate(&mut self, sample_rate: NonZeroU32);
        }
    }

    fn apply_gain(&mut self, left: &mut [f32], right: &mut [f32]) {
        if self.gain.has_settled() {
            let gain = self.gain.target_value();
            for (l, r) in left.iter_mut().zip(right.iter_mut()) {
                *l *= gain;
                *r *= gain;
            }
            return;
        }
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            let gain = self.gain.next_smoothed();
            *l *= gain;
            *r *= gain;
        }
        self.gain.settle();
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
}

/// The deck's tracks as one range sees them, in lists sized to the deck's slots so a range never
/// allocates.
struct RangeTracks {
    /// The playing tracks that are not stopped: their slot and whether they lead.
    active: Vec<ActiveTrackEntry>,
    /// Whether the playing track at each index of `active` already rendered as a handover.
    skip: Vec<bool>,
}

impl RangeTracks {
    fn new(slots: NonZeroUsize) -> Self {
        Self {
            active: Vec::with_capacity(slots.get()),
            skip: Vec::with_capacity(slots.get()),
        }
    }

    fn refill(&mut self, tracks: &TrackSlots) {
        self.active.clear();
        self.active.extend(
            tracks
                .iter()
                .filter(|(_, track)| track.state().is_playing() && !track.is_stopped())
                .map(|(slot, track)| (slot, track.state().is_leading())),
        );
        self.skip.clear();
        self.skip.resize(self.active.len(), false);
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
