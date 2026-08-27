use bon::Builder;
use kithara_audio::{AudioConfig, ResamplerBackend};
use kithara_platform::sync::Arc;
use kithara_stream::StreamType;
use kithara_warp::WarpConfig;

use super::EngineLoad;
use crate::effects::AudioEffect;

/// Play-owned configuration for one resident Warp/audio producer lane.
#[derive(Builder, fieldwork::Fieldwork)]
#[builder(start_fn = for_audio)]
#[fieldwork(opt_in, get)]
#[non_exhaustive]
pub struct TrackConfig<T, B>
where
    T: StreamType,
    B: ResamplerBackend,
{
    /// Source-only decoder configuration.
    #[builder(start_fn)]
    #[field(get)]
    pub(crate) audio: AudioConfig<T, B>,
    /// Playback effects after the resident Warp stage.
    #[builder(default)]
    #[field(get)]
    pub(crate) effects: Vec<Box<dyn AudioEffect>>,
    /// Optional live cost meter for this play-owned producer lane.
    #[field(get)]
    pub(crate) engine_load: Option<Arc<EngineLoad>>,
    /// Resident Warp resources and live temporal controls.
    #[builder(default = WarpConfig::builder().build())]
    #[field(get)]
    pub(crate) warp: WarpConfig,
}

impl<T, B> From<AudioConfig<T, B>> for TrackConfig<T, B>
where
    T: StreamType,
    B: ResamplerBackend,
{
    fn from(audio: AudioConfig<T, B>) -> Self {
        Self::for_audio(audio).build()
    }
}
