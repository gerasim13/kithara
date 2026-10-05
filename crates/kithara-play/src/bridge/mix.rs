use kithara_config::Config;
use kithara_signal::FaderValue;

/// How loud a deck sounds, changed while it plays.
///
/// A change of one field goes to the deck as one [`DeckPart::Mix`](super::DeckPart::Mix); the
/// deck applies it on its frame and ramps its output gain to [`DeckMixSettings::gain`].
#[derive(Clone, Copy, Debug, PartialEq, Config)]
#[config(default, fields(value, get(copy)))]
pub struct DeckMixSettings {
    /// The deck's fader, at unity unless changed.
    #[config(live, builder(default = FaderValue::DEFAULT))]
    volume: FaderValue,
    /// Whether the deck is silent whatever its volume, unmuted unless changed.
    #[config(live, builder(default))]
    muted: bool,
}

impl DeckMixSettings {
    /// The amplitude the deck sounds at: silence when muted, else the square of the fader.
    #[must_use]
    pub fn gain(self) -> f32 {
        if self.muted {
            0.0
        } else {
            let volume = f32::from(self.volume);
            volume * volume
        }
    }
}
