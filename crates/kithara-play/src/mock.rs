use std::{
    num::NonZeroU32,
    sync::atomic::{AtomicU64, Ordering},
};

use kithara_audio::ConsumerWakeMode;
use kithara_platform::sync::{Arc, Mutex};
#[cfg(test)]
use ringbuf::traits::{Consumer, Producer};

pub use crate::api::equalizer::EqualizerMock;
use crate::{
    PlayError, SharedEq, SlotId, StreamShape,
    bridge::{NodeInputs, slot_channels},
    session::{AllocatedSlot, Cmd, Reply, SessionBinding, SessionDispatcher, SessionSampleRate},
};

/// Sample rate every `SessionMock` answers with.
pub const SAMPLE_RATE: NonZeroU32 = match NonZeroU32::new(44_100) {
    Some(sample_rate) => sample_rate,
    None => unreachable!(),
};

/// A session that registers players and hands out slots without a graph.
pub struct SessionMock {
    next_player: AtomicU64,
    next_slot: AtomicU64,
    nodes: Mutex<Vec<NodeInputs>>,
    sample_rate: NonZeroU32,
    shape: Option<StreamShape>,
}

impl<S> SessionDispatcher<S> for SessionMock {
    fn consumer_wake_mode(&self) -> ConsumerWakeMode {
        ConsumerWakeMode::RealtimeDeferred
    }

    fn exec(&self, cmd: Cmd<S>) -> Result<Reply, PlayError> {
        let reply = match cmd {
            Cmd::RegisterPlayer { .. } => {
                Reply::PlayerRegistered(crate::session::RegisteredPlayer {
                    id: self.next_player.fetch_add(1, Ordering::Relaxed),
                    eq: SharedEq::new(10),
                })
            }
            Cmd::AllocateSlot { .. } => {
                let slot = SlotId::new(self.next_slot.fetch_add(1, Ordering::Relaxed));
                let (inputs, control) = slot_channels(SharedEq::new(10));
                self.nodes.lock().push(inputs);
                Reply::SlotAllocated(AllocatedSlot::new(control, slot))
            }
            Cmd::QuerySampleRate => {
                Reply::SampleRate(SessionSampleRate::new(None, self.sample_rate.get()))
            }
            Cmd::QueryStreamShape => Reply::StreamShape(self.shape),
            _ => Reply::Ok,
        };
        Ok(reply)
    }
}

/// A binding to a fresh `SessionMock` with no stream shape.
#[must_use]
pub fn session<S>() -> SessionBinding<S> {
    session_with_shape(None)
}

/// A binding to a fresh `SessionMock` answering `shape` to stream-shape queries.
#[must_use]
pub fn session_with_shape<S>(shape: Option<StreamShape>) -> SessionBinding<S> {
    binding(shape, SAMPLE_RATE)
}

/// A binding to a fresh `SessionMock` running at `sample_rate` instead of `SAMPLE_RATE`.
#[must_use]
pub fn session_at<S>(sample_rate: NonZeroU32) -> SessionBinding<S> {
    binding(None, sample_rate)
}

/// A binding to a fresh `SessionMock`, together with that mock standing in
/// for the audio threads of the slots it allocates.
#[cfg(test)]
pub(crate) fn session_with_mock<S>() -> (SessionBinding<S>, Arc<SessionMock>) {
    let mock = Arc::new(SessionMock::new(None, SAMPLE_RATE));
    let dispatcher: Arc<dyn SessionDispatcher<S>> = Arc::clone(&mock) as _;
    (SessionBinding::new(dispatcher, SAMPLE_RATE), mock)
}

fn binding<S>(shape: Option<StreamShape>, sample_rate: NonZeroU32) -> SessionBinding<S> {
    SessionBinding::new(Arc::new(SessionMock::new(shape, sample_rate)), sample_rate)
}

impl SessionMock {
    fn new(shape: Option<StreamShape>, sample_rate: NonZeroU32) -> Self {
        Self {
            shape,
            sample_rate,
            next_player: AtomicU64::new(1),
            next_slot: AtomicU64::new(0),
            nodes: Mutex::default(),
        }
    }

    /// Answer as the audio thread of every allocated slot.
    #[cfg(test)]
    pub(crate) fn notify(&self, notification: &crate::bridge::PlayerNotification) {
        for node in self.nodes.lock().iter_mut() {
            assert!(
                node.notif_tx.try_push(notification.clone()).is_ok(),
                "fixture notification ring has room"
            );
        }
    }

    /// Everything the audio threads of the allocated slots were sent, in order.
    #[cfg(test)]
    pub(crate) fn take_commands(&self) -> Vec<crate::bridge::PlayerCmd> {
        let mut commands = Vec::new();
        for node in self.nodes.lock().iter_mut() {
            commands.extend(node.cmd_rx.pop_iter());
        }
        commands
    }
}
