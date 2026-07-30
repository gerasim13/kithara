use std::{
    any::Any,
    mem,
    panic::{AssertUnwindSafe, catch_unwind},
    sync::atomic::{AtomicU32, Ordering},
};

use kithara_decode::{
    DecodeError, DecodeResult, Decoder, DecoderChunkOutcome, DecoderSeekOutcome, GaplessMode,
    PcmChunk,
};
use kithara_events::{DeferredBus, Event};
use kithara_platform::sync::Arc;
use kithara_stream::{MediaInfo, OpenedReader, PlayheadWrite, SeekObserve, StreamType};
use kithara_test_utils::kithara;
use tracing::{debug, warn};

use crate::{
    pipeline::{
        blend::PcmBlender,
        decode::{drain::EofDrain, generation::DecoderGeneration, resume::ResumeCursor},
        fetch::Fetch,
        rebuild::RecreateState,
        seek::{ResumeState, SeekEngine, emit::commit_outcome},
        stream::shared::SharedStream,
        track::{TrackFailure, WaitingReason},
    },
    renderer::{apply_effects, reset_effects},
    traits::AudioEffect,
};

/// Factory closure that creates a decoder from an opened reader and media facts.
pub(crate) type DecoderFactory =
    Arc<dyn Fn(OpenedReader, MediaInfo) -> Result<Box<dyn Decoder>, DecodeError> + Send + Sync>;

/// Decoder construction state shared by initial installation and later rebuilds.
pub(crate) struct DecodeInit {
    pub(crate) decoder: Box<dyn Decoder>,
    pub(crate) decoder_factory: DecoderFactory,
    pub(crate) decoder_backend: kithara_decode::DecoderBackend,
    pub(crate) gapless_mode: GaplessMode,
    pub(crate) host_sample_rate: Arc<AtomicU32>,
    pub(crate) media_info: Option<MediaInfo>,
    pub(crate) playback_resampler_backend: &'static str,
    pub(crate) recreate_on_host_rate_change: bool,
}

pub(crate) struct DecodeParts {
    pub(crate) active: ActiveDecode,
    pub(crate) factory: DecoderFactory,
    pub(crate) host_sample_rate: Arc<AtomicU32>,
    pub(crate) recreate_on_host_rate_change: bool,
    pub(crate) decoder_host_sample_rate: u32,
    pub(crate) decoder_backend: kithara_decode::DecoderBackend,
    pub(crate) playback_resampler_backend: &'static str,
}

impl DecodeInit {
    pub(crate) fn decoder_host_sample_rate(&self) -> u32 {
        self.host_sample_rate.load(Ordering::Acquire)
    }

    pub(crate) fn into_parts(
        self,
        effects: Vec<Box<dyn AudioEffect>>,
        installed_at_seek_epoch: u64,
    ) -> DecodeParts {
        let decoder_host_sample_rate = self.decoder_host_sample_rate();
        let Self {
            decoder,
            decoder_factory,
            decoder_backend,
            gapless_mode,
            host_sample_rate,
            media_info,
            playback_resampler_backend,
            recreate_on_host_rate_change,
        } = self;
        let active = DecoderGeneration::new(
            decoder,
            media_info,
            0,
            installed_at_seek_epoch,
            gapless_mode,
        );
        DecodeParts {
            active: ActiveDecode::new(active, gapless_mode, effects),
            factory: decoder_factory,
            host_sample_rate,
            recreate_on_host_rate_change,
            decoder_host_sample_rate,
            decoder_backend,
            playback_resampler_backend,
        }
    }
}

pub(crate) struct ActiveDecode {
    active: DecoderGeneration,
    gapless_mode: GaplessMode,
    blender: PcmBlender,
    effects: Vec<Box<dyn AudioEffect>>,
    drain: EofDrain,
}

pub(crate) struct DecodeCtx<'a, T: StreamType> {
    pub(crate) emit: Option<&'a DeferredBus<Event>>,
    pub(crate) playhead: &'a dyn PlayheadWrite,
    pub(crate) resume: Option<&'a mut ResumeState>,
    pub(crate) cursor: &'a mut ResumeCursor,
    pub(crate) seek: &'a SeekEngine,
    pub(crate) seek_observe: &'a dyn SeekObserve,
    pub(crate) stream: &'a SharedStream<T>,
}

pub(crate) enum DecodeAction {
    Produced(Fetch<PcmChunk>),
    Pending(WaitingReason),
    StartRecreate(RecreateState),
    SeekInterrupted,
    Eof,
    Failed(TrackFailure),
}

impl ActiveDecode {
    fn new(
        active: DecoderGeneration,
        gapless_mode: GaplessMode,
        effects: Vec<Box<dyn AudioEffect>>,
    ) -> Self {
        let drain = EofDrain::new(effects.len());
        let blender = PcmBlender::new(active.blender_profile());
        Self {
            active,
            gapless_mode,
            blender,
            effects,
            drain,
        }
    }

    pub(crate) fn active(&self) -> &DecoderGeneration {
        &self.active
    }

    pub(crate) fn gapless_mode(&self) -> GaplessMode {
        self.gapless_mode
    }

    pub(crate) fn reset(&mut self) {
        reset_effects(&mut self.effects);
        self.drain.reset();
    }

    pub(crate) fn notify_seek(&mut self) {
        self.active.notify_seek();
    }

    pub(crate) fn set_tail_compensation(&mut self) {
        self.active.finish();
    }

    pub(crate) fn push(&mut self, chunk: PcmChunk) {
        self.active.push(chunk);
    }

    pub(crate) fn track(
        &mut self,
        chunk: &PcmChunk,
        playhead: &dyn PlayheadWrite,
        emit: Option<&DeferredBus<Event>>,
    ) {
        self.drain.track(chunk, playhead, emit);
    }

    pub(crate) fn next_output(&mut self) -> Option<PcmChunk> {
        while let Some(chunk) = self.active.next() {
            let chunk = self.blender.process_active(chunk);
            if let Some(output) = apply_effects(&mut self.effects, chunk) {
                return Some(output);
            }
        }
        None
    }

    pub(crate) fn next_drain(&mut self) -> Option<PcmChunk> {
        self.drain.next(&mut self.effects)
    }

    pub(crate) fn stats(&self) -> (u64, u64) {
        self.drain.stats()
    }

    #[kithara::rtsan_allow_blocking]
    pub(crate) fn next_chunk(&mut self, stream_position: u64) -> DecodeResult<DecoderChunkOutcome> {
        let outcome =
            match catch_unwind(AssertUnwindSafe(|| self.active.decoder_mut().next_chunk())) {
                Ok(result) => result,
                Err(payload) => {
                    warn!(panic = %panic_message(payload), "decoder panicked during next_chunk");
                    Err(DecodeError::InvalidData {
                        detail: "decoder panicked during next_chunk",
                    })
                }
            };
        let (chunks, samples) = self.stats();
        match &outcome {
            Ok(DecoderChunkOutcome::Eof) => {
                debug!(
                    chunks,
                    samples,
                    pos = stream_position,
                    "decoder returned EOF"
                );
            }
            Err(error) => {
                debug!(error_class = ?error.classify(), chunks, samples, pos = stream_position, "decoder returned error");
            }
            Ok(DecoderChunkOutcome::Chunk(_) | DecoderChunkOutcome::Pending(_)) => {}
        }
        outcome
    }

    #[kithara::rtsan_allow_blocking]
    pub(crate) fn seek<T: StreamType>(
        &mut self,
        stream: &SharedStream<T>,
        playhead: &dyn PlayheadWrite,
        position: kithara_platform::time::Duration,
    ) -> DecodeResult<DecoderSeekOutcome> {
        let before = stream.position();
        let outcome = match catch_unwind(AssertUnwindSafe(|| {
            self.active.decoder_mut().seek(position)
        })) {
            Ok(result) => result,
            Err(payload) => {
                warn!(panic = %panic_message(payload), "decoder panicked during seek");
                return Err(DecodeError::InvalidData {
                    detail: "decoder panicked during seek",
                });
            }
        };
        if let Ok(ref outcome) = outcome {
            commit_outcome(&self.active, stream, playhead, outcome);
        }
        debug!(
            ?position,
            before,
            after = stream.position(),
            ?outcome,
            "decoder seek completed"
        );
        outcome
    }

    pub(crate) fn update_len(&self, len: u64) {
        self.active.decoder().update_byte_len(len);
    }

    pub(crate) fn replace_active(&mut self, active: DecoderGeneration) -> DecoderGeneration {
        self.blender.replace_active(active.blender_profile());
        mem::replace(&mut self.active, active)
    }

    pub(crate) fn flush_reader_signals(&mut self) {
        self.active.decoder_mut().flush_reader_signals();
    }
}

fn panic_message(payload: Box<dyn Any + Send>) -> String {
    match payload.downcast::<String>() {
        Ok(message) => *message,
        Err(payload) => payload.downcast::<&'static str>().map_or_else(
            |_| "unknown panic payload".to_string(),
            |message| (*message).to_string(),
        ),
    }
}
