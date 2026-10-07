use kithara_signal::SessionFrame;

/// A refused tempo step or synchronization configuration.
#[derive(Clone, Debug, PartialEq, thiserror::Error)]
pub enum LinkError {
    /// A new step must come after the trajectory's initial anchor.
    #[error("tempo step at {frame:?} is not after the initial anchor")]
    BeforeAnchor { frame: SessionFrame },
    /// Each frame names at most one tempo change in flight.
    #[error("a tempo step already occupies {frame:?}")]
    Occupied { frame: SessionFrame },
    /// Correction steps require a finite, positive speed limit.
    #[error("correction epsilon must be finite and positive, got {epsilon}")]
    Epsilon { epsilon: f32 },
}
