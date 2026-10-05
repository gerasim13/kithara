use super::super::registry::{GlyphPresentation, TextPresentation};
use crate::{
    ids::InternId,
    module::{GlyphStyle, ScalarFormat, TextStyle, Tone},
    mount::{Glyph, Readout, Select, Telemetry, Text},
};

pub(crate) fn text<'a>(
    style: TextStyle,
    (label, color, active_color, active, align, font, weight): TextPresentation<'a>,
) -> Text<'a> {
    Text::builder()
        .style(style)
        .maybe_active(active)
        .maybe_active_color(active_color)
        .align(align)
        .maybe_color(color)
        .maybe_font(font)
        .maybe_label(label)
        .maybe_weight(weight)
        .build()
}

pub(crate) fn glyph<'a>(
    style: GlyphStyle,
    (icon, active_icon, color, active_color, active): GlyphPresentation<'a>,
) -> Glyph<'a> {
    Glyph::builder()
        .style(style)
        .maybe_active(active)
        .maybe_active_color(active_color)
        .maybe_active_icon(active_icon)
        .maybe_color(color)
        .icon(icon)
        .build()
}

pub(crate) fn telemetry(format: ScalarFormat, framed: bool) -> Telemetry {
    Telemetry::builder().format(format).framed(framed).build()
}

pub(crate) fn select(label: InternId) -> Select {
    Select::builder().label(label).build()
}

pub(crate) fn readout(label: Option<InternId>, tone: Tone, framed: bool) -> Readout {
    Readout::builder()
        .framed(framed)
        .maybe_label(label)
        .tone(tone)
        .build()
}
