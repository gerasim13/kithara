use kithara_signal::SessionEpoch;
use kithara_warp::{BeatGridStamp, MeterFacts, SessionAnchor, SessionAxis};

/// A parent's accepted tempo and phase segment, offered to one direct child.
///
/// Only a child in [`crate::SyncMode::HostSync`] adopts it; a child in any
/// other mode records it so a later enable follows the parent's current
/// segment rather than one it never saw.
#[derive(Clone, Copy, Debug, PartialEq, fieldwork::Fieldwork)]
#[fieldwork(opt_in, get)]
#[non_exhaustive]
pub struct ParentGridUpdate {
    /// Returns the parent grid identity and revision this segment belongs to.
    #[field(get, copy)]
    parent: BeatGridStamp,
    /// Returns the session-axis generation the anchor's frames are on.
    #[field(get, copy)]
    epoch: SessionEpoch,
    /// Returns the parent's tempo trajectory from its commit frame onwards.
    #[field(get, copy)]
    anchor: SessionAnchor,
    /// Returns the parent's meter evidence, when it has any.
    #[field(get, copy)]
    meter: Option<MeterFacts>,
}

impl ParentGridUpdate {
    /// Describes one parent segment on its exact session axis.
    #[must_use]
    pub const fn new(
        parent: BeatGridStamp,
        epoch: SessionEpoch,
        anchor: SessionAnchor,
        meter: Option<MeterFacts>,
    ) -> Self {
        Self {
            parent,
            epoch,
            anchor,
            meter,
        }
    }

    /// Returns the session axis the segment's frames are measured on.
    #[must_use]
    pub fn axis(self) -> SessionAxis {
        SessionAxis::new(self.anchor.sample_rate(), self.epoch)
    }
}

/// The physical session axis changed: a new epoch, possibly at a new rate.
///
/// Every mode accepts it, because every frame planned on the previous axis is
/// meaningless on the new one.
#[derive(Clone, Copy, Debug, Eq, PartialEq, fieldwork::Fieldwork)]
#[fieldwork(opt_in, get)]
#[non_exhaustive]
pub struct SessionAxisUpdate {
    /// Returns the axis every later frame is measured on.
    #[field(get, copy)]
    axis: SessionAxis,
}

impl SessionAxisUpdate {
    /// Announces the session axis that replaces the current one.
    #[must_use]
    pub const fn new(axis: SessionAxis) -> Self {
        Self { axis }
    }
}
