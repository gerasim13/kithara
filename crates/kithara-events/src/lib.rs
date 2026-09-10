#![forbid(unsafe_code)]

//! Unified event bus for the kithara audio pipeline.

extern crate self as kithara_events;

mod bus;
mod bus_event;
mod deferred;
mod event;
mod ids;
mod meta;
mod receiver;
mod scope;
mod seek;
mod topic;

#[cfg(feature = "audio")]
mod audio;
#[cfg(feature = "decoder")]
mod decoder;
#[cfg(feature = "drm")]
mod drm;
#[cfg(feature = "hls")]
mod hls;
#[cfg(feature = "player")]
mod play;
#[cfg(feature = "queue")]
mod queue;

#[cfg(feature = "audio")]
pub use audio::{
    AudioEvent, PlaybackResamplerKind, SeekLifecycleStage, SegmentLocation, TrackFailureKind,
};
pub use bus::{DEFAULT_EVENT_BUS_CAPACITY, EventBus};
pub use bus_event::BusEvent;
#[cfg(feature = "decoder")]
pub use decoder::{
    AudioCodecKind, ContainerKind, DecodeErrorClass, DecodeErrorKind, DecoderBackend,
    DecoderChangeCause, DecoderEvent, FrameDomain, GaplessSpan, ResamplerKind,
};
pub use deferred::DeferredBus;
#[cfg(feature = "drm")]
pub use drm::{DrmEvent, KeyFailureStage, KeySource};
pub use event::{Event, EventSet};
#[cfg(feature = "hls")]
pub use hls::{HlsError, HlsEvent};
pub use ids::{SlotId, TrackId};
pub use kithara_derive::{Event, EventSet};
pub use kithara_platform::tokio::{
    select,
    sync::broadcast::error::{RecvError, TryRecvError},
};
pub use meta::{Envelope, EventMeta, ScopeLabel};
#[cfg(feature = "player")]
pub use play::{
    BpmInfo, DjEvent, EngineEvent, InterruptionKind, ItemEvent, ItemRole, ItemStatus, MediaTime,
    PlaybackDirection, PlayerEvent, PlayerStatus, PortDescription, PortType, RouteChangeReason,
    RouteDescription, SessionEvent, StretchBackendKind, TimeControlStatus, TimeRange, TrackRef,
    TransportEvent, WaitingReason,
};
#[cfg(feature = "queue")]
pub use queue::{AdvanceReason, QueueEvent, QueueRepeatMode, TrackStatus};
pub use receiver::{EventReceiver, TopicReceiver};
pub use scope::BusScope;
pub use seek::SeekEpoch;
