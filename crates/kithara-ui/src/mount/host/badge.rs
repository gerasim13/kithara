use crate::{
    expand::Binding,
    ids::InternId,
    module::Tone,
    mount::{Cell, StatusDot, Swatch},
    skin::ColorRole,
};

pub(crate) fn status_dot<'a>(
    label: InternId,
    (dot_size, tone, active_tone, active): (Option<f32>, Tone, Option<Tone>, Option<&'a Binding>),
) -> StatusDot<'a> {
    StatusDot::builder()
        .maybe_active(active)
        .maybe_active_tone(active_tone)
        .maybe_dot_size(dot_size)
        .label(label)
        .tone(tone)
        .build()
}

pub(crate) fn swatch(role: ColorRole, label: InternId) -> Swatch {
    Swatch::builder().label(label).role(role).build()
}

pub(crate) fn cell(label: Option<InternId>, highlighted: bool) -> Cell {
    Cell::builder()
        .highlighted(highlighted)
        .maybe_label(label)
        .build()
}
