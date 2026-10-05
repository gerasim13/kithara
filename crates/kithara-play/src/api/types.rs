use firewheel::Volume;
pub use kithara_events::{SlotId, TrackId};

/// How far the whole session output is lowered under a competing sound.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum SessionDuckingMode {
    #[default]
    Off,
    Soft,
    Hard,
}

impl SessionDuckingMode {
    /// The fader volume this ducking policy sets the session output to.
    #[must_use]
    pub const fn volume(self) -> Volume {
        match self {
            Self::Off => Volume::UNITY_GAIN,
            Self::Soft => Volume::Linear(0.4),
            Self::Hard => Volume::Linear(0.2),
        }
    }
}
