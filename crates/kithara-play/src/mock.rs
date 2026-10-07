use std::{
    num::NonZeroU32,
    sync::atomic::{AtomicU64, Ordering},
};

use kithara_audio::ConsumerWakeMode;
use kithara_platform::sync::{Arc, Mutex};
use kithara_render::bridge::{NodeInputs, slot_channels};

pub use crate::api::equalizer::EqualizerMock;
use crate::{
    PlayError, SlotId, StreamShape,
    session::{AllocatedSlot, Cmd, Reply, SessionBinding, SessionDispatcher, SessionSampleRate},
};

/// Sample rate every `SessionMock` answers with.
pub const SAMPLE_RATE: NonZeroU32 = match NonZeroU32::new(44_100) {
    Some(sample_rate) => sample_rate,
    None => unreachable!(),
};

/// A session that registers and starts players, building each deck's slot without a graph.
pub struct SessionMock {
    asked: Mutex<Vec<&'static str>>,
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
        self.asked.lock().push(asked(&cmd));
        let reply = match cmd {
            Cmd::RegisterPlayer { .. } => {
                Reply::PlayerRegistered(self.next_player.fetch_add(1, Ordering::Relaxed))
            }
            Cmd::StartPlayer { .. } => {
                let slot = SlotId::new(self.next_slot.fetch_add(1, Ordering::Relaxed));
                let (inputs, control) = slot_channels();
                self.nodes.lock().push(inputs);
                Reply::PlayerStarted(Box::new(AllocatedSlot::new(control, slot)))
            }
            _ => Reply::Ok,
        };
        Ok(reply)
    }

    fn sample_rate(&self) -> SessionSampleRate {
        SessionSampleRate::new(None, self.sample_rate.get())
    }

    fn stream_shape(&self) -> Option<StreamShape> {
        self.shape
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
/// for the audio threads of the slots it builds.
#[cfg(test)]
pub(crate) fn session_with_mock<S>() -> (SessionBinding<S>, Arc<SessionMock>) {
    let mock = Arc::new(SessionMock::new(None, SAMPLE_RATE));
    let dispatcher: Arc<dyn SessionDispatcher<S>> = Arc::clone(&mock) as _;
    (SessionBinding::new(dispatcher, SAMPLE_RATE), mock)
}

/// What the session was asked, by command.
const fn asked<S>(cmd: &Cmd<S>) -> &'static str {
    match cmd {
        Cmd::RegisterPlayer { .. } => "register",
        Cmd::UnregisterPlayer { .. } => "unregister",
        Cmd::StartPlayer { .. } => "start",
        Cmd::StopPlayer { .. } => "stop",
        Cmd::Tick => "tick",
    }
}

fn binding<S>(shape: Option<StreamShape>, sample_rate: NonZeroU32) -> SessionBinding<S> {
    SessionBinding::new(Arc::new(SessionMock::new(shape, sample_rate)), sample_rate)
}

impl SessionMock {
    fn new(shape: Option<StreamShape>, sample_rate: NonZeroU32) -> Self {
        Self {
            asked: Mutex::default(),
            shape,
            sample_rate,
            next_player: AtomicU64::new(1),
            next_slot: AtomicU64::new(0),
            nodes: Mutex::default(),
        }
    }

    /// Every command the session was asked, in order.
    #[cfg(test)]
    pub(crate) fn asked(&self) -> Vec<&'static str> {
        self.asked.lock().clone()
    }

    /// Answer as the audio thread of every slot it built.
    #[cfg(test)]
    pub(crate) fn notify(&self, notification: &kithara_render::bridge::PlayerNotification) {
        for node in self.nodes.lock().iter_mut() {
            assert!(
                kithara_render::mock::notify(node, notification.clone()).is_ok(),
                "fixture notification ring has room"
            );
        }
    }

    /// Everything the audio threads of the slots it built were sent, in order.
    #[cfg(test)]
    pub(crate) fn take_commands(&self) -> Vec<kithara_render::bridge::DeckPart> {
        self.nodes
            .lock()
            .iter_mut()
            .flat_map(kithara_render::mock::take_batches)
            .flatten()
            .collect()
    }
}
