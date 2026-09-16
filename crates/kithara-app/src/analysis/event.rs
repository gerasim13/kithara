use kithara::{
    play::{EngineEvent, PlayerEvent, SessionEvent},
    queue::QueueEvent,
};
use kithara_events::EventSet;

#[derive(Clone, Debug, EventSet)]
#[non_exhaustive]
pub(crate) enum AnalysisEvent {
    Queue(QueueEvent),
    Player(PlayerEvent),
    Engine(EngineEvent),
    Session(SessionEvent),
}
