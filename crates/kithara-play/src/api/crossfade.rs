use crate::CrossfadeSettings;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Deserialize)]
#[non_exhaustive]
pub enum SelectionPlayback {
    #[default]
    Play,
    Pause,
}

/// How an armed successor joins the item it follows, fixed when it is armed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SuccessorLink {
    /// Chained behind its predecessor: the deck starts it on the frame the
    /// predecessor runs out.
    Gapless,
    /// Faded in over its predecessor when the transition commits.
    Fade,
}

/// A transition with no fade duration is gapless.
impl From<CrossfadeSettings> for SuccessorLink {
    fn from(settings: CrossfadeSettings) -> Self {
        if settings.duration > 0.0 {
            Self::Fade
        } else {
            Self::Gapless
        }
    }
}
