use std::ops::Range;

use kithara_test_macros as kithara;
use kithara_warp::{PresentationFrontier, RenderContext, WarpMapRevision};
use num_traits::cast::AsPrimitive;

use super::{PlayerTrack, ReadOutcome, RtSink};
use crate::bridge::{DeckEvent, PlaybackFault, RtMetrics, SlotState};

/// Result of a single track render attempt.
#[derive(Debug)]
pub enum TrackReadOutcome {
    /// The full requested block was written into the mix buffer.
    Full {
        /// Playback position snapshot after the read (seconds).
        position: f64,
        /// Real audio frames copied from the underlying resource/scratch buffer.
        frames: usize,
        /// Visible duration snapshot in seconds.
        duration: f64,
        /// Exact remaining buffered frames after EOF has been observed.
        frames_until_eof: Option<usize>,
    },
    /// Only the first `frames` samples were written; EOF was reached in-block.
    Partial {
        /// Number of frames written into the destination block.
        frames: usize,
        /// Visible duration snapshot in seconds.
        duration: f64,
    },
    /// No frames were written because the track is already finished.
    Eof,
    /// The source reported a non-recoverable error mid-stream, or the render
    /// context could not serve this track. The payload names which.
    Failed(PlaybackFault),
}

impl TrackReadOutcome {
    /// Block offset of the frame after the track's last, when the read ended the track; a read
    /// over `range` that ended it wrote nothing past that frame.
    #[must_use]
    pub const fn ended_at(&self, range: &Range<usize>) -> Option<usize> {
        match self {
            Self::Full { .. } => None,
            Self::Partial { frames, .. } => Some(range.start.saturating_add(*frames)),
            Self::Eof | Self::Failed(_) => Some(range.start),
        }
    }
}

impl PlayerTrack {
    /// Advance the media clock by `frames` of mixed output.
    ///
    /// The mix output runs on the output clock; one output frame carries the
    /// resource's current effective rate in media frames.
    fn advance_media_clock(&mut self, frames: usize) {
        let output_frames: f64 = AsPrimitive::as_(frames);
        let playback_rate = self.playback_rate();
        self.served_media_frames =
            output_frames.mul_add(f64::from(playback_rate), self.served_media_frames);
    }

    /// The track ended on block offset `offset`: it stops sounding there and reports it.
    fn end(&mut self, offset: usize, sink: &mut RtSink<'_>) {
        if self.state == SlotState::Ended {
            return;
        }
        self.set_state(SlotState::Ended);
        self.shut();
        let event = DeckEvent::Ended {
            slot: sink.slot,
            at: sink.at(offset),
        };
        sink.report(event);
    }

    /// Scale the read frames by the gate and envelope and add them into the mix, then settle
    /// the transport: a gate that shut stops the track, an envelope that reached silence stops
    /// it and reports so.
    fn mix(
        &mut self,
        scratch_bufs: &mut [&mut [f32]],
        mix_bufs: &mut [&mut [f32]],
        range: Range<usize>,
        sink: &mut RtSink<'_>,
    ) {
        let fading_out = self.fade.is_fading_out();
        let remaining = usize::try_from(self.fade.remaining()).unwrap_or(usize::MAX);
        let start = range.start;
        let frames = range.len();
        self.gate.apply(scratch_bufs, range.clone());
        self.fade.mix_range(scratch_bufs, mix_bufs, range, frames);
        if fading_out && self.fade.settled() {
            self.set_state(SlotState::Stopped);
            self.shut();
            let event = DeckEvent::Faded {
                slot: sink.slot,
                at: sink.at(start.saturating_add(remaining)),
            };
            sink.report(event);
        } else if self.gate.is_shut() {
            self.set_state(SlotState::Stopped);
        }
    }

    fn handle_full_read(
        &mut self,
        scratch_bufs: &mut [&mut [f32]],
        mix_bufs: &mut [&mut [f32]],
        range: Range<usize>,
        sink: &mut RtSink<'_>,
        outcome: TrackReadOutcome,
    ) -> TrackReadOutcome {
        let TrackReadOutcome::Full {
            duration,
            frames,
            frames_until_eof,
            ..
        } = outcome
        else {
            return outcome;
        };

        self.advance_media_clock(frames);
        self.observed_duration = duration;
        self.update_observed_eof(frames_until_eof);
        let position = self.position();
        let duration = self.observed_duration;
        let missing = range.len().saturating_sub(frames);
        if missing > 0 {
            let event = DeckEvent::Underrun {
                slot: sink.slot,
                at: sink.at(range.start.saturating_add(frames)),
                frames: u32::try_from(missing).unwrap_or(u32::MAX),
            };
            sink.report(event);
        }
        self.mix(scratch_bufs, mix_bufs, range, sink);

        TrackReadOutcome::Full {
            position,
            duration,
            frames,
            frames_until_eof,
        }
    }

    fn handle_partial_read(
        &mut self,
        scratch_bufs: &mut [&mut [f32]],
        mix_bufs: &mut [&mut [f32]],
        range: Range<usize>,
        sink: &mut RtSink<'_>,
        (frames, duration): (usize, f64),
    ) -> TrackReadOutcome {
        self.advance_media_clock(frames);
        let position = self.position();
        self.observed_duration = if position > 0.0 { position } else { duration };
        let duration = self.observed_duration;
        let end = range.start + frames;
        self.mix(scratch_bufs, mix_bufs, range.start..end, sink);
        self.end(end, sink);

        TrackReadOutcome::Partial { frames, duration }
    }

    /// Read audio from this track into scratch/mix buffers.
    pub fn read(
        &mut self,
        scratch_bufs: &mut [&mut [f32]],
        mix_bufs: &mut [&mut [f32]],
        range: Range<usize>,
        sink: &mut RtSink<'_>,
    ) -> TrackReadOutcome {
        self.read_with_context(None, scratch_bufs, mix_bufs, range, sink)
    }

    fn read_resource(
        &mut self,
        context: Option<&RenderContext>,
        scratch_bufs: &mut [&mut [f32]],
        range: Range<usize>,
        metrics: &RtMetrics,
    ) -> TrackReadOutcome {
        let resource = &mut self.resource;
        let (scratch_left, scratch_right) = scratch_bufs.split_at_mut(1);
        let mut scratch_window = [
            &mut scratch_left[0][range.clone()],
            &mut scratch_right[0][range.clone()],
        ];

        match resource.read_with_context(context, &mut scratch_window, 0..range.len(), metrics) {
            ReadOutcome::Full { frames } => TrackReadOutcome::Full {
                frames,
                duration: resource.duration(),
                frames_until_eof: resource.frames_until_eof(),
                position: 0.0,
            },
            ReadOutcome::Partial { frames } => TrackReadOutcome::Partial {
                frames,
                duration: resource.duration(),
            },
            ReadOutcome::Eof => TrackReadOutcome::Eof,
            ReadOutcome::Failed(kind) => TrackReadOutcome::Failed(PlaybackFault::Decode(kind)),
        }
    }

    fn read_with_context(
        &mut self,
        context: Option<&RenderContext>,
        scratch_bufs: &mut [&mut [f32]],
        mix_bufs: &mut [&mut [f32]],
        range: Range<usize>,
        sink: &mut RtSink<'_>,
    ) -> TrackReadOutcome {
        if self.state != SlotState::Playing {
            return TrackReadOutcome::Eof;
        }

        let read_outcome = self.read_resource(context, scratch_bufs, range.clone(), sink.metrics);
        match read_outcome {
            TrackReadOutcome::Full { .. } => {
                self.handle_full_read(scratch_bufs, mix_bufs, range, sink, read_outcome)
            }
            TrackReadOutcome::Partial { frames, duration } => {
                self.handle_partial_read(scratch_bufs, mix_bufs, range, sink, (frames, duration))
            }
            TrackReadOutcome::Eof => {
                self.end(range.start, sink);
                TrackReadOutcome::Eof
            }
            TrackReadOutcome::Failed(fault) => {
                sink.metrics.record_decode_error();
                self.end(range.start, sink);
                TrackReadOutcome::Failed(fault)
            }
        }
    }

    /// One track's contribution to one output block.
    ///
    /// `range` is the block-relative span this track covers: a track a chain started inside the
    /// block renders from the frame it started on, so the span carries the in-block seam.
    /// Together with the session-axis base in `context` it names the exact output frames this
    /// track wrote, which is what attributes a frame to a slot.
    #[kithara::probe(
        slot = sink.slot.get(),
        output_base = context.map(|ctx| i64::from(ctx.output().output_frames().start)),
        range_start = range.start,
        range_end = range.end,
        served_media_frames = AsPrimitive::<u64>::as_(self.served_media_frames)
    )]
    pub(crate) fn render(
        &mut self,
        context: Option<&RenderContext>,
        scratch_bufs: &mut [&mut [f32]],
        mix_bufs: &mut [&mut [f32]],
        range: Range<usize>,
        sink: &mut RtSink<'_>,
    ) -> TrackReadOutcome {
        let Some(context) = context else {
            self.resource.clear_render();
            return self.read_with_context(None, scratch_bufs, mix_bufs, range, sink);
        };
        if context.output().sample_rate().get() != self.sample_rate {
            self.resource.clear_render();
            self.end(range.start, sink);
            return TrackReadOutcome::Failed(PlaybackFault::OutputRateMismatch);
        }
        let Some(context) = context.for_output_range(range.clone()) else {
            self.resource.clear_render();
            self.end(range.start, sink);
            return TrackReadOutcome::Failed(PlaybackFault::OutputRangeUnavailable);
        };
        if let Some(source) = self
            .resource
            .presentation_source_end(context.output().sample_rate())
        {
            self.resource.publish_render(
                &context,
                presentation_frontier(&context, source.frame())
                    .with_warp_map(source.mapping_revision().map(WarpMapRevision::from)),
            );
        } else {
            self.resource.clear_render();
        }
        self.read_with_context(Some(&context), scratch_bufs, mix_bufs, range, sink)
    }

    /// Read the consumer's next `left.len()` frames, ramped from the envelope's last gain down to
    /// silence, into a replaced slot's tail; returns how many frames it wrote.
    pub(crate) fn read_tail(
        &mut self,
        left: &mut [f32],
        right: &mut [f32],
        metrics: &RtMetrics,
    ) -> usize {
        let len = left.len().min(right.len());
        if self.state != SlotState::Playing || len == 0 {
            return 0;
        }
        let from = self.fade.gain();
        let mut window = [&mut left[..len], &mut right[..len]];
        let written = match self.resource.read(&mut window, 0..len, metrics) {
            ReadOutcome::Full { .. } => len,
            ReadOutcome::Partial { frames } => frames,
            ReadOutcome::Eof | ReadOutcome::Failed(_) => 0,
        };
        let span: f32 = AsPrimitive::as_(len);
        for (index, (l, r)) in left[..written].iter_mut().zip(&mut right[..written]).enumerate() {
            let step: f32 = AsPrimitive::as_(index);
            let gain = from * (1.0 - step / span);
            *l *= gain;
            *r *= gain;
        }
        written
    }

    fn update_observed_eof(&mut self, frames_until_eof: Option<usize>) {
        if let Some(remaining_frames) = frames_until_eof {
            let sample_rate = self.sample_rate.max(1);
            let remaining_f64: f64 = AsPrimitive::as_(remaining_frames);
            let observed_eof = self.position() + remaining_f64 / f64::from(sample_rate);
            if self.observed_duration <= 0.0 || observed_eof < self.observed_duration {
                self.observed_duration = observed_eof;
            }
        }
    }
}

fn presentation_frontier(context: &RenderContext, source: u64) -> PresentationFrontier {
    PresentationFrontier::builder()
        .source(source)
        .output(context.output().output_frames().start)
        .build()
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU32;

    use kithara_signal::{OutputContext, SessionEpoch, SessionFrame};
    use kithara_test_utils::kithara;
    use kithara_warp::RenderContext;

    use super::presentation_frontier;

    #[kithara::test]
    fn publication_uses_the_derived_subrange_start() {
        let output = OutputContext::new(
            SessionFrame::new(1_000)..SessionFrame::new(1_200),
            NonZeroU32::new(48_000).expect("fixture sample rate is non-zero"),
            SessionEpoch::new(1),
            None,
        )
        .expect("fixture output range is ordered");
        let context = RenderContext::new(output, None)
            .expect("fixture context is valid")
            .for_output_range(40..80)
            .expect("fixture subrange is valid");

        let frontier = presentation_frontier(&context, 8_000);

        assert_eq!(frontier.source(), 8_000);
        assert_eq!(frontier.output(), SessionFrame::new(1_040));
    }
}
