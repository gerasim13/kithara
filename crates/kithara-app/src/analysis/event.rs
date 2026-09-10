use kithara::play::{EngineEvent, PlayerEvent, SessionEvent};
use kithara_events::{EventSet, QueueEvent};

#[derive(Clone, Debug, EventSet)]
#[non_exhaustive]
pub(crate) enum AnalysisEvent {
    Queue(QueueEvent),
    Player(PlayerEvent),
    Engine(EngineEvent),
    Session(SessionEvent),
}
