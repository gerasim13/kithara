use crate::{expand::Binding, ids::InternId, module::{GlyphStyle, IconName, ScalarFormat, TextAlign, TextStyle, Tone}, mount::{Glyph, Readout, Select, Telemetry, Text}, skin::{ColorRole, FontFamily, FontWeight}};

pub(crate) fn text<'a>(
    style: TextStyle,
    _presentation: (Option<InternId>, Option<ColorRole>, Option<ColorRole>, Option<&'a Binding>, TextAlign, Option<FontFamily>, Option<FontWeight>)
) -> Text {
    Text::builder().style(style).build()
}

pub(crate) fn glyph<'a>(
    style: GlyphStyle,
    _presentation: (IconName, Option<IconName>, Option<ColorRole>, Option<ColorRole>, Option<&'a Binding>)
) -> Glyph {
    Glyph::builder().style(style).build()
}

pub(crate) fn telemetry(
    _format: ScalarFormat, _framed: bool
) -> Telemetry {
    Telemetry::builder().build()
}

pub(crate) fn select(
    _label: InternId
) -> Select {
    Select::builder().build()
}

pub(crate) fn readout(
    _label: Option<InternId>, _tone: Tone, _framed: bool
) -> Readout {
    Readout::builder().build()
}
