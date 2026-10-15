use kithara_decode::{DecodeError, DecodeResult, DecoderSeekOutcome};
use kithara_platform::time::Duration;
use kithara_stream::{PendingReason, SourcePhase, StreamType};

use super::core::{OwnerPhase, StreamAudioSource};
use crate::{
    AudioEvent, SeekOutcome, TrackFailureKind,
    pipeline::{
        decode::format::{FormatDecision, detect},
        rebuild::{RecreateCause, RecreateState},
        seek::{ResumeTarget, emit::commit_outcome},
    },
};

impl<T: StreamType> StreamAudioSource<T> {
    pub(super) fn release_seek_abr(&mut self) {
        if let Some(abr) = self.seek_abr.take() {
            abr.unlock();
        }
    }

    pub(super) fn resume_target(&self) -> ResumeTarget {
        if let OwnerPhase::Seeking { target, .. } = self.phase {
            return target;
        }
        if let Some(target) = self.decode.active().resume_target() {
            return target;
        }
        self.resume.source_end().map_or_else(
            || ResumeTarget::Position(self.playhead.position()),
            ResumeTarget::Source,
        )
    }

    pub(super) fn prepare_active(&mut self) -> Result<(), (TrackFailureKind, DecodeError)> {
        let seeking = match self.phase {
            OwnerPhase::Seeking { target, .. } => Some(target),
            _ => None,
        };
        let (outcome, mut replaced) = match self.phase {
            OwnerPhase::Seeking {
                target,
                reason: PendingReason::SessionRetired,
            } => (Some(self.replace_retired_reader(target)?), true),
            _ => (
                self.decode.prepare_deferred(true).map_err(|error| {
                    let failure = if self.shared_stream.phase() == SourcePhase::Cancelled {
                        TrackFailureKind::SourceCancelled
                    } else {
                        TrackFailureKind::Decode {
                            kind: crate::map_decode_error_kind(&error),
                        }
                    };
                    (failure, error)
                })?,
                false,
            ),
        };
        if self.shared_stream.phase() == SourcePhase::Cancelled {
            return Err((TrackFailureKind::SourceCancelled, DecodeError::Interrupted));
        }
        let Some(mut outcome) = outcome else {
            return if seeking.is_some() {
                Err((
                    TrackFailureKind::Decode {
                        kind: crate::DecodeErrorKind::InvalidData,
                    },
                    DecodeError::InvalidData {
                        detail: "pending seek has no decoder continuation",
                    },
                ))
            } else {
                Ok(())
            };
        };
        let target = self.resume_target();
        match outcome {
            DecoderSeekOutcome::Pending(PendingReason::SessionRetired) if !replaced => {
                outcome = self.replace_retired_reader(target)?;
                replaced = true;
            }
            DecoderSeekOutcome::Pending(PendingReason::VariantChange) if !replaced => {
                if let FormatDecision::Recreate(recreate) =
                    detect(&self.shared_stream, self.decode.active())
                {
                    let offset = recreate.offset;
                    outcome = self
                        .install_replacement(recreate, Some(target))
                        .and_then(|outcome| {
                            outcome.ok_or(DecodeError::InvalidData {
                                detail: "replacement seek has no outcome",
                            })
                        })
                        .map_err(|error| {
                            let failure = if self.shared_stream.phase() == SourcePhase::Cancelled {
                                TrackFailureKind::SourceCancelled
                            } else {
                                TrackFailureKind::RecreateFailed { offset }
                            };
                            (failure, error)
                        })?;
                    replaced = true;
                }
            }
            _ => {}
        }
        if replaced && matches!(outcome, DecoderSeekOutcome::Landed { .. }) {
            let continuation = self.decode.prepare_deferred(true).map_err(|error| {
                let failure = if self.shared_stream.phase() == SourcePhase::Cancelled {
                    TrackFailureKind::SourceCancelled
                } else {
                    TrackFailureKind::Decode {
                        kind: crate::map_decode_error_kind(&error),
                    }
                };
                (failure, error)
            })?;
            if continuation.is_some() {
                return Err((
                    TrackFailureKind::Decode {
                        kind: crate::DecodeErrorKind::InvalidData,
                    },
                    DecodeError::InvalidData {
                        detail: "landed replacement still has an unresolved seek",
                    },
                ));
            }
        }
        if let Some(target) = seeking {
            self.accept_seek_outcome(target, outcome).map_err(|error| {
                (
                    TrackFailureKind::Decode {
                        kind: crate::map_decode_error_kind(&error),
                    },
                    error,
                )
            })?;
        }
        Ok(())
    }

    pub(super) fn replace_retired_reader(
        &mut self,
        target: ResumeTarget,
    ) -> Result<DecoderSeekOutcome, (TrackFailureKind, DecodeError)> {
        let mut offset = self.decode.active().base_offset();
        let result = (|| {
            if self.shared_stream.phase() == SourcePhase::Cancelled {
                return Err(DecodeError::Interrupted);
            }
            self.shared_stream
                .seek_time_anchor(DecodeResult::from(target)?)
                .map_err(|source| DecodeError::Io { source })?;
            if self.shared_stream.phase() == SourcePhase::Cancelled {
                return Err(DecodeError::Interrupted);
            }
            let media_info = self
                .shared_stream
                .media_info()
                .ok_or(DecodeError::InvalidData {
                    detail: "retired reader has no current media information",
                })?;
            let range = self
                .shared_stream
                .format_change_segment_range()
                .map_err(DecodeError::backend)?;
            offset = range.start;
            self.install_replacement(
                RecreateState {
                    media_info: Some(media_info),
                    offset,
                    cause: RecreateCause::VariantSwitch,
                },
                Some(target),
            )?
            .ok_or(DecodeError::InvalidData {
                detail: "replacement seek has no outcome",
            })
        })();
        result.map_err(|error| {
            let failure = if self.shared_stream.phase() == SourcePhase::Cancelled {
                TrackFailureKind::SourceCancelled
            } else {
                TrackFailureKind::RecreateFailed { offset }
            };
            (failure, error)
        })
    }

    pub(super) fn accept_seek_outcome(
        &mut self,
        target: ResumeTarget,
        outcome: DecoderSeekOutcome,
    ) -> DecodeResult<SeekOutcome> {
        if self.shared_stream.phase() == SourcePhase::Cancelled {
            self.fail(TrackFailureKind::SourceCancelled, None);
            return Err(DecodeError::Interrupted);
        }
        let position = DecodeResult::from(target)?;
        let accepted = match outcome {
            DecoderSeekOutcome::Pending(reason) => {
                self.phase = OwnerPhase::Seeking { target, reason };
                self.waiting_for_source = reason != PendingReason::Retry;
                return Ok(SeekOutcome::Pending {
                    target: position,
                    reason,
                });
            }
            DecoderSeekOutcome::Landed { landed_at, .. } => {
                commit_outcome(
                    self.decode.active(),
                    &self.shared_stream,
                    self.playhead.as_ref(),
                    &outcome,
                )?;
                self.phase = OwnerPhase::Decoding;
                SeekOutcome::Landed {
                    target: position,
                    landed_at,
                }
            }
            DecoderSeekOutcome::PastEof { duration } => {
                commit_outcome(
                    self.decode.active(),
                    &self.shared_stream,
                    self.playhead.as_ref(),
                    &outcome,
                )?;
                self.phase = OwnerPhase::AtEof;
                self.release_seek_abr();
                SeekOutcome::PastEof {
                    target: position,
                    duration,
                }
            }
        };
        self.waiting_for_source = false;
        self.emit.enqueue(AudioEvent::SeekLifecycle {
            stage: crate::SeekLifecycleStage::SeekApplied,
            location: crate::SegmentLocation::default(),
        });
        Ok(accepted)
    }

    pub(super) fn seek_selected(
        &mut self,
        position: Duration,
        for_render: bool,
    ) -> Result<SeekOutcome, crate::AudioReadError> {
        if self.shared_stream.phase() == SourcePhase::Cancelled {
            self.fail(TrackFailureKind::SourceCancelled, None);
        }
        if let OwnerPhase::Failed { failure, .. } = self.phase {
            return Err(crate::AudioReadError::Stream {
                what: "seek decoded source",
                source: crate::FailureSource::ProducerAfterSeek { failure },
            });
        }
        self.emit.enqueue(AudioEvent::SeekLifecycle {
            stage: crate::SeekLifecycleStage::SeekRequest,
            location: crate::SegmentLocation::default(),
        });
        let result = self.seek_owned(position, for_render);
        if let Err(error) = &result {
            let failure = if self.shared_stream.phase() == SourcePhase::Cancelled {
                TrackFailureKind::SourceCancelled
            } else {
                TrackFailureKind::from(error)
            };
            self.fail(failure, None);
            self.emit
                .enqueue(AudioEvent::SeekRejected { target: position });
        }
        result
    }

    pub(super) fn seek_owned(
        &mut self,
        position: Duration,
        for_render: bool,
    ) -> Result<SeekOutcome, crate::AudioReadError> {
        if !position.is_zero() {
            self.decode.seal_visible_origin();
        }
        if self.seek_abr.is_none()
            && let Some(abr) = self.shared_stream.abr_handle()
        {
            abr.lock();
            self.seek_abr = Some(abr);
        }
        self.phase = OwnerPhase::Decoding;
        self.discard_local_incoming();
        self.resume.clear();
        self.waiting_for_source = false;
        let target = if for_render {
            ResumeTarget::Render(position)
        } else {
            ResumeTarget::Position(position)
        };
        if let Some(duration) = self
            .decode
            .active()
            .visible_origin()
            .and_then(|origin| origin.checked_add(self.playhead.duration()?))
            .filter(|endpoint| position >= *endpoint)
        {
            drop(self.decode.notify_seek());
            return Ok(self.accept_seek_outcome(target, DecoderSeekOutcome::PastEof { duration })?);
        }
        self.shared_stream
            .seek_time_anchor(position)
            .map_err(|source| DecodeError::Io { source })?;
        if let FormatDecision::Recreate(recreate) =
            detect(&self.shared_stream, self.decode.active())
        {
            let offset = recreate.offset;
            self.install_replacement(recreate, None).map_err(|error| {
                self.fail(TrackFailureKind::RecreateFailed { offset }, Some(error));
                crate::AudioReadError::Stream {
                    what: "seek decoder recreation",
                    source: crate::FailureSource::Producer {
                        failure: TrackFailureKind::RecreateFailed { offset },
                    },
                }
            })?;
        }
        if let Some(len) = self.shared_stream.len() {
            self.decode
                .update_len(len.saturating_sub(self.decode.active().base_offset()));
        }
        let outcome = if for_render {
            self.decode.seek_for_render(position)?
        } else {
            self.decode.seek(position)?
        };
        Ok(self.accept_seek_outcome(target, outcome)?)
    }
}
