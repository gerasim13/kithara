use kithara::effects::GainDb;

use crate::{
    deck::{EqMode, TempoPercent},
    engine::{DeckCmd, DeckSnapshot, TempoChange},
};

pub(crate) mod consts {
    use super::TempoPercent;

    /// Tempo travel either way, in percent: tempo spans `-TEMPO_RANGE` to
    /// `+TEMPO_RANGE`.
    pub(crate) const TEMPO_RANGE: f32 = TempoPercent::MAX.0;
}

/// Everything a single deck can be told to do. Carries no deck identity: the
/// composer that renders a deck maps this into `Message::Deck(id, msg)`.
#[derive(Debug, Clone)]
pub(crate) enum DeckMsg {
    TogglePlayPause,
    Next,
    Prev,
    SeekTo(f64),
    EqBandChanged(usize, GainDb),
    DeleteTrack,
    Tempo(TempoChange),
    SetQuality(Option<usize>),
}

pub(crate) fn command(shown: &DeckSnapshot, eq_mode: EqMode, msg: &DeckMsg) -> Option<DeckCmd> {
    Some(match *msg {
        DeckMsg::TogglePlayPause if shown.playing => DeckCmd::Pause,
        DeckMsg::TogglePlayPause => DeckCmd::Play,
        DeckMsg::Next => DeckCmd::Next,
        DeckMsg::Prev => DeckCmd::Prev,
        DeckMsg::SeekTo(fraction) => DeckCmd::SeekFraction(fraction),
        DeckMsg::EqBandChanged(band, gain) => DeckCmd::SetEqGain {
            band,
            gain,
            layout: eq_mode,
        },
        DeckMsg::DeleteTrack => {
            let current = shown.current_track_index?;
            DeckCmd::RemoveTrack(shown.tracks.get(current)?.id)
        }
        DeckMsg::Tempo(change) => DeckCmd::Tempo(change),
        DeckMsg::SetQuality(variant) => DeckCmd::SetQuality(variant),
    })
}

#[cfg(test)]
mod tests {
    use kithara_test_utils::kithara;

    use super::{TempoPercent, consts::TEMPO_RANGE};

    #[kithara::test]
    fn the_whole_travel_is_within_reach_of_a_few_detents() {
        const REACH: f32 = 40.0;

        let detents = TEMPO_RANGE / TempoPercent::STEP;
        assert!(
            detents <= REACH,
            "one end of the travel takes {detents} detents"
        );
    }
}
