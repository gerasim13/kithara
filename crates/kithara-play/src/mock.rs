use std::{
    num::NonZeroU32,
    sync::atomic::{AtomicU64, Ordering},
};

use kithara_audio::ConsumerWakeMode;
#[cfg(test)]
use kithara_beat::{
    BeatGridModel, BeatGridState as WireState, GridBeat, RawBeatGrid, SCHEMA_VERSION,
};
use kithara_platform::sync::{Arc, Mutex};
#[cfg(test)]
use kithara_signal::{SessionEpoch, SessionFrame};
#[cfg(test)]
use kithara_warp::{
    AssetAxis, AssetExtent, Beat, BeatAlignment, BeatGridId, BeatGridRevision, BeatGridSnapshot,
    MapPoint, SessionAnchor, SessionBeat, WarpMap, WarpMapRevision, WarpPlan,
};
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
                Reply::SlotAllocated(Box::new(AllocatedSlot::new(control, slot)))
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

/// A plan entering a 120 BPM recording one second into a session at the
/// same tempo, so the activation starts past the recording's first frame.
#[cfg(test)]
pub(crate) fn entering_plan() -> WarpPlan {
    let rate = NonZeroU32::new(48_000).expect("fixture rate");
    let model = BeatGridModel::try_from(RawBeatGrid {
        schema_version: SCHEMA_VERSION,
        model_id: "readiness".to_owned(),
        revision: 1,
        state: WireState::Final,
        duration: Some(10.0),
        bpm: 120.0,
        beats: [0.0, 0.5]
            .into_iter()
            .zip(0..)
            .map(|(at, ordinal)| GridBeat {
                at,
                ordinal,
                confidence: Some(1.0),
            })
            .collect(),
        downbeats: Vec::new(),
        meter: None,
    })
    .expect("fixture model");
    let asset = BeatGridSnapshot::model(
        BeatGridId::allocate().expect("asset identity"),
        BeatGridRevision::first(),
        &model,
        AssetAxis::new(rate, AssetExtent::Bounded(480_000)),
    )
    .expect("fixture asset grid");
    let session = BeatGridSnapshot::session(
        BeatGridId::allocate().expect("session identity"),
        BeatGridRevision::first(),
        SessionEpoch::new(0),
        SessionAnchor::new(SessionFrame::new(0), SessionBeat::default(), 2.0, rate)
            .expect("fixture tempo"),
        None,
    );
    let cue = Beat::new(0.0).expect("fixture cue");
    let alignment = BeatAlignment::new(
        MapPoint::new(asset.stamp(), cue),
        MapPoint::new(session.stamp(), cue),
    );
    let map = WarpMap::projected(asset, session, alignment, WarpMapRevision::first())
        .expect("fixture projection");
    WarpPlan::new(map, SessionFrame::new(48_000)).expect("fixture activation")
}
