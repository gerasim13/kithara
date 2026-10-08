use crate::{
    AudioEvent, AudioLaneEvent, AudioSource, DecoderChangeCause, SeekOutcome, TrackFailureKind,
    TrackStep, WaitingReason,
    pipeline::{
        decode::{
            DecoderGeneration,
            core::{ActiveDecode, DecodeAction, DecodeCtx, DecoderFactory, panic_message},
            event::{GenerationInstalled, enqueue_generation_installed},
            format::{FormatDecision, detect},
            resume::ResumeCursor,
            step::tick,
            transition::{IncomingPrime, OutgoingFrontier},
        },
        rebuild::{RecreateCause, RecreateState},
        seek::{ResumeTarget, emit::commit_outcome},
        stream::shared::SharedStream,
    },
};
use kithara_decode::{DecodeError, DecoderSeekOutcome};
use kithara_events::DeferredBus;
use kithara_platform::{sync::Arc, time::Duration};
use kithara_signal::AudioChunk;
use kithara_stream::{
    MediaInfo, OpenedReader, OpenedVariantReader, OutgoingDisposition, PlayheadWrite, StreamType,
    VariantControl, VariantPromotion, VariantReaderTake, VariantTransition, WorkerWake,
};
use std::{
    io::SeekFrom,
    num::NonZeroU32,
    panic::{AssertUnwindSafe, catch_unwind},
};
use tracing::{debug, trace, warn};

enum OwnerPhase {
    Decoding,
    AtEof,
    Failed {
        failure: TrackFailureKind,
        error: Option<DecodeError>,
    },
}

#[cfg(test)]
mod tests;

pub(crate) struct StreamAudioSource<T: StreamType> {
    decode: ActiveDecode,
    factory: DecoderFactory,
    host_rate: Option<NonZeroU32>,
    decoder_backend: kithara_decode::DecoderBackend,
    playback_resampler_backend: &'static str,
    playhead: Arc<dyn PlayheadWrite>,
    emit: Arc<DeferredBus<AudioLaneEvent>>,
    variant_control: Option<Arc<dyn VariantControl>>,
    phase: OwnerPhase,
    failure_logged: bool,
    resume: ResumeCursor,
    shared_stream: SharedStream<T>,
    wake: Arc<dyn WorkerWake>,
}

fn promotion_frontier_for(
    transition: VariantTransition,
    frontier: OutgoingFrontier,
) -> OutgoingFrontier {
    if transition.outgoing_disposition() == OutgoingDisposition::Abandoned {
        OutgoingFrontier::Unavailable
    } else {
        frontier
    }
}
fn initial_promotion_frontier(transition: VariantTransition) -> OutgoingFrontier {
    if transition.outgoing_disposition() == OutgoingDisposition::Abandoned {
        OutgoingFrontier::Unavailable
    } else {
        OutgoingFrontier::Awaiting
    }
}

impl<T: StreamType> StreamAudioSource<T> {
    pub(crate) fn new(
        shared_stream: SharedStream<T>,
        decode: ActiveDecode,
        factory: DecoderFactory,
        host_rate: Option<NonZeroU32>,
        decoder_backend: kithara_decode::DecoderBackend,
        playback_resampler_backend: &'static str,
        emit: Arc<DeferredBus<AudioLaneEvent>>,
        wake: Arc<dyn WorkerWake>,
    ) -> Self {
        let playhead = shared_stream.playhead_write();
        let variant_control = shared_stream.variant_control();
        Self {
            shared_stream,
            wake,
            decode,
            factory,
            host_rate,
            decoder_backend,
            playback_resampler_backend,
            playhead,
            emit,
            variant_control,
            phase: OwnerPhase::Decoding,
            failure_logged: false,
            resume: ResumeCursor::default(),
        }
    }

    fn fail(&mut self, failure: TrackFailureKind, error: Option<DecodeError>) -> TrackFailureKind {
        if let OwnerPhase::Failed { failure, .. } = self.phase {
            return failure;
        }
        self.phase = OwnerPhase::Failed { failure, error };
        self.emit.enqueue(AudioEvent::TrackFailed { failure });
        failure
    }

    fn finish_failure_diagnostic(&mut self) {
        if self.failure_logged {
            return;
        }
        let OwnerPhase::Failed { failure, error } = &mut self.phase else {
            return;
        };
        let error = error.take();
        match failure {
            TrackFailureKind::Decode { .. } => warn!(err = ?error, "track failed: decode error"),
            TrackFailureKind::RecreateFailed { offset } => {
                warn!(offset, "track failed: decoder recreation failed");
            }
            TrackFailureKind::SourceCancelled => warn!("track failed: source cancelled"),
            TrackFailureKind::ChannelClosed => warn!("track failed: channel closed"),
            TrackFailureKind::Render => warn!("track failed: render error"),
        }
        self.failure_logged = true;
    }

    fn discard_local_incoming(&mut self) {
        drop(self.decode.discard_incoming());
    }

    fn abort_local_incoming(
        &mut self,
        control: &dyn VariantControl,
        transition: VariantTransition,
    ) {
        self.discard_local_incoming();
        let _ = control.abort_variant(transition);
    }

    fn abandon_incoming(&mut self) {
        if let Some(transition) = self.decode.incoming_transition()
            && let Some(control) = self.variant_control.clone()
        {
            self.abort_local_incoming(control.as_ref(), transition);
        } else {
            self.discard_local_incoming();
        }
    }

    fn start_incoming_build(
        &mut self,
        control: &dyn VariantControl,
        transition: VariantTransition,
        reader: OpenedVariantReader,
    ) {
        let (plan, reader) = reader.split();
        let landing = Some(ResumeTarget::Position(plan.landing_time()));
        let info = Some(plan.media_info().clone());
        match self.build_generation(reader, info, 0, landing) {
            Ok(generation) => {
                drop(self.decode.install_incoming(transition, generation));
                self.wake.wake();
            }
            Err(error) => {
                warn!(?error, ?transition, "incoming decoder build failed");
                self.abort_local_incoming(control, transition);
            }
        }
    }

    fn build_generation(
        &self,
        reader: OpenedReader,
        info: Option<MediaInfo>,
        offset: u64,
        landing: Option<ResumeTarget>,
    ) -> Result<DecoderGeneration, DecodeError> {
        let gate = reader.construction_gate();
        if let Some(gate) = &gate {
            gate.arm();
        }
        let result = catch_unwind(AssertUnwindSafe(|| {
            let decoder = self.factory.create(reader, info.clone(), self.host_rate)?;
            let mut generation = DecoderGeneration::new(
                decoder,
                info,
                offset,
                gate.clone(),
                None,
                self.decode.gapless_mode(),
            );
            if let Some(target) = landing {
                generation.notify_seek();
                match generation.seek(target.position()?)? {
                    DecoderSeekOutcome::Landed { .. } => generation.trim_to(target),
                    DecoderSeekOutcome::PastEof { .. } => generation.finish(),
                }
            }
            Ok(generation)
        }));
        if let Some(gate) = &gate {
            gate.disarm();
        }
        result.map_err(|payload| {
            warn!(panic = %panic_message(payload), "decoder factory panicked");
            DecodeError::InvalidData {
                detail: "decoder factory panicked",
            }
        })?
    }

    fn install_replacement(
        &mut self,
        recreate: RecreateState,
        landing: Option<crate::SourceEnd>,
    ) -> Result<(), DecodeError> {
        self.abandon_incoming();
        self.shared_stream
            .probe_seek(SeekFrom::Start(recreate.offset))
            .map_err(|source| DecodeError::Io { source })?;
        let reader = self.shared_stream.open_rebuild_reader(recreate.offset);
        let generation = self.build_generation(
            reader,
            recreate.media_info,
            recreate.offset,
            landing.map(ResumeTarget::Source),
        )?;
        let old_spec = self.decode.output_spec();
        self.decode
            .prepare_replacement_profile(generation.blender_profile());
        if let Some(error) = self.decode.take_stage_error() {
            return Err(error);
        }
        drop(self.decode.replace_active(generation));
        self.decode.reset();
        self.resume.clear();
        if let Some(landing) = landing {
            self.resume.rebase(landing);
        }
        if !matches!(self.phase, OwnerPhase::Failed { .. }) {
            self.phase = if self.decode.active().is_finished() {
                OwnerPhase::AtEof
            } else {
                OwnerPhase::Decoding
            };
        }
        let new_spec = self.decode.output_spec();
        if old_spec != new_spec {
            self.emit.enqueue(AudioEvent::FormatChanged {
                old: old_spec,
                new: new_spec,
            });
        }
        self.publish_generation(match recreate.cause {
            RecreateCause::FormatBoundary => DecoderChangeCause::FormatBoundary,
            RecreateCause::HostRateChange => DecoderChangeCause::HostRateChange,
            RecreateCause::VariantSwitch => DecoderChangeCause::VariantSwitch,
        });
        Ok(())
    }

    fn publish_generation(&self, cause: DecoderChangeCause) {
        enqueue_generation_installed(
            &self.emit,
            &GenerationInstalled {
                backend: self.decoder_backend,
                cause,
                generation: self.decode.active(),
                host_sample_rate: self.host_rate.map_or(0, NonZeroU32::get),
                playback_resampler_backend: self.playback_resampler_backend,
                recreates_on_route: true,
            },
        );
    }

    fn seek_owned(&mut self, position: Duration) -> Result<SeekOutcome, crate::AudioReadError> {
        self.discard_local_incoming();
        drop(self.decode.notify_seek());
        self.decode.reset();
        self.resume.clear();
        if let Some(duration) = self
            .playhead
            .duration()
            .filter(|duration| position >= *duration)
        {
            let outcome = DecoderSeekOutcome::PastEof { duration };
            commit_outcome(
                self.decode.active(),
                &self.shared_stream,
                self.playhead.as_ref(),
                &outcome,
            );
            if !matches!(self.phase, OwnerPhase::Failed { .. }) {
                self.phase = OwnerPhase::AtEof;
            }
            return Ok(SeekOutcome::PastEof {
                target: position,
                duration,
            });
        }
        let _anchor = self
            .shared_stream
            .seek_time_anchor(position)
            .map_err(|source| DecodeError::Io { source })?;
        if let FormatDecision::Recreate(recreate) =
            detect(&self.shared_stream, self.decode.active())
        {
            let offset = recreate.offset;
            self.install_replacement(recreate, None).map_err(|_| crate::AudioReadError::Stream {
                what: "seek decoder recreation",
                source: crate::FailureSource::Producer {
                    failure: TrackFailureKind::RecreateFailed { offset },
                },
            })?;
        }
        if let Some(len) = self.shared_stream.len() {
            self.decode
                .update_len(len.saturating_sub(self.decode.active().base_offset()));
        }
        let outcome = self
            .decode
            .seek(&self.shared_stream, self.playhead.as_ref(), position)?;
        match outcome {
            DecoderSeekOutcome::Landed { landed_at, .. } => {
                if !matches!(self.phase, OwnerPhase::Failed { .. }) {
                    self.phase = OwnerPhase::Decoding;
                }
                Ok(SeekOutcome::Landed {
                    target: position,
                    landed_at,
                })
            }
            DecoderSeekOutcome::PastEof { duration } => {
                if !matches!(self.phase, OwnerPhase::Failed { .. }) {
                    self.phase = OwnerPhase::AtEof;
                }
                Ok(SeekOutcome::PastEof {
                    target: position,
                    duration,
                })
            }
        }
    }
    fn prepare_incoming_transition(
        &mut self,
        control: &dyn VariantControl,
        outgoing_frontier: OutgoingFrontier,
    ) -> Option<VariantTransition> {
        let plan = match control.plan_variant_reader(self.decode.landing_for(outgoing_frontier)) {
            Ok(plan) => plan,
            Err(error) => {
                warn!(?error, "failed to plan exact incoming variant reader");
                if let Some(transition) = self.decode.incoming_transition() {
                    self.abort_local_incoming(control, transition);
                }
                return None;
            }
        };
        let Some(plan) = plan else {
            self.discard_local_incoming();
            return None;
        };
        let transition = plan.transition();
        if self.decode.incoming_transition() != Some(transition)
            && let Some(generation) = self
                .decode
                .begin_incoming(transition, initial_promotion_frontier(transition))
        {
            drop(generation);
        }
        if !self.decode.incoming_is_preparing(transition) {
            return None;
        }

        let byte_map = self.shared_stream.byte_map();
        let profile = self
            .factory
            .reader_profile(plan.media_info(), byte_map.as_deref());
        match control.prepare_variant_reader(plan, profile) {
            Ok(Some(prepared)) if prepared == transition => Some(transition),
            Ok(Some(prepared)) => {
                warn!(
                    ?transition,
                    ?prepared,
                    "source prepared a different exact variant transition"
                );
                self.abort_local_incoming(control, transition);
                None
            }
            Ok(None) => {
                self.discard_local_incoming();
                None
            }
            Err(error) => {
                warn!(
                    ?error,
                    ?transition,
                    "failed to prepare exact incoming reader"
                );
                self.abort_local_incoming(control, transition);
                None
            }
        }
    }

    /// A reader starved on the outgoing variant keeps advancing an already-requested transition;
    /// the transition itself owns whether that source remains part of the promotion proof.
    fn progress_variant_transition(&mut self) {
        match &self.phase {
            OwnerPhase::Decoding => {}
            OwnerPhase::AtEof | OwnerPhase::Failed { .. } => {
                if let (Some(control), Some(transition)) = (
                    self.variant_control.clone(),
                    self.decode.incoming_transition(),
                ) {
                    debug!(
                        at_eof = matches!(self.phase, OwnerPhase::AtEof),
                        latched_frontier = ?self.decode.incoming_frontier(),
                        landing = ?self.resume.decode_head(),
                        ?transition,
                        "outgoing ended: aborting variant transition"
                    );
                    self.abort_local_incoming(control.as_ref(), transition);
                }
                return;
            }
        }
        let Some(control) = self.variant_control.clone() else {
            return;
        };

        let landing_frontier = match self.resume.decode_head() {
            Some((frame, rate)) => OutgoingFrontier::Exact { frame, rate },
            None => OutgoingFrontier::Awaiting,
        };
        self.retire_failed_incoming(control.as_ref());
        let observed_frontier = self
            .decode
            .incoming_transition()
            .map_or(landing_frontier, |transition| {
                promotion_frontier_for(transition, landing_frontier)
            });
        let prime = self.decode.prime_incoming(observed_frontier);
        if let Some(incoming) = self.decode.incoming_transition() {
            trace!(
                ?landing_frontier,
                ?observed_frontier,
                latched_frontier = ?self.decode.incoming_frontier(),
                ?prime,
                ?incoming,
                "variant transition pass"
            );
        }
        if prime == IncomingPrime::Advanced {
            self.wake.wake();
        }
        if !self.promote_ready_incoming(control.as_ref()) {
            return;
        }
        let Some(transition) = self.prepare_incoming_transition(control.as_ref(), landing_frontier)
        else {
            return;
        };
        self.take_prepared_incoming(control.as_ref(), transition);
    }

    fn promote_ready_incoming(&mut self, control: &dyn VariantControl) -> bool {
        let Some(prepared) = self.decode.prepare_promotion() else {
            return true;
        };
        let transition = prepared.transition();
        match control.promote_variant(transition) {
            VariantPromotion::Promoted => {
                let outgoing = self.decode.commit_prepared_promotion(prepared);
                {
                    let emit = &self.emit;
                    enqueue_generation_installed(
                        emit,
                        &GenerationInstalled {
                            backend: self.decoder_backend,
                            cause: DecoderChangeCause::VariantSwitch,
                            generation: self.decode.active(),
                            host_sample_rate: self.host_rate.map_or(0, NonZeroU32::get),
                            playback_resampler_backend: self.playback_resampler_backend,
                            recreates_on_route: true,
                        },
                    );
                }
                drop(outgoing);
                self.wake.wake();
                true
            }
            VariantPromotion::Deferred => {
                self.decode.restore_prepared_promotion(prepared);
                false
            }
            VariantPromotion::Stale => {
                drop(DecoderGeneration::from(prepared));
                true
            }
            _ => {
                warn!(
                    ?transition,
                    "source returned an unsupported variant promotion result"
                );
                self.decode.restore_prepared_promotion(prepared);
                false
            }
        }
    }

    fn retire_failed_incoming(&mut self, control: &dyn VariantControl) {
        if let Some((transition, generation)) = self.decode.take_failed_incoming() {
            drop(generation);
            let _ = control.abort_variant(transition);
        }
    }

    fn take_prepared_incoming(
        &mut self,
        control: &dyn VariantControl,
        transition: VariantTransition,
    ) {
        match control.take_prepared_variant_reader(transition) {
            Ok(VariantReaderTake::Preparing) => {}
            Ok(VariantReaderTake::Ready(reader)) => {
                self.start_incoming_build(control, transition, reader);
            }
            Ok(VariantReaderTake::Taken) => {
                warn!(
                    ?transition,
                    "incoming reader was transferred without a matching decoder build"
                );
                self.abort_local_incoming(control, transition);
            }
            Ok(VariantReaderTake::Stale) => self.discard_local_incoming(),
            Err(error) => {
                warn!(?error, ?transition, "failed to take exact incoming reader");
                self.abort_local_incoming(control, transition);
            }
            Ok(_) => {
                warn!(
                    ?transition,
                    "source returned an unsupported incoming reader state"
                );
                self.abort_local_incoming(control, transition);
            }
        }
    }

    /// Hang classification for a transition-pending decode tick: a pending
    /// transition whose incoming byte is serviced by an in-flight fetch is
    /// upstream work (`WaitingDemand`, watchdog-quiet); one with nothing in
    /// flight stays `Waiting` so a wedged switch still surfaces as a hang.
    pub(crate) fn transition_wait_reason(&self) -> WaitingReason {
        let demand_backed = self
            .variant_control
            .as_deref()
            .zip(self.decode.incoming_transition())
            .is_some_and(|(control, transition)| control.transition_demand_in_flight(transition));
        if demand_backed {
            WaitingReason::WaitingDemand
        } else {
            WaitingReason::Waiting
        }
    }
}

impl<T: StreamType> AudioSource for StreamAudioSource<T> {
    type Chunk = AudioChunk;
    fn commit_source_end(&mut self, end: crate::SourceEnd) {
        self.resume.commit_source_end(end);
    }
    fn discontinuity(&self) -> Option<crate::SourceDiscontinuity> {
        Some(self.decode.discontinuity())
    }
    fn seek(&mut self, position: Duration) -> Result<SeekOutcome, crate::AudioReadError> {
        if self.shared_stream.phase() == kithara_stream::SourcePhase::Cancelled {
            self.fail(TrackFailureKind::SourceCancelled, None);
        }
        if let OwnerPhase::Failed { failure, .. } = self.phase {
            self.finish_deferred();
            return Err(crate::AudioReadError::Stream {
                what: "seek decoded source",
                source: crate::FailureSource::ProducerAfterSeek { failure },
            });
        }
        self.emit.enqueue(AudioEvent::SeekLifecycle {
            stage: crate::SeekLifecycleStage::SeekRequest,
            location: crate::SegmentLocation::default(),
        });
        let result = self.seek_owned(position);
        match &result {
            Ok(_) => self.emit.enqueue(AudioEvent::SeekLifecycle {
                stage: crate::SeekLifecycleStage::SeekApplied,
                location: crate::SegmentLocation::default(),
            }),
            Err(error) => {
                let failure = TrackFailureKind::from(error);
                let first_failure = !matches!(self.phase, OwnerPhase::Failed { .. });
                self.fail(failure, None);
                if first_failure
                    && !self.failure_logged
                    && let crate::AudioReadError::Decode(error) = error
                {
                    warn!(err = ?error, "track failed: decode error");
                    self.failure_logged = true;
                }
                self.emit
                    .enqueue(AudioEvent::SeekRejected { target: position });
            }
        }
        self.finish_deferred();
        result
    }
    fn host_sample_rate(&self) -> Option<NonZeroU32> {
        self.host_rate
    }
    fn set_host_sample_rate(&mut self, rate: NonZeroU32) {
        if matches!(self.phase, OwnerPhase::Failed { .. }) || self.host_rate == Some(rate) {
            return;
        }
        let initial_binding = self.host_rate.is_none();
        self.host_rate = Some(rate);
        if initial_binding && self.decode.output_spec().sample_rate == rate {
            return;
        }
        let landing = self.resume.source_end().map_or_else(
            || {
                let spec = self.decode.output_spec();
                spec.frame_at(self.playhead.position())
                    .map(|frame| crate::SourceEnd::new(frame, spec.sample_rate))
                    .map_err(DecodeError::from)
            },
            Ok,
        );
        let result = landing.and_then(|landing| {
            self.shared_stream
                .seek_time_anchor(ResumeTarget::Source(landing).position()?)
                .map_err(|source| DecodeError::Io { source })
                .and_then(|_| {
                    let media_info = self
                        .decode
                        .active()
                        .media_info()
                        .cloned()
                        .or_else(|| self.shared_stream.media_info());
                    self.install_replacement(
                        RecreateState {
                            media_info,
                            offset: self.decode.active().base_offset(),
                            cause: RecreateCause::HostRateChange,
                        },
                        Some(landing),
                    )
                })
        });
        if let Err(error) = result {
            self.fail(
                TrackFailureKind::RecreateFailed {
                    offset: self.decode.active().base_offset(),
                },
                Some(error),
            );
        }
        self.finish_deferred();
    }
    fn finish_deferred(&mut self) {
        self.finish_failure_diagnostic();
        if let Some(wake) = self.shared_stream.peer_wake() {
            wake.flush();
        }
        self.emit.flush();
    }
    fn prepare_deferred(&mut self) -> Option<kithara_signal::AudioSpec> {
        self.progress_variant_transition();
        if matches!(self.phase, OwnerPhase::Decoding) {
            self.decode.prepare_deferred();
        }
        Some(self.decode.output_spec())
    }
    fn step_track(&mut self) -> TrackStep<AudioChunk> {
        match &mut self.phase {
            OwnerPhase::AtEof => return TrackStep::Eof,
            OwnerPhase::Failed { failure, .. } => return TrackStep::Failed(*failure),
            OwnerPhase::Decoding => {}
        }
        if self.shared_stream.phase() == kithara_stream::SourcePhase::Cancelled {
            return TrackStep::Failed(self.fail(TrackFailureKind::SourceCancelled, None));
        }
        let action = tick(
            &mut self.decode,
            DecodeCtx {
                cursor: &mut self.resume,
                stream: &self.shared_stream,
                playhead: self.playhead.as_ref(),
                emit: Some(&self.emit),
            },
        );
        match action {
            DecodeAction::Produced(fetch) => TrackStep::Produced(fetch),
            DecodeAction::Progress => {
                self.wake.wake();
                TrackStep::StateChanged
            }
            DecodeAction::Pending(reason) => TrackStep::Blocked(reason),
            DecodeAction::TransitionPending => TrackStep::Blocked(self.transition_wait_reason()),
            DecodeAction::StartRecreate(recreate) => {
                let offset = recreate.offset;
                match self.install_replacement(recreate, self.resume.source_end()) {
                    Ok(()) => {
                        self.wake.wake();
                        TrackStep::StateChanged
                    }
                    Err(error) => {
                        let failure = TrackFailureKind::RecreateFailed { offset };
                        TrackStep::Failed(self.fail(failure, Some(error)))
                    }
                }
            }
            DecodeAction::Eof => {
                if !matches!(self.phase, OwnerPhase::Failed { .. }) {
                    self.phase = OwnerPhase::AtEof;
                }
                self.emit.enqueue(AudioEvent::EndOfStream);
                TrackStep::Eof
            }
            DecodeAction::Failed(error) => {
                let failure = TrackFailureKind::Decode {
                    kind: crate::map_decode_error_kind(&error),
                };
                TrackStep::Failed(self.fail(failure, Some(error)))
            }
        }
    }
    fn warm_up(&mut self) {
        let _ = self.shared_stream.len();
    }
}

impl<T: StreamType> Drop for StreamAudioSource<T> {
    fn drop(&mut self) {
        self.abandon_incoming();
        self.finish_deferred();
    }
}
#[cfg(test)]
mod resolve_format_change_target_tests {
    use kithara_stream::{AudioCodec, ContainerFormat, MediaInfo};
    use kithara_test_utils::kithara;

    use crate::pipeline::decode::format::resolve_target;

    fn info(
        codec: Option<AudioCodec>,
        container: Option<ContainerFormat>,
        variant: Option<u32>,
    ) -> MediaInfo {
        let mut info = MediaInfo::builder()
            .maybe_codec(codec)
            .maybe_container(container)
            .build();
        info.variant_index = variant;
        info
    }

    #[kithara::test]
    fn no_change_when_variant_index_matches() {
        let cached = info(
            Some(AudioCodec::AacLc),
            Some(ContainerFormat::Fmp4),
            Some(0),
        );
        let current = info(
            Some(AudioCodec::AacLc),
            Some(ContainerFormat::Fmp4),
            Some(0),
        );
        assert!(resolve_target(Some(&cached), &current).is_none());
    }

    #[kithara::test]
    fn same_codec_fmp4_variant_change_recreates_boundary() {
        let cached = info(
            Some(AudioCodec::AacLc),
            Some(ContainerFormat::Fmp4),
            Some(0),
        );
        let current = info(
            Some(AudioCodec::AacLc),
            Some(ContainerFormat::Fmp4),
            Some(1),
        );
        let target = resolve_target(Some(&cached), &current)
            .expect("same-codec fMP4 variant change must re-prime the demuxer");
        assert_eq!(target.variant_index, Some(1));
        assert_eq!(target.codec, Some(AudioCodec::AacLc));
        assert_eq!(target.container, Some(ContainerFormat::Fmp4));
    }

    #[kithara::test]
    fn same_codec_wav_variant_change_is_byte_continuity() {
        let cached = info(Some(AudioCodec::Pcm), Some(ContainerFormat::Wav), Some(0));
        let current = info(Some(AudioCodec::Pcm), Some(ContainerFormat::Wav), Some(1));
        assert!(resolve_target(Some(&cached), &current).is_none());
    }

    #[kithara::test]
    fn variant_change_keeps_cached_codec_and_container_when_current_disagrees() {
        let cached = info(Some(AudioCodec::Pcm), Some(ContainerFormat::Wav), Some(0));
        let current = info(None, Some(ContainerFormat::Fmp4), Some(1));
        let target = resolve_target(Some(&cached), &current).expect("variant change must trigger");
        assert_eq!(target.codec, Some(AudioCodec::Pcm));
        assert_eq!(target.container, Some(ContainerFormat::Wav));
        assert_eq!(target.variant_index, Some(1));
    }

    #[kithara::test]
    fn variant_change_falls_back_to_current_when_cached_lacks_codec_or_container() {
        let cached = info(None, None, Some(0));
        let current = info(
            Some(AudioCodec::AacLc),
            Some(ContainerFormat::Fmp4),
            Some(2),
        );
        let target = resolve_target(Some(&cached), &current).expect("variant change must trigger");
        assert_eq!(target.codec, Some(AudioCodec::AacLc));
        assert_eq!(target.container, Some(ContainerFormat::Fmp4));
        assert_eq!(target.variant_index, Some(2));
    }

    #[kithara::test]
    fn no_cached_uses_current_directly() {
        let current = info(
            Some(AudioCodec::AacLc),
            Some(ContainerFormat::Fmp4),
            Some(1),
        );
        let target =
            resolve_target(None, &current).expect("None cached + Some(variant) must trigger");
        assert_eq!(target, current);
    }

    #[kithara::test]
    fn explicit_codec_change_takes_current_codec() {
        let cached = info(Some(AudioCodec::AacLc), Some(ContainerFormat::Fmp4), None);
        let current = info(Some(AudioCodec::Flac), Some(ContainerFormat::Fmp4), None);
        let target = resolve_target(Some(&cached), &current).expect("codec change must trigger");
        assert_eq!(target.codec, Some(AudioCodec::Flac));
        assert_eq!(target.container, Some(ContainerFormat::Fmp4));
    }

    #[kithara::test]
    fn current_codec_none_is_not_a_codec_change() {
        let cached = info(
            Some(AudioCodec::AacLc),
            Some(ContainerFormat::Fmp4),
            Some(0),
        );
        let current = info(None, Some(ContainerFormat::Fmp4), Some(0));
        assert!(resolve_target(Some(&cached), &current).is_none());
    }

    #[kithara::test]
    fn no_change_when_neither_side_has_variant() {
        let cached = info(Some(AudioCodec::AacLc), Some(ContainerFormat::Fmp4), None);
        let current = info(Some(AudioCodec::AacLc), Some(ContainerFormat::Fmp4), None);
        assert!(resolve_target(Some(&cached), &current).is_none());
    }
}
