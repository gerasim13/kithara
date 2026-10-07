use std::{
    num::NonZeroU32,
    sync::atomic::{AtomicU64, Ordering},
};

use kithara_audio::ConsumerWakeMode;
use kithara_platform::sync::{Arc, Mutex};
use kithara_render::bridge::{NodeInputs, slot_channels};

pub use crate::api::equalizer::EqualizerMock;
use crate::{
    SlotId, StreamShape,
    player::PlayerControlSource,
    session::{AllocatedSlot, SessionBinding, SessionOutputView},
};

/// Sample rate every mock session runs at.
pub const SAMPLE_RATE: NonZeroU32 = match NonZeroU32::new(44_100) {
    Some(sample_rate) => sample_rate,
    None => unreachable!(),
};

/// A session that builds each deck's slot without a graph, as a Host's insert would.
#[derive(Default)]
pub struct SessionMock {
    next_slot: AtomicU64,
    nodes: Mutex<Vec<NodeInputs>>,
}

/// Binds `player` to a fresh mock session and seats it on a deck slot that
/// mock built, as a Host's insert does.
///
/// # Panics
/// Panics when `player` is already bound to a session.
pub fn insert<P: PlayerControlSource>(player: &mut P) -> Arc<SessionMock> {
    let mock = Arc::new(SessionMock::default());
    player
        .attach_session(session())
        .expect("a fresh player binds to the mock session");
    mock.seat(player);
    mock
}

/// A binding to a fresh mock session with no stream shape.
#[must_use]
pub fn session() -> SessionBinding {
    session_with_shape(None)
}

/// A binding to a fresh mock session that publishes `shape` as its stream shape.
#[must_use]
pub fn session_with_shape(shape: Option<StreamShape>) -> SessionBinding {
    binding(shape, SAMPLE_RATE)
}

/// A binding to a fresh mock session running at `sample_rate` instead of `SAMPLE_RATE`.
#[must_use]
pub fn session_at(sample_rate: NonZeroU32) -> SessionBinding {
    binding(None, sample_rate)
}

fn binding(shape: Option<StreamShape>, sample_rate: NonZeroU32) -> SessionBinding {
    let output = SessionOutputView::new(sample_rate);
    output.publish(output.sample_rate(), shape);
    SessionBinding::new(output, ConsumerWakeMode::RealtimeDeferred, sample_rate)
}

impl SessionMock {
    /// Builds a deck slot and seats `player` on it, as a Host's insert does.
    pub fn seat<P: PlayerControlSource>(&self, player: &mut P) {
        let slot = SlotId::new(self.next_slot.fetch_add(1, Ordering::Relaxed));
        let (inputs, control) = slot_channels();
        self.nodes.lock().push(inputs);
        player.seat(AllocatedSlot::new(control, slot));
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
