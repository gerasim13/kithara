use std::{
    num::NonZeroU32,
    sync::atomic::{AtomicU32, Ordering},
};

use kithara_platform::{sync::Arc, time::Duration};
use kithara_signal::{AudioChunk, AudioSpec};
use kithara_stream::StreamType;

use crate::pipeline::{
    decode::DecoderGeneration,
    rebuild::{RecreateCause, RecreateNext, RecreateState},
    seek::{SeekContext, SeekEngine, SeekRequest, anchor},
    stream::shared::SharedStream,
};

#[derive(fieldwork::Fieldwork)]
#[fieldwork(opt_in, get)]
pub(crate) struct ResumeCursor {
    host_rate: Arc<AtomicU32>,
    decode_head: Option<(u64, u64, u32)>,
    rendered_source_head: Option<(u64, u64, u32)>,
    #[field(get = recreates_on_route, vis = "pub(crate)")]
    recreate_on_route: bool,
    decoder_rate: u32,
}

pub(crate) struct RouteCtx<'a, T: StreamType> {
    pub(crate) active: &'a DecoderGeneration,
    pub(crate) seek: &'a SeekEngine,
    pub(crate) stream: &'a SharedStream<T>,
    pub(crate) committed: Duration,
    pub(crate) seek_active: bool,
}

impl ResumeCursor {
    pub(crate) const fn new(
        host_rate: Arc<AtomicU32>,
        recreate_on_route: bool,
        decoder_rate: u32,
    ) -> Self {
        Self {
            host_rate,
            recreate_on_route,
            decoder_rate,
            decode_head: None,
            rendered_source_head: None,
        }
    }

    pub(crate) fn commit_source_end(&mut self, source_end: crate::SourceEnd, epoch: u64) {
        self.rendered_source_head =
            Some((epoch, source_end.frame(), source_end.sample_rate().get()));
    }

    pub(crate) fn decode_head(&self, epoch: u64) -> Option<(u64, u32)> {
        self.decode_head
            .filter(|&(head_epoch, _, _)| head_epoch == epoch)
            .map(|(_, frame, rate)| (frame, rate))
    }

    #[cfg(test)]
    pub(crate) const fn decoder_rate(&self) -> u32 {
        self.decoder_rate
    }

    pub(crate) fn host_rate(&self) -> u32 {
        self.host_rate.load(Ordering::Acquire)
    }

    pub(crate) fn rebase_decode_to_rendered(&mut self, epoch: u64) {
        self.decode_head = self
            .rendered_source_head
            .filter(|&(head_epoch, _, _)| head_epoch == epoch);
    }

    pub(crate) fn record(&mut self, chunk: &AudioChunk, epoch: u64) {
        self.decode_head = Some((
            epoch,
            chunk
                .meta
                .frame_offset
                .saturating_add(u64::from(chunk.meta.frames)),
            chunk.meta.spec.sample_rate.get(),
        ));
    }

    pub(crate) fn rendered_source_head(&self, epoch: u64) -> Option<(u64, u32)> {
        self.rendered_source_head
            .filter(|&(head_epoch, _, _)| head_epoch == epoch)
            .map(|(_, frame, rate)| (frame, rate))
    }

    pub(crate) fn resume_position(
        &self,
        epoch: u64,
        committed: Duration,
        resume_target: Option<(u64, Duration)>,
    ) -> Duration {
        let head = self
            .rendered_source_head(epoch)
            .and_then(|(frame, rate)| {
                NonZeroU32::new(rate).map(|sample_rate| {
                    let spec = AudioSpec::new(1, sample_rate);
                    spec.duration_for(frame)
                        .unwrap_or(Duration::from_nanos(u64::MAX))
                })
            })
            .filter(|&position| position > committed)
            .unwrap_or(committed);
        match resume_target {
            Some((target_epoch, target)) if target_epoch == epoch && target > head => target,
            _ => head,
        }
    }

    pub(crate) fn route_change<T: StreamType>(
        &mut self,
        ctx: &RouteCtx<'_, T>,
    ) -> Option<RecreateState> {
        if !self.recreate_on_route || ctx.seek_active {
            return None;
        }
        let host_rate = self.host_rate.load(Ordering::Acquire);
        if host_rate == 0 {
            return None;
        }
        if self.decoder_rate == 0 && ctx.active.decoder().spec().sample_rate.get() == host_rate {
            self.decoder_rate = host_rate;
            return None;
        }
        if host_rate == self.decoder_rate {
            return None;
        }
        let media_info = ctx
            .active
            .media_info()
            .cloned()
            .or_else(|| ctx.stream.media_info())?;
        // WHY: A route change keeps the container, so the rebuilt demuxer must start where the container starts - not at the byte the resume
        // time maps to.
        let offset = if ctx.stream.has_variant_surface() {
            anchor::recreate_offset(
                ctx.stream,
                media_info.container,
                false,
                ctx.active.base_offset(),
            )?
        } else {
            ctx.active.base_offset()
        };
        let epoch = ctx.seek.epoch();
        let target = self.resume_position(epoch, ctx.committed, None);
        self.decoder_rate = host_rate;
        Some(RecreateState {
            media_info,
            offset,
            cause: RecreateCause::RouteChange,
            next: RecreateNext::ApplySeek(SeekRequest {
                seek: SeekContext { target, epoch },
                emit_request: false,
            }),
        })
    }
}
