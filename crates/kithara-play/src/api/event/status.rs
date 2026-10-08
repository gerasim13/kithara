#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum PlayerStatus {
    #[default]
    Unknown,
    ReadyToPlay,
    Failed,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum TimeControlStatus {
    #[default]
    Paused,
    WaitingToPlay,
    Playing,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum WaitingReason {
    ToMinimizeStalls,
    EvaluatingBufferingRate,
    NoItemToPlay,
    InterruptedBySession,
}
