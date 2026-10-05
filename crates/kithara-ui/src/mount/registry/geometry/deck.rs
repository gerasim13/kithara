use crate::{expand::Binding, ids::InternId, module::{DeckSummaryStyle, WaveStyle}, mount::{Bpm, Summary, Wave}};

pub(crate) fn summary(
    _style: DeckSummaryStyle
) -> Summary {
    Summary::builder().build()
}

pub(crate) fn bpm(
    _placeholder: Option<InternId>
) -> Bpm {
    Bpm::builder().build()
}

pub(crate) fn wave<'a>(
    style: WaveStyle, _badge: Option<InternId>, _zoom: Option<&'a Binding>
) -> Wave {
    Wave::builder().style(style).build()
}
