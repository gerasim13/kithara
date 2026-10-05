use crate::{
    expand::Binding,
    ids::InternId,
    module::{DeckSummaryStyle, WaveStyle},
    mount::{Bpm, Summary, Wave},
};

pub(crate) fn summary(style: DeckSummaryStyle) -> Summary {
    Summary::builder().style(style).build()
}

pub(crate) fn bpm(placeholder: Option<InternId>) -> Bpm {
    Bpm::builder().maybe_placeholder(placeholder).build()
}

pub(crate) fn wave<'a>(
    style: WaveStyle,
    badge: Option<InternId>,
    zoom: Option<&'a Binding>,
) -> Wave<'a> {
    Wave::builder()
        .style(style)
        .maybe_badge(badge)
        .maybe_zoom(zoom)
        .build()
}
