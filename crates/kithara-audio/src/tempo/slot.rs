use kithara_platform::sync::Arc;

use crate::{
    musical::{SessionBeat, SourceSchedule},
    tempo::streaming::StretchControls,
};

/// Returns whether the selected exact-span engine accepts this continuous
/// source-frame advance, or `None` when this build has no such engine.
#[must_use]
pub fn bound_rate_supported(source_frames_per_output: f64) -> Option<bool> {
    #[cfg(all(
        not(target_arch = "wasm32"),
        any(feature = "stretch-signalsmith", feature = "stretch-bungee")
    ))]
    {
        super::bound::rate_supported(source_frames_per_output)
    }
    #[cfg(not(all(
        not(target_arch = "wasm32"),
        any(feature = "stretch-signalsmith", feature = "stretch-bungee")
    )))]
    {
        let _ = source_frames_per_output;
        None
    }
}

/// Output frames in one exact-span planning request for the selected engine.
#[must_use]
pub fn bound_render_span_frames() -> Option<u64> {
    #[cfg(all(
        not(target_arch = "wasm32"),
        any(feature = "stretch-signalsmith", feature = "stretch-bungee")
    ))]
    {
        Some(super::bound::render_span_frames())
    }
    #[cfg(not(all(
        not(target_arch = "wasm32"),
        any(feature = "stretch-signalsmith", feature = "stretch-bungee")
    )))]
    {
        None
    }
}

/// A tempo slot cannot be built as configured.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
#[non_exhaustive]
pub enum TempoSlotError {
    /// A deck was bound to the session grid on a build with no exact-span
    /// engine. Rendering it through the streaming slot would silently place its
    /// beats somewhere else, so the binding is refused instead.
    #[error("binding a deck to the session grid needs a compiled exact-span engine")]
    BoundEngineMissing,
}

/// What drives one deck's tempo slot.
///
/// The two arms are the two ways a deck can be timed and a deck is on exactly
/// one of them: unbound decks follow live controls and take whatever span the
/// streaming backend renders, bound decks follow the session grid and get an
/// exact span per block. Making that a choice rather than two independent
/// options is what keeps a deck from ending up on both.
#[derive(Clone)]
#[non_exhaustive]
pub enum TempoSlot {
    /// Live speed, key-lock and region plan, output span chosen by the backend.
    Streaming(Arc<StretchControls>),
    /// Bound at a fixed transport revision: the schedule names the source span
    /// due after advancing from the supplied session-beat origin.
    Bound(Arc<SourceSchedule>, SessionBeat),
}

impl From<Arc<StretchControls>> for TempoSlot {
    fn from(controls: Arc<StretchControls>) -> Self {
        Self::Streaming(controls)
    }
}

impl TempoSlot {
    /// Returns the live controls when this deck is unbound.
    #[must_use]
    pub fn streaming(&self) -> Option<&Arc<StretchControls>> {
        match self {
            Self::Streaming(controls) => Some(controls),
            Self::Bound(_, _) => None,
        }
    }

    /// Returns the schedule when this deck is bound to the session grid.
    #[must_use]
    pub fn bound(&self) -> Option<&Arc<SourceSchedule>> {
        match self {
            Self::Bound(schedule, _) => Some(schedule),
            Self::Streaming(_) => None,
        }
    }
}
