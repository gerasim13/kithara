use std::ops::Range;

use kithara_audio::{PresentationAdvance, PresentationCursor, PresentationPoint, SessionFrame};
use kithara_platform::sync::Arc;
use num_traits::cast::AsPrimitive;
use ringbuf::{HeapProd, traits::Producer};

use super::{
    PlayerTrack, ReadOutcome, RtSink,
    triggers::{TrackTriggers, TriggerInput},
};
use crate::bridge::{PlayerNotification, RtMetrics, TrackPlaybackStopReason, TrackState};

struct TrackReadContext<'a> {
    sink: RtSink<'a>,
    range: Range<usize>,
}

#[derive(Clone, Copy)]
struct PartialRead {
    duration: f64,
    frames: usize,
    presentation: Option<PresentationCursor>,
}

/// Result of a single track render attempt.
#[derive(Debug)]
pub enum TrackReadOutcome {
    /// The full requested block was written into the mix buffer.
    Full {
        /// Playback position snapshot after the read (seconds).
        position: f64,
        /// Real PCM frames copied from the underlying resource/scratch buffer.
        frames: usize,
        /// Visible duration snapshot in seconds.
        duration: f64,
        /// Exact remaining buffered frames after EOF has been observed.
        frames_until_eof: Option<usize>,
        /// Producer point mapped from an exactly consumed final boundary.
        presentation: Option<PresentationCursor>,
    },
    /// Only the first `frames` samples were written; EOF was reached in-block.
    Partial {
        /// Number of frames written into the destination block.
        frames: usize,
        /// Visible duration snapshot in seconds.
        duration: f64,
        /// Producer point mapped from an exactly consumed final boundary.
        presentation: Option<PresentationCursor>,
    },
    /// No frames were written because the track is already finished.
    Eof,
    /// The source reported a non-recoverable error mid-stream.
    Failed,
}

impl PlayerTrack {
    /// Advance the media clock after one read of real output frames.
    ///
    /// A proven consumed presentation boundary replaces accumulated scalar
    /// drift with its exact source coordinate. Playback rate advances the
    /// whole read only when no proof was consumed.
    fn advance_media_clock(&mut self, frames: usize, consumed: Option<PresentationPoint>) {
        self.served_media_frames = media_frames_after_read(
            self.served_media_frames,
            frames,
            self.playback_rate,
            self.sample_rate,
            consumed,
        );
    }

    fn check_notifications(
        triggers: &mut TrackTriggers,
        notification_tx: &mut HeapProd<PlayerNotification>,
        input: TriggerInput,
    ) {
        triggers.check(notification_tx, input);
    }

    fn handle_failed_end(&mut self, notification_tx: &mut HeapProd<PlayerNotification>) {
        if self.state == TrackState::Finished {
            return;
        }
        self.set_state(TrackState::Finished);
        notification_tx
            .try_push(PlayerNotification::PlaybackStopped {
                src: Arc::clone(self.src()),
                item_id: self.item_id.clone(),
                reason: TrackPlaybackStopReason::Failed,
            })
            .ok();
        self.state_dirty = false;
    }

    fn handle_full_read(
        &mut self,
        scratch_bufs: &mut [&mut [f32]],
        mix_bufs: &mut [&mut [f32]],
        ctx: TrackReadContext<'_>,
        outcome: TrackReadOutcome,
    ) -> TrackReadOutcome {
        let TrackReadOutcome::Full {
            duration,
            frames,
            frames_until_eof,
            presentation,
            ..
        } = outcome
        else {
            return outcome;
        };

        self.observed_duration = duration;
        self.update_observed_eof(frames_until_eof);
        let position = self.position();
        let duration = self.observed_duration;

        let TrackReadContext { sink, range } = ctx;
        let range_len = range.len();
        self.fade
            .mix_range(scratch_bufs, mix_bufs, range, range_len);
        Self::check_notifications(
            &mut self.triggers,
            sink.notifications,
            TriggerInput {
                duration,
                frames_until_eof,
                position,
                block_frames: range_len,
                fade_duration: self.fade.duration(),
                prefetch_duration: self.prefetch_duration,
                sample_rate: self.sample_rate,
            },
        );
        self.update_after_mix(sink.notifications);

        TrackReadOutcome::Full {
            position,
            duration,
            frames,
            frames_until_eof,
            presentation,
        }
    }

    /// Finalize the track at its natural end — unless the control thread has
    /// already published a seek this track has not been re-based onto yet.
    ///
    /// The publish happens before the matching `PlayerCmd::Seek` is sent, so a
    /// newer `published_seek_epoch` means the user left this position while the
    /// render block was in flight. Ending the track there would hand the queue a
    /// `ItemDidPlayToEnd` for a position nobody is at, and the queue would
    /// auto-advance out from under the seek the processor is about to apply.
    /// Holding costs the caller one block of silence: the seek command that
    /// releases the hold is already on its way.
    fn handle_natural_end(
        &mut self,
        notification_tx: &mut HeapProd<PlayerNotification>,
        published_seek_epoch: u64,
    ) {
        if self.state == TrackState::Finished {
            return;
        }
        if published_seek_epoch != self.seek_epoch {
            return;
        }
        self.triggers.mark_prefetch_requested();
        self.triggers.emit_handover_requested(notification_tx);
        self.set_state(TrackState::Finished);
        self.ended_at_eof = true;
        notification_tx
            .try_push(PlayerNotification::PlaybackStopped {
                src: Arc::clone(self.src()),
                item_id: self.item_id.clone(),
                reason: TrackPlaybackStopReason::Eof,
            })
            .ok();
        self.state_dirty = false;
    }

    fn handle_partial_read(
        &mut self,
        scratch_bufs: &mut [&mut [f32]],
        mix_bufs: &mut [&mut [f32]],
        ctx: TrackReadContext<'_>,
        partial: PartialRead,
    ) -> TrackReadOutcome {
        let TrackReadContext { sink, range } = ctx;
        let published_seek_epoch = sink.seek_epoch;
        let notification_tx = sink.notifications;
        let PartialRead {
            frames,
            duration,
            presentation,
        } = partial;
        let position = self.position();
        self.observed_duration = if position > 0.0 { position } else { duration };
        let duration = self.observed_duration;
        let block_frames = range.len();
        let mix_range = range.start..range.start + frames;

        self.fade
            .mix_range(scratch_bufs, mix_bufs, mix_range, frames);
        Self::check_notifications(
            &mut self.triggers,
            notification_tx,
            TriggerInput {
                block_frames,
                duration,
                position,
                fade_duration: self.fade.duration(),
                frames_until_eof: Some(0),
                prefetch_duration: self.prefetch_duration,
                sample_rate: self.sample_rate,
            },
        );
        self.handle_natural_end(notification_tx, published_seek_epoch);

        TrackReadOutcome::Partial {
            frames,
            duration,
            presentation,
        }
    }

    fn notify_state_change(&mut self, notification_tx: &mut HeapProd<PlayerNotification>) {
        if !self.state_dirty {
            return;
        }
        let notification = match self.state {
            TrackState::Preloading => PlayerNotification::Loaded {
                src: Arc::clone(self.src()),
            },
            TrackState::FadingIn => PlayerNotification::FadingIn {
                src: Arc::clone(self.src()),
            },
            TrackState::FadingOut => PlayerNotification::FadingOut {
                src: Arc::clone(self.src()),
            },
            TrackState::Playing => PlayerNotification::PlaybackStarted {
                src: Arc::clone(self.src()),
                item_id: self.item_id.clone(),
            },
            TrackState::Finished => PlayerNotification::PlaybackStopped {
                src: Arc::clone(self.src()),
                item_id: self.item_id.clone(),
                reason: TrackPlaybackStopReason::Stop,
            },
        };

        if notification_tx.try_push(notification).is_ok() {
            self.state_dirty = false;
        }
    }

    /// Read audio from this track into scratch/mix buffers.
    pub fn read(
        &mut self,
        scratch_bufs: &mut [&mut [f32]],
        mix_bufs: &mut [&mut [f32]],
        range: Range<usize>,
        block_frame: Option<SessionFrame>,
        sink: &mut RtSink<'_>,
    ) -> TrackReadOutcome {
        if self.state == TrackState::Finished {
            return TrackReadOutcome::Eof;
        }

        let read_outcome =
            self.read_resource(scratch_bufs, range.clone(), block_frame, sink.metrics);
        match read_outcome {
            TrackReadOutcome::Full { .. } => self.handle_full_read(
                scratch_bufs,
                mix_bufs,
                TrackReadContext {
                    sink: sink.reborrow(),
                    range,
                },
                read_outcome,
            ),
            TrackReadOutcome::Partial {
                frames,
                duration,
                presentation,
            } => self.handle_partial_read(
                scratch_bufs,
                mix_bufs,
                TrackReadContext {
                    sink: sink.reborrow(),
                    range,
                },
                PartialRead {
                    duration,
                    frames,
                    presentation,
                },
            ),
            TrackReadOutcome::Eof => {
                self.handle_natural_end(sink.notifications, sink.seek_epoch);
                TrackReadOutcome::Eof
            }
            TrackReadOutcome::Failed => {
                self.handle_failed_end(sink.notifications);
                TrackReadOutcome::Failed
            }
        }
    }

    fn read_resource(
        &mut self,
        scratch_bufs: &mut [&mut [f32]],
        range: Range<usize>,
        block_frame: Option<SessionFrame>,
        metrics: &RtMetrics,
    ) -> TrackReadOutcome {
        let (scratch_left, scratch_right) = scratch_bufs.split_at_mut(1);
        let mut scratch_window = [
            &mut scratch_left[0][range.clone()],
            &mut scratch_right[0][range.clone()],
        ];

        let (read_outcome, point, duration, frames_until_eof, presentation_advance) = {
            let resource = &mut self.resource;
            let read_outcome = resource.read(&mut scratch_window, 0..range.len(), metrics);
            let presentation_advance = resource.take_presentation_advance();
            (
                read_outcome,
                resource.presentation_point(),
                resource.duration(),
                resource.frames_until_eof(),
                presentation_advance,
            )
        };

        match read_outcome {
            ReadOutcome::Full { frames } => {
                let presentation = if frames == range.len() && frames > 0 {
                    self.update_presentation(
                        point,
                        presentation_advance,
                        block_frame,
                        &range,
                        frames,
                    )
                } else {
                    self.invalidate_presentation();
                    None
                };
                let consumed = consumed_point(presentation_advance, frames);
                self.advance_media_clock(frames, consumed);
                TrackReadOutcome::Full {
                    frames,
                    duration,
                    frames_until_eof,
                    position: 0.0,
                    presentation,
                }
            }
            ReadOutcome::Partial { frames } => {
                let presentation = if frames > 0 {
                    self.update_presentation(
                        point,
                        presentation_advance,
                        block_frame,
                        &range,
                        frames,
                    )
                } else {
                    self.invalidate_presentation();
                    None
                };
                let consumed = consumed_point(presentation_advance, frames);
                self.advance_media_clock(frames, consumed);
                TrackReadOutcome::Partial {
                    frames,
                    duration,
                    presentation,
                }
            }
            ReadOutcome::Eof => {
                self.invalidate_presentation();
                TrackReadOutcome::Eof
            }
            ReadOutcome::Failed => {
                self.invalidate_presentation();
                TrackReadOutcome::Failed
            }
        }
    }

    fn update_after_mix(&mut self, notification_tx: &mut HeapProd<PlayerNotification>) {
        if self.fade.has_settled() {
            self.update_state_after_fade();
        }

        if self.state_dirty {
            self.notify_state_change(notification_tx);
        }
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

    fn update_state_after_fade(&mut self) {
        let new_state = match self.state {
            TrackState::FadingIn => TrackState::Playing,
            TrackState::FadingOut => TrackState::Finished,
            current => current,
        };
        self.set_state(new_state);
    }
}

fn consumed_point(
    advance: Option<PresentationAdvance>,
    frames: usize,
) -> Option<PresentationPoint> {
    let advance = advance?;
    (advance.read_offset_frames() <= frames).then_some(advance.point())
}

fn media_frames_after_read(
    current: f64,
    frames: usize,
    playback_rate: f32,
    host_rate: u32,
    consumed: Option<PresentationPoint>,
) -> f64 {
    let Some(consumed) = consumed else {
        let output_frames: f64 = AsPrimitive::as_(frames);
        return output_frames.mul_add(f64::from(playback_rate), current);
    };
    let source_frames: f64 = AsPrimitive::as_(consumed.source_frame());
    let source_rate = f64::from(consumed.sample_rate().get());
    let host_rate = f64::from(host_rate.max(1));
    source_frames / source_rate * host_rate
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU32;

    use kithara_audio::{PresentationAdvance, PresentationPoint};
    use kithara_test_utils::kithara;

    use super::{consumed_point, media_frames_after_read};

    #[kithara::test]
    fn consumed_presentation_replaces_scalar_media_clock_advance() {
        let source_rate = NonZeroU32::new(48_000).expect("test source rate is non-zero");
        let point = PresentationPoint::new(0, 48_000, 0, 4_096, source_rate);
        let advance = PresentationAdvance::new(point, 256);
        let consumed = consumed_point(Some(advance), 512)
            .expect("consumed boundary inside the read is valid proof");

        let corrected = media_frames_after_read(44_000.0, 512, 1.5, 44_100, Some(consumed));
        let scalar_only = media_frames_after_read(44_000.0, 512, 1.5, 44_100, None);

        assert_eq!(corrected, 44_100.0);
        assert_eq!(scalar_only, 44_768.0);
        assert_ne!(
            corrected, scalar_only,
            "source proof must replace scalar drift"
        );
    }
}
