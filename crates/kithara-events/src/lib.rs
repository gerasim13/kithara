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
mod topic;

#[cfg(feature = "queue")]
mod queue;

pub use bus::{DEFAULT_EVENT_BUS_CAPACITY, EventBus};
pub use bus_event::BusEvent;
pub use deferred::DeferredBus;
pub use event::{Event, EventSet};
pub use ids::{SlotId, TrackId};
pub use kithara_derive::{Event, EventSet};
pub use kithara_platform::tokio::{
    select,
    sync::broadcast::error::{RecvError, TryRecvError},
};
pub use meta::{Envelope, EventMeta, ScopeLabel};
#[cfg(feature = "queue")]
pub use queue::{AdvanceReason, QueueEvent, QueueRepeatMode, TrackStatus};
pub use receiver::{EventReceiver, TopicReceiver};
pub use scope::BusScope;
