use bon::Builder;

use crate::{module::DeckSummaryStyle, mount::Control, size::SizeSpec, skin::SkinDoc};

/// The deck's headline: what is loaded and how it is playing.
#[derive(Builder)]
pub(crate) struct Summary {
    pub(crate) style: DeckSummaryStyle,
}

impl Control for Summary {
    fn size(&self, skin: &SkinDoc) -> SizeSpec {
        skin.deck.summary_size
    }
}
