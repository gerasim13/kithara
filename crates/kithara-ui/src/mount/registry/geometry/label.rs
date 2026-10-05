use super::super::{GlyphPresentation, TextPresentation};
use crate::{
    ids::InternId,
    module::{GlyphStyle, ScalarFormat, TextStyle, Tone},
    mount::{Glyph, Readout, Select, Telemetry, Text},
};

pub(crate) fn text<'a>(style: TextStyle, _presentation: TextPresentation<'a>) -> Text {
    Text::builder().style(style).build()
}

pub(crate) fn glyph<'a>(style: GlyphStyle, _presentation: GlyphPresentation<'a>) -> Glyph {
    Glyph::builder().style(style).build()
}

pub(crate) fn telemetry(_format: ScalarFormat, _framed: bool) -> Telemetry {
    Telemetry::builder().build()
}

pub(crate) fn select(_label: InternId) -> Select {
    Select::builder().build()
}

pub(crate) fn readout(_label: Option<InternId>, _tone: Tone, _framed: bool) -> Readout {
    Readout::builder().build()
}
