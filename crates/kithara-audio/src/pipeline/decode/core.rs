use std::{
    any::Any,
    panic::{AssertUnwindSafe, catch_unwind},
    sync::atomic::{AtomicU32, Ordering},
};

use kithara_decode::{
    DecodeError, DecodeResult, Decoder, DecoderChunkOutcome, DecoderSeekOutcome, GaplessMode,
    PcmChunk,
};
use kithara_events::{DeferredBus, Event};
use kithara_platform::sync::Arc;
use kithara_stream::{MediaInfo, PlayheadWrite, SeekObserve, SharedStream, StreamType};
use kithara_test_utils::kithara;
use tracing::{debug, warn};

use crate::{
    pipeline::{
        blend::{ActiveDecode, BlendSide, Origin},
        decode::{drain::EofDrain, resume::ResumeCursor},
        fetch::Fetch,
        gapless::GaplessStage,
        rebuild::RecreateState,
        seek::{ResumeState, SeekEngine, emit::commit_outcome},
        track::{TrackFailure, WaitingReason},
    },
    renderer::{apply_effects, reset_effects},
    traits::AudioEffect,
};

/// Decoder and its associated metadata, installed as an atomic unit.
pub(crate) struct DecoderSession {
    pub(crate) decoder: Box<dyn Decoder>,
    pub(crate) media_info: Option<MediaInfo>,
    pub(crate) base_offset: u64,
    pub(crate) installed_at_seek_epoch: u64,
}

/// Factory closure that creates a new decoder from stream, media info, and base offset.
///
/// Production opens a reader session on the stream and builds a decoder over
/// it; tests may return a mock decoder without real I/O. Interrupted
/// construction remains distinct
/// from a hard decoder or codec error so recreation can wait for source bytes.
pub(crate) type DecoderFactory<T> = Arc<
    dyn Fn(SharedStream<T>, MediaInfo, u64) -> Result<Box<dyn Decoder>, DecodeError> + Send + Sync,
>;

/// Decoder construction state shared by initial installation and later rebuilds.
pub(crate) struct DecodeInit<T: StreamType> {
    pub(crate) decoder: Box<dyn Decoder>,
    pub(crate) decoder_factory: DecoderFactory<T>,
    pub(crate) decoder_backend: kithara_decode::DecoderBackend,
    pub(crate) gapless_mode: GaplessMode,
    pub(crate) host_sample_rate: Arc<AtomicU32>,
    pub(crate) media_info: Option<MediaInfo>,
    pub(crate) playback_resampler_backend: &'static str,
    pub(crate) recreate_on_host_rate_change: bool,
}

pub(crate) struct DecodeParts<T: StreamType> {
    pub(crate) core: DecodeCore,
    pub(crate) factory: DecoderFactory<T>,
    pub(crate) host_sample_rate: Arc<AtomicU32>,
    pub(crate) recreate_on_host_rate_change: bool,
    pub(crate) decoder_host_sample_rate: u32,
    pub(crate) decoder_backend: kithara_decode::DecoderBackend,
    pub(crate) playback_resampler_backend: &'static str,
}

impl<T: StreamType> DecodeInit<T> {
    pub(crate) fn build_gapless(&self) -> GaplessStage {
        GaplessStage::build(
            self.decoder.as_ref(),
            self.gapless_mode,
            self.media_info.as_ref(),
        )
    }

    pub(crate) fn decoder_host_sample_rate(&self) -> u32 {
        self.host_sample_rate.load(Ordering::Acquire)
    }

    pub(crate) fn into_parts(
        self,
        effects: Vec<Box<dyn AudioEffect>>,
        installed_at_seek_epoch: u64,
    ) -> DecodeParts<T> {
        let gapless = self.build_gapless();
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
        DecodeParts {
            core: DecodeCore::new(
                DecoderSession {
                    decoder,
                    base_offset: 0,
                    media_info,
                    installed_at_seek_epoch,
                },
                gapless_mode,
                gapless,
                effects,
            ),
            factory: decoder_factory,
            host_sample_rate,
            recreate_on_host_rate_change,
            decoder_host_sample_rate,
            decoder_backend,
            playback_resampler_backend,
        }
    }
}

pub(crate) struct DecodeCore {
    /// Everything the blender is mixing. Always present, even at one input —
    /// there is one PCM path, and `Single` is its one-input arm.
    active: ActiveDecode,
    /// The scale this track's positions are quoted on: the first generation's
    /// [`Origin`]. Every later generation is converted onto it, so the track
    /// keeps the timeline it started with and a variant switch cannot move it.
    anchor: Option<Origin>,
    /// The scale the generation now decoding labels its output on, learned
    /// from its first chunk. Cleared when a generation is replaced, because
    /// the next one is free to have a different one.
    origin: Option<Origin>,
    /// Whether the frames held back for the crossfade are quoted on a scale
    /// the generation now decoding also reads. False across a codec change,
    /// where the two sides have no position in common to hand over on.
    tail_comparable: bool,
    gapless_mode: GaplessMode,
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

impl DecodeCore {
    fn new(
        session: DecoderSession,
        gapless_mode: GaplessMode,
        gapless: GaplessStage,
        effects: Vec<Box<dyn AudioEffect>>,
    ) -> Self {
        let drain = EofDrain::new(effects.len());
        Self {
            active: ActiveDecode::Single(BlendSide::new(session, gapless)),
            anchor: None,
            origin: None,
            tail_comparable: true,
            gapless_mode,
            effects,
            drain,
        }
    }

    pub(crate) fn session(&self) -> &DecoderSession {
        &self.active.audible().session
    }

    pub(crate) fn gapless_mode(&self) -> GaplessMode {
        self.gapless_mode
    }

    pub(crate) fn reset(&mut self) {
        reset_effects(&mut self.effects);
        self.drain.reset();
    }

    pub(crate) fn notify_seek(&mut self) {
        let side = self.active.audible_mut();
        side.gapless.notify_seek();
        // Frames held back for a ramp belong to where the reader used to be.
        // A seek is the one event that makes them not audio anymore.
        side.drop_staged();
    }

    pub(crate) fn set_tail_compensation(&mut self) {
        let side = self.active.audible_mut();
        side.gapless
            .set_tail_compensation(side.session.decoder.track_info().gapless_tail);
        side.gapless.flush();
    }

    pub(crate) fn push(&mut self, chunk: PcmChunk) {
        self.active.audible_mut().gapless.push(chunk);
    }

    /// Put a chunk on the track's scale before anything downstream reads its
    /// position. The first chunk of the track defines that scale; the first
    /// chunk of every later generation only says where *that* generation is
    /// counting from.
    fn rebase(&mut self, chunk: &mut PcmChunk) {
        let origin = *self.origin.get_or_insert_with(|| Origin::of(chunk));
        let anchor = *self.anchor.get_or_insert(origin);
        origin.rebase(anchor, chunk);
    }

    /// Where the crossfade starts: the first frame the audible side is holding
    /// back, quoted on the track's scale. This is the frame the incoming
    /// generation has to produce first for the two sides to line up.
    ///
    /// Nothing to hand over when the scales differ — the position would mean
    /// one thing to the generation that measured it and another to the one
    /// being asked to land on it.
    pub(crate) fn blend_start(&self) -> Option<kithara_platform::time::Duration> {
        self.tail_comparable
            .then(|| self.active.audible().tail_start())
            .flatten()
    }

    pub(crate) fn track(
        &mut self,
        chunk: &PcmChunk,
        playhead: &dyn PlayheadWrite,
        emit: Option<&DeferredBus<Event>>,
    ) {
        self.drain.track(chunk, playhead, emit);
    }

    /// One chunk out of the blender, with the effect chain applied once on top
    /// of whatever it mixed.
    pub(crate) fn next_gapless(&mut self) -> Option<PcmChunk> {
        while let Some(chunk) = self.active.next() {
            if let Some(output) = apply_effects(&mut self.effects, chunk) {
                return Some(output);
            }
        }
        None
    }

    /// The blender's output at end of stream: what it was holding back for a
    /// ramp is owed to the listener, because no generation is coming to ramp
    /// into.
    pub(crate) fn release_gapless(&mut self) -> Option<PcmChunk> {
        while let Some(chunk) = self.active.release() {
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
        let mut outcome = match catch_unwind(AssertUnwindSafe(|| {
            self.active.audible_mut().session.decoder.next_chunk()
        })) {
            Ok(result) => result,
            Err(payload) => {
                warn!(panic = %panic_message(payload), "decoder panicked during next_chunk");
                Err(DecodeError::InvalidData {
                    detail: "decoder panicked during next_chunk",
                })
            }
        };
        if let Ok(DecoderChunkOutcome::Chunk(ref mut chunk)) = outcome {
            self.rebase(chunk);
        }
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
            self.active.audible_mut().session.decoder.seek(position)
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
            commit_outcome(&self.active.audible().session, stream, playhead, outcome);
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
        self.active.audible().session.decoder.update_byte_len(len);
    }

    pub(crate) fn install(
        &mut self,
        decoder: Box<dyn Decoder>,
        media_info: MediaInfo,
        offset: u64,
        seek_epoch: u64,
    ) -> Box<dyn Decoder> {
        // An origin is a reading on the container's clock, so it only compares
        // between generations reading the same one. Two variants of a track
        // share that clock; two codecs do not — their containers carry their
        // own priming — so a codec change starts a scale rather than joining
        // the one in use. What that leaves is the pre-existing offset between
        // codecs, which is not this scale's to close.
        let same_clock = self
            .session()
            .media_info
            .as_ref()
            .and_then(|info| info.codec)
            == media_info.codec;
        let session = DecoderSession {
            decoder,
            media_info: Some(media_info),
            base_offset: offset,
            installed_at_seek_epoch: seek_epoch,
        };
        // The successor counts from wherever its own decoder starts, which is
        // not where the retiring one counted from. Forgetting the old scale is
        // what makes `rebase` read the new one off its first chunk; the anchor
        // stays, so the track keeps the timeline it has been playing on.
        self.origin = None;
        self.tail_comparable = same_clock;
        if !same_clock {
            self.anchor = None;
        }
        // The replacement decoder takes over the audible side and keeps that
        // side's gapless stage: the logical track is unchanged, only the
        // generation decoding it. What the outgoing generation was still
        // holding back becomes the material its successor fades out of.
        self.active.audible_mut().hand_over(session)
    }

    pub(crate) fn flush_reader_signals(&mut self) {
        self.active
            .audible_mut()
            .session
            .decoder
            .flush_reader_signals();
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
