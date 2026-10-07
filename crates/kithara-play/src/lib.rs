#![forbid(unsafe_code)]
#![cfg_attr(all(), allow(clippy::missing_errors_doc))]
#![cfg_attr(all(rtsan, not(rtsan_standalone)), feature(sanitize))]

mod error;
mod guard;
#[cfg(test)]
pub(crate) use kithara_test_utils::bufpool as test_pools;

pub mod api;
pub mod engine;
pub mod player;
pub mod policy;
pub mod resource;
pub mod session;
pub mod worker;

#[cfg(target_arch = "wasm32")]
pub mod wasm;

#[cfg(any(test, feature = "mock"))]
pub mod mock;

pub use api::{
    BpmInfo, DjEvent, EngineEvent, Equalizer, InterruptionKind, ItemRole, ItemStatus, MediaTime,
    PlaybackDirection, PlayerEvent, PlayerStatus, PortDescription, PortType, RouteChangeReason,
    RouteDescription, SelectionPlayback, SessionBeat, SessionDuckingMode, SessionEvent,
    SessionTransportSnapshot, SlotId, StretchBackendKind, SuccessorLink, SyncUnavailable, Tempo,
    TempoError, TimeControlStatus, TimeRange, TrackBinding, TrackRef, TransportRevision,
    WaitingReason,
};
pub use engine::{EngineConfig, EngineImpl};
pub use error::PlayError;
use humantime_serde as _;
pub use kithara_assets::{AssetLayout, DefaultLayout};
pub use kithara_audio::SeekOutcome;
pub use kithara_effects::eq::EqBandConfig;
pub use kithara_net::Headers;
pub use kithara_render::{
    CrossfadeCurve, CrossfadeSettings, InvalidCrossfade, ServiceClass,
    bridge::{
        MixTapWriter, NodeInputs, PlaybackFault, PlaybackShared, PlaybackSnapshot,
        PlayerNotification, RtMetricsSnapshot, SlotControl, TrackPlaybackStopReason, TrackState,
        TrackTransition,
    },
    rt::{BufferGeometryError, DeckMixerConfig, PlayerNode, StreamShape},
};
pub use kithara_warp::{BeatGrid, BeatGridId, BeatGridSnapshot, MIN_SPEED};
pub use player::{
    DEFAULT_CROSSFADE_DURATION, DEFAULT_PLAYING_RATE, PlayerConfig, PlayerConfigPatch, PlayerImpl,
    SelectTransition,
};
pub use resource::{
    ArtifactDocument, ArtifactFetch, ArtifactLoadError, ArtifactSource, Cover, MAX_ARTIFACT_BYTES,
    PlaybackResamplerBackend, Resource, ResourceConfig, ResourceSrc, SourceType,
};
pub use session::{
    AllocatedSlot, DeckRegistration, OutputSnapshot, PlayerId, SessionBinding, SessionError,
    SessionOutputView, SessionSampleRate,
};
pub use worker::{
    EngineLoad, EngineLoadSnapshot, LoadRefusal, PlayWorker, PlayWorkerConfig,
    PlayWorkerConfigPatch, RegisteredAudio, TrackConfig,
};
mod consts;
