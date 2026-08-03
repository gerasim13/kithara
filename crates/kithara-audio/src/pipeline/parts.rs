use std::sync::atomic::AtomicU64;

use kithara_platform::sync::Arc;
use kithara_stream::{
    Activity, PlayheadWrite, SeekControl, SeekObserve, StreamType, VariantControl,
};

use crate::pipeline::{
    decode::{
        core::{ActiveDecode, DecodeParts},
        gate::ReadinessGate,
        resume::ResumeCursor,
    },
    rebuild::port::{RebuildPort, RebuildRuntime},
    seek::SeekEngine,
    stream::shared::SharedStream,
};

pub(crate) struct SourceParts<T: StreamType> {
    pub(crate) playback_resampler_backend: &'static str,
    pub(crate) decode: ActiveDecode,
    pub(crate) activity: Arc<dyn Activity>,
    pub(crate) playhead: Arc<dyn PlayheadWrite>,
    pub(crate) seek: Arc<dyn SeekControl>,
    pub(crate) seek_obs: Arc<dyn SeekObserve>,
    pub(crate) decoder_backend: kithara_decode::DecoderBackend,
    pub(crate) variant_control: Option<Arc<dyn VariantControl>>,
    pub(crate) readiness: ReadinessGate,
    pub(crate) rebuild: RebuildPort<T>,
    pub(crate) resume: ResumeCursor,
    pub(crate) seek_engine: SeekEngine,
}

impl<T: StreamType> SourceParts<T> {
    pub(crate) fn new(
        stream: &SharedStream<T>,
        decode: DecodeParts,
        epoch: Arc<AtomicU64>,
        rebuild: RebuildRuntime,
        variant_control: Option<Arc<dyn VariantControl>>,
    ) -> Self {
        let DecodeParts {
            active,
            factory,
            host_sample_rate,
            recreate_on_host_rate_change,
            decoder_host_sample_rate,
            decoder_backend,
            playback_resampler_backend,
        } = decode;
        let gapless_mode = active.gapless_mode();
        Self {
            decoder_backend,
            playback_resampler_backend,
            variant_control,
            activity: stream.activity(),
            decode: active,
            playhead: stream.playhead_write(),
            readiness: ReadinessGate::new(stream.peer_wake()),
            rebuild: RebuildPort::new(factory, gapless_mode, rebuild),
            resume: ResumeCursor::new(
                host_sample_rate,
                recreate_on_host_rate_change,
                decoder_host_sample_rate,
            ),
            seek: stream.seek_control(),
            seek_engine: SeekEngine::new(epoch),
            seek_obs: stream.seek_observe(),
        }
    }
}
