use kithara_events::{EngineEvent, EventSet, PlayerEvent, QueueEvent, SessionEvent};

#[derive(Clone, Debug, EventSet)]
#[non_exhaustive]
pub(crate) enum AnalysisEvent {
    Queue(QueueEvent),
    Player(PlayerEvent),
    Engine(EngineEvent),
    Session(SessionEvent),
}
