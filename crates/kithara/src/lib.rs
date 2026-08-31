#![forbid(unsafe_code)]

//! # Kithara
//!
//! Facade crate providing a unified API for audio streaming and decoding.
//!
//! ## Quick start
//!
//! ```ignore
//! use kithara::{
//!     assets::AssetStore,
//!     bufpool::{OverallBudget, PoolConfig, pool_schema},
//!     prelude::*,
//! };
//!
//! pool_schema! {
//!     AppPools {
//!         bytes: u8,
//!         samples: f32,
//!     }
//! }
//! let pool_config = || PoolConfig::builder().max_buffers(128).build();
//! let pools = AppPools::builder(OverallBudget(64 * 1024 * 1024))
//!     .bytes(pool_config())
//!     .samples(pool_config())
//!     .build()?;
//! let worker = PlayWorker::new(PlayWorkerConfig::builder(pools.clone()).build());
//! let config: ResourceConfig<AppPools> =
//!     ResourceConfig::for_src(ResourceSrc::parse("https://example.com/song.mp3")?)
//!         .store(AssetStore::builder(pools).build())
//!         .worker(worker)
//!         .build();
//! let mut resource = Resource::new(config).await?;
//!
//! // Read interleaved PCM
//! let mut buf = [0.0f32; 1024];
//! resource.read(&mut buf);
//! ```

pub mod audio {
    pub use kithara_audio::*;
}

#[cfg(feature = "analysis")]
pub mod analysis {
    pub use kithara_analysis::*;
}

#[cfg(feature = "broadcast")]
pub mod broadcast {
    pub use kithara_broadcast::*;
}

pub mod bufpool {
    pub use kithara_bufpool::*;
}

pub mod decode {
    pub use kithara_decode::*;
}

#[cfg(feature = "encode")]
pub mod encode {
    pub use kithara_encode::*;
}

pub mod events {
    pub use kithara_events::*;
}

pub mod host {
    pub use kithara_host::*;
}

pub mod platform {
    pub use kithara_platform::*;
}

pub mod play {
    pub use kithara_play::*;
}

pub mod resampler {
    pub use kithara_resampler::*;
}

pub mod signal {
    pub use kithara_signal::*;
}

#[cfg(feature = "queue")]
pub mod queue {
    pub use kithara_queue::*;
}

pub mod stream {
    pub use kithara_stream::*;
}

#[cfg(any(feature = "stretch-signalsmith", feature = "stretch-bungee"))]
pub mod stretch {
    pub use kithara_stretch::*;
}

#[cfg(feature = "ui")]
pub mod ui {
    pub use kithara_ui::*;
}

pub mod warp {
    pub use kithara_warp::*;
}

#[cfg(feature = "worker")]
pub mod worker {
    pub use kithara_worker::*;
}

#[cfg(feature = "file")]
pub mod file {
    pub use kithara_file::*;
}

#[cfg(feature = "hls")]
pub mod abr {
    pub use kithara_abr::*;
}

#[cfg(feature = "hls")]
pub mod drm {
    pub use kithara_drm::*;
}

#[cfg(feature = "hls")]
pub mod hls {
    pub use kithara_hls::*;
}

#[cfg(any(feature = "file", feature = "hls", feature = "assets"))]
pub mod assets {
    pub use kithara_assets::*;
}

#[cfg(any(feature = "file", feature = "hls", feature = "net"))]
pub mod net {
    pub use kithara_net::*;
}

#[cfg(feature = "assets")]
pub mod storage {
    pub use kithara_storage::*;
}

pub use kithara_test_utils::{kithara::mock, no_block};
#[cfg(feature = "probe")]
pub use kithara_test_utils::{
    kithara::{fixture, test},
    kithara_facade::{allow_block, flash, no_block},
};
#[cfg(all(
    not(target_arch = "wasm32"),
    any(feature = "stretch-signalsmith", feature = "stretch-bungee")
))]
pub use kithara_warp::StretchKind;
pub use kithara_warp::{GridSegment, RegionPlan, RegionPlanError, StretchControls};

#[cfg(feature = "mock")]
pub mod mock {
    pub use kithara_audio::mock::*;
    pub use kithara_decode::mock::*;
    pub use kithara_play::mock::*;
    pub use kithara_stream::mock::*;
}

/// Prelude — flat imports for common types.
pub mod prelude {
    #[cfg(feature = "hls")]
    pub use kithara_abr::AbrMode;
    pub use kithara_audio::{
        Audio, AudioConfig, AudioControl, AudioRead, AudioReader, AudioSession, ResamplerQuality,
    };
    pub use kithara_decode::{DecodeError, DecodeResult, DecoderTrackInfo, TrackMetadata};
    #[cfg(feature = "hls")]
    pub use kithara_events::HlsEvent;
    pub use kithara_events::{AudioEvent, BusScope, Event, EventBus, EventReceiver, FileEvent};
    #[cfg(feature = "file")]
    pub use kithara_file::{File, FileConfig};
    #[cfg(feature = "hls")]
    pub use kithara_hls::{Hls, HlsConfig};
    pub use kithara_play::{
        EngineConfig, EngineImpl, EngineLoadSnapshot, PlayWorker, PlayWorkerConfig,
        PlaybackResamplerBackend, PlayerConfig, PlayerImpl, Resource, ResourceConfig, ResourceSrc,
        ServiceClass, SourceType,
    };
    pub use kithara_signal::{AudioChunkInfo, AudioSpec};
    pub use kithara_stream::{AudioCodec, ContainerFormat, MediaInfo, Stream, StreamType};
    #[cfg(all(
        not(target_arch = "wasm32"),
        any(feature = "stretch-signalsmith", feature = "stretch-bungee")
    ))]
    pub use kithara_warp::StretchKind;
    pub use kithara_warp::{GridSegment, RegionPlan, RegionPlanError, StretchControls};
}
