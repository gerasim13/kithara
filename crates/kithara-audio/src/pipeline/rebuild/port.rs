use std::{
    io::SeekFrom,
    panic::{AssertUnwindSafe, catch_unwind},
};

use crossbeam_queue::ArrayQueue;
use kithara_decode::GaplessMode;
use kithara_platform::{
    sync::Arc,
    tokio::{runtime::Handle as RuntimeHandle, task::spawn_blocking_on},
};
use kithara_stream::{
    MediaInfo, OpenedReader, OpenedVariantReader, StreamType, VariantTransition, WorkerWake,
};
use tracing::warn;

use crate::pipeline::{
    decode::{core::DecoderFactory, generation::DecoderGeneration},
    rebuild::{
        policy::classify,
        state::{
            BuildId, DecoderBuildComplete, DecoderBuildPurpose, RebuildState, RecreateCause,
            RecreateNext, RecreateOutcome, RecreateState,
        },
    },
    stream::shared::SharedStream,
};

struct JobDeps {
    completion: Arc<ArrayQueue<DecoderBuildComplete>>,
    factory: DecoderFactory,
    gapless_mode: GaplessMode,
    wake: Arc<dyn WorkerWake>,
}

impl Clone for JobDeps {
    fn clone(&self) -> Self {
        Self {
            completion: self.completion.clone(),
            factory: self.factory.clone(),
            gapless_mode: self.gapless_mode,
            wake: self.wake.clone(),
        }
    }
}

enum PendingInput<T: StreamType> {
    Opened(OpenedReader),
    Shared {
        stream: SharedStream<T>,
        offset: u64,
    },
}

struct PendingJob<T: StreamType> {
    build: BuildId,
    deps: JobDeps,
    input: PendingInput<T>,
    media_info: MediaInfo,
    offset: u64,
    purpose: DecoderBuildPurpose,
    seek_epoch: u64,
}

pub(crate) struct RebuildPort<T: StreamType> {
    deps: JobDeps,
    next_build: u64,
    pending: Option<PendingJob<T>>,
    ready_replacement: Option<DecoderBuildComplete>,
    runtime: RuntimeHandle,
}

pub(crate) struct RebuildRuntime {
    pub(crate) handle: RuntimeHandle,
    pub(crate) wake: Arc<dyn WorkerWake>,
}

impl<T: StreamType> RebuildPort<T> {
    const COMPLETION_CAPACITY: usize = 4;

    pub(crate) fn new(
        factory: DecoderFactory,
        gapless_mode: GaplessMode,
        runtime: RebuildRuntime,
    ) -> Self {
        Self {
            deps: JobDeps {
                completion: Arc::new(ArrayQueue::new(Self::COMPLETION_CAPACITY)),
                factory,
                gapless_mode,
                wake: runtime.wake,
            },
            next_build: 1,
            pending: None,
            ready_replacement: None,
            runtime: runtime.handle,
        }
    }

    pub(crate) fn prepare(
        &mut self,
        stream: &SharedStream<T>,
        recreate: RecreateState,
        started_seek_epoch: u64,
    ) -> Result<RebuildState, (RecreateState, RecreateOutcome)> {
        if self.pending.is_some() {
            return Err((recreate, RecreateOutcome::SoftFailed));
        }
        if recreate.cause == RecreateCause::FormatBoundary
            && matches!(recreate.next, RecreateNext::Decode)
        {
            stream.clear_variant_fence();
            if let Err(error) = stream.probe_seek(SeekFrom::Start(recreate.offset)) {
                let outcome = classify(&kithara_decode::DecodeError::from(error));
                return Err((recreate, outcome));
            }
        } else {
            stream.clear_variant_fence();
            if stream.probe_seek(SeekFrom::Start(recreate.offset)).is_err() {
                return Err((recreate, RecreateOutcome::SoftFailed));
            }
            stream.clear_variant_fence();
        }
        let build = self.next_build();
        self.pending = Some(PendingJob {
            build,
            deps: self.deps.clone(),
            input: PendingInput::Shared {
                stream: stream.clone(),
                offset: recreate.offset,
            },
            media_info: recreate.media_info.clone(),
            offset: recreate.offset,
            purpose: DecoderBuildPurpose::Replacement,
            seek_epoch: started_seek_epoch,
        });
        Ok(RebuildState {
            build,
            superseded_seek: None,
            recreate,
            started_seek_epoch,
        })
    }

    pub(crate) fn prepare_incoming(
        &mut self,
        opened: OpenedVariantReader,
    ) -> Option<(VariantTransition, BuildId)> {
        if self.pending.is_some() {
            return None;
        }
        let transition = opened.transition();
        let media_info = opened.media_info().clone();
        let offset = opened.base_offset();
        let seek_epoch = transition.id().seek_epoch();
        let (_plan, reader) = opened.split();
        let build = self.next_build();
        self.pending = Some(PendingJob {
            build,
            deps: self.deps.clone(),
            input: PendingInput::Opened(reader),
            media_info,
            offset,
            purpose: DecoderBuildPurpose::Incoming(transition),
            seek_epoch,
        });
        Some((transition, build))
    }

    pub(crate) const fn can_prepare(&self) -> bool {
        self.pending.is_none()
    }

    pub(crate) fn pop_completion(&self) -> Option<DecoderBuildComplete> {
        self.deps.completion.pop()
    }

    pub(crate) fn cache_replacement(
        &mut self,
        complete: DecoderBuildComplete,
    ) -> Option<DecoderBuildComplete> {
        self.ready_replacement.replace(complete)
    }

    pub(crate) fn take_replacement(&mut self, build: BuildId) -> Option<DecoderBuildComplete> {
        if self
            .ready_replacement
            .as_ref()
            .is_some_and(|complete| complete.build == build)
        {
            self.ready_replacement.take()
        } else {
            None
        }
    }

    pub(crate) fn submit(&mut self) {
        let Some(job) = self.pending.take() else {
            return;
        };
        drop(spawn_blocking_on(&self.runtime, move || run(job)));
    }

    fn next_build(&mut self) -> BuildId {
        let build = BuildId::new(self.next_build);
        self.next_build = self.next_build.wrapping_add(1);
        build
    }

    #[cfg(test)]
    pub(crate) fn completion(&self) -> Arc<ArrayQueue<DecoderBuildComplete>> {
        self.deps.completion.clone()
    }

    #[cfg(test)]
    pub(crate) fn run_inline(&mut self) {
        if let Some(job) = self.pending.take() {
            run(job);
        }
    }

    #[cfg(test)]
    pub(crate) fn runtime(&self) -> &RuntimeHandle {
        &self.runtime
    }
}

fn run<T: StreamType>(job: PendingJob<T>) {
    let PendingJob {
        build,
        deps,
        input,
        media_info,
        offset,
        purpose,
        seek_epoch,
    } = job;
    let result = match catch_unwind(AssertUnwindSafe(|| {
        let reader = match input {
            PendingInput::Opened(reader) => reader,
            PendingInput::Shared { stream, offset } => stream.open_rebuild_reader(offset),
        };
        let decoder = (deps.factory)(reader, media_info.clone())?;
        Ok(DecoderGeneration::new(
            decoder,
            Some(media_info),
            offset,
            seek_epoch,
            deps.gapless_mode,
        ))
    })) {
        Ok(result) => result.map_err(|error| classify(&error)),
        Err(payload) => {
            warn!(
                build = build.get(),
                offset,
                panic = %panic_message(payload),
                "decoder factory panicked during rebuild; failing track"
            );
            Err(RecreateOutcome::SoftFailed)
        }
    };
    let complete = DecoderBuildComplete {
        build,
        purpose,
        result,
    };
    if let Err(complete) = deps.completion.push(complete) {
        let _ = deps.completion.pop();
        if deps.completion.push(complete).is_err() {
            warn!(
                build = build.get(),
                "decoder build completion queue overflowed"
            );
        }
    }
    deps.wake.wake();
}

fn panic_message(payload: Box<dyn std::any::Any + Send>) -> String {
    match payload.downcast::<String>() {
        Ok(message) => *message,
        Err(payload) => payload.downcast::<&'static str>().map_or_else(
            |_| "unknown panic payload".to_string(),
            |message| (*message).to_string(),
        ),
    }
}
