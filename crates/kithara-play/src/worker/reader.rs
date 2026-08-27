use std::num::NonZeroU32;

use kithara_audio::{
    Audio, ChunkOutcome, PcmControl, PcmRead, PcmSession, PcmTaskId, PreloadGate, ReadOutcome,
    SeekBegin, SeekOutcome, ServiceClass,
};
use kithara_decode::{DecodeError, PcmSpec, TrackMetadata};
use kithara_events::EventBus;
use kithara_platform::{maybe_send::MaybeSend, sync::Arc, time::Duration};

use super::PlayWorker;

pub(crate) struct TrackLease {
    task_id: Option<PcmTaskId>,
    worker: PlayWorker,
}

impl TrackLease {
    pub(crate) const fn new(worker: PlayWorker, task_id: PcmTaskId) -> Self {
        Self {
            task_id: Some(task_id),
            worker,
        }
    }
}

impl Drop for TrackLease {
    fn drop(&mut self) {
        if let Some(task_id) = self.task_id.take() {
            self.worker.unregister(task_id);
        }
    }
}

/// Audio reader whose producer task is registered with a [`PlayWorker`].
///
/// The reader drops before its registration lease, so its wake handles and
/// buffers are released before the final worker owner can shut down.
pub struct RegisteredAudio<S> {
    audio: Audio<S>,
    _lease: TrackLease,
}

impl<S> RegisteredAudio<S> {
    pub(super) const fn new(audio: Audio<S>, lease: TrackLease) -> Self {
        Self {
            audio,
            _lease: lease,
        }
    }
}

impl<S: MaybeSend> PcmRead for RegisteredAudio<S> {
    delegate::delegate! {
        to self.audio {
            fn cached_span(&self) -> Duration;
            fn decoded_frontier(&self) -> Duration;
            fn next_chunk(&mut self) -> Result<ChunkOutcome, DecodeError>;
            fn position(&self) -> Duration;
            fn read(&mut self, buf: &mut [f32]) -> Result<ReadOutcome, DecodeError>;
            fn read_planar<'a>(
                &mut self,
                output: &'a mut [&'a mut [f32]],
            ) -> Result<ReadOutcome, DecodeError>;
            fn spec(&self) -> PcmSpec;
        }
    }
}

impl<S: MaybeSend> PcmSession for RegisteredAudio<S> {
    delegate::delegate! {
        to self.audio {
            fn abr_handle(&self) -> Option<kithara_abr::AbrHandle>;
            fn duration(&self) -> Option<Duration>;
            fn event_bus(&self) -> &EventBus;
            fn is_preloaded(&self) -> bool;
            fn metadata(&self) -> &TrackMetadata;
            fn preload_epoch(&self) -> u64;
            fn preload_gate(&self) -> Option<Arc<PreloadGate>>;
        }
    }
}

impl<S: MaybeSend> PcmControl for RegisteredAudio<S> {
    delegate::delegate! {
        to self.audio {
            fn preload(&mut self) -> Result<(), DecodeError>;
            fn seek(&mut self, position: Duration) -> Result<SeekOutcome, DecodeError>;
            fn sync_seek(&mut self);
            fn set_host_sample_rate(&self, sample_rate: NonZeroU32);
            fn set_playback_rate(&self, rate: f32);
            fn set_service_class(&self, class: ServiceClass);
        }
    }

    fn seek_handle(&self) -> Option<Arc<dyn SeekBegin>> {
        PcmControl::seek_handle(&self.audio)
    }
}
