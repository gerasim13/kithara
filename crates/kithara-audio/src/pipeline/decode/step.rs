use kithara_decode::{DecodeError, DecoderChunkOutcome, ErrorClass};
use kithara_events::{AudioEvent, DecoderEvent, SeekLifecycleStage, SegmentLocation};
use kithara_signal::AudioChunk;
use kithara_stream::{PendingReason, StreamType};
use kithara_test_utils::kithara;

use crate::{
    audio::event::{
        decode_error_detail, map_audio_codec_kind, map_decode_error_class, map_decode_error_kind,
    },
    pipeline::{
        decode::{
            core::{ActiveDecode, DecodeAction, DecodeCtx},
            format::{FormatDecision, detect},
        },
        fetch::Fetch,
        gapless::visible_duration,
        seek::skip::apply as apply_skip,
        track::{TrackFailure, WaitingReason},
    },
};

#[kithara::measure(label = "audio.decode.step")]
#[kithara::hang_watchdog]
pub(crate) fn tick<T: StreamType>(
    core: &mut ActiveDecode,
    mut ctx: DecodeCtx<'_, T>,
) -> DecodeAction {
    let active = core.active();
    if let Some(duration) = visible_duration(
        active.decoder().duration(),
        active.gapless_profile(),
        core.gapless_mode(),
    ) && Some(duration) > ctx.playhead.duration()
    {
        ctx.playhead.set_duration(Some(duration));
    }
    let epoch = ctx.seek.epoch();
    loop {
        if ctx.seek_observe.is_flushing() || ctx.seek_observe.is_pending() {
            return DecodeAction::SeekInterrupted;
        }
        if let Some(error) = core.take_stage_error() {
            return decode_failed(core, error, &ctx);
        }
        match core.next_output(&mut *ctx.cursor, epoch) {
            Ok(Some(chunk)) => return produced(chunk, epoch, &mut ctx),
            Ok(None) => {}
            Err(error) => return decode_failed(core, error, &ctx),
        }
        if core.transition_holds_output() && !core.outgoing_holdback_needs_pcm() {
            return transition_hold(core, &ctx);
        }
        if core.active().is_source_exhausted() {
            if core.has_live_incoming() {
                // WHY: Exhausted with the switch still in flight but the output hold not engaged (frontier unlatched or blender mid-join): surfacing
                // EOF here would latch AtEof and abort the pending intent, so park as a transition wait instead.
                return transition_hold(core, &ctx);
            }
            if !core.active().is_exhaustion_observed() {
                // WHY: Defer finalization by exactly one tick: the scheduler runs the transition driver between ticks, so an intent that raced the
                // last chunk still gets its incoming slot planted before EOF finalizes and AtEof can latch.
                core.observe_source_exhaustion();
                return DecodeAction::TransitionPending;
            }
            core.finish_active();
            match core.next_output_unheld(&mut *ctx.cursor, epoch) {
                Ok(Some(chunk)) => return produced(chunk, epoch, &mut ctx),
                Ok(None) => {}
                Err(error) => return decode_failed(core, error, &ctx),
            }
            if let FormatDecision::Recreate(recreate) =
                detect(ctx.stream, core.active(), ctx.seek_observe)
            {
                return DecodeAction::StartRecreate(recreate);
            }
            return DecodeAction::Eof;
        }
        match core.next_chunk(ctx.stream.position()) {
            Ok(DecoderChunkOutcome::Pending(PendingReason::VariantChange)) => {
                return variant_change(core, &ctx);
            }
            Ok(DecoderChunkOutcome::Pending(_)) => {
                return DecodeAction::Pending(WaitingReason::Waiting);
            }
            Ok(DecoderChunkOutcome::Chunk(chunk)) => {
                let Some(chunk) = apply_skip(chunk, epoch, ctx.resume.as_deref_mut()) else {
                    continue;
                };
                if chunk.samples.is_empty() {
                    continue;
                }
                hang_reset!();
                core.track(&chunk, ctx.emit);
                if let Err(error) = core.push(chunk) {
                    return decode_failed(core, error, &ctx);
                }
            }
            Ok(DecoderChunkOutcome::Eof) => {
                core.mark_source_exhausted();
            }
            Err(error) if error.classify() == ErrorClass::VariantChange => {
                return variant_change(core, &ctx);
            }
            Err(error) if error.classify() == ErrorClass::Interrupted => {}
            Err(error) => return decode_failed(core, error, &ctx),
        }
    }
}

fn transition_hold<T: StreamType>(core: &mut ActiveDecode, ctx: &DecodeCtx<'_, T>) -> DecodeAction {
    if core.announce_transition_hold()
        && let Some(emit) = ctx.emit
    {
        emit.enqueue(
            DecoderEvent::TransitionHold {
                source_exhausted: core.active().is_source_exhausted(),
            }
            .into(),
        );
    }
    DecodeAction::TransitionPending
}

fn decode_failed<T: StreamType>(
    core: &ActiveDecode,
    error: DecodeError,
    ctx: &DecodeCtx<'_, T>,
) -> DecodeAction {
    if let Some(emit) = ctx.emit {
        emit.enqueue(
            DecoderEvent::DecodeError {
                class: map_decode_error_class(error.classify()),
                kind: map_decode_error_kind(&error),
                codec: core
                    .active()
                    .media_info()
                    .and_then(|info| info.codec)
                    .map(map_audio_codec_kind),
                detail: decode_error_detail(&error),
            }
            .into(),
        );
    }
    DecodeAction::Failed(TrackFailure::Decode(error))
}

pub(crate) fn produced<T: StreamType>(
    chunk: AudioChunk,
    epoch: u64,
    ctx: &mut DecodeCtx<'_, T>,
) -> DecodeAction {
    if ctx
        .resume
        .as_deref()
        .is_some_and(|resume| resume.seek.epoch == epoch)
        && let Some(emit) = ctx.emit
    {
        emit.enqueue(
            AudioEvent::SeekLifecycle {
                stage: SeekLifecycleStage::DecodeStarted,
                seek_epoch: epoch,
                location: SegmentLocation::new(
                    chunk.meta.variant_index,
                    chunk.meta.segment_index,
                    None,
                    None,
                ),
            }
            .into(),
        );
    }
    DecodeAction::Produced(Fetch::data(chunk, epoch))
}

pub(crate) fn variant_change<T: StreamType>(
    core: &ActiveDecode,
    ctx: &DecodeCtx<'_, T>,
) -> DecodeAction {
    match detect(ctx.stream, core.active(), ctx.seek_observe) {
        FormatDecision::Recreate(recreate) => DecodeAction::StartRecreate(recreate),
        FormatDecision::None => DecodeAction::SeekInterrupted,
    }
}
