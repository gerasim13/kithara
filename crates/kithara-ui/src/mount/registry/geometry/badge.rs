use crate::{expand::Binding, ids::InternId, mount::{Cell, StatusDot, Swatch}, skin::{ColorRole, Tone}};

pub(crate) fn status_dot<'a>(
    _label: InternId,
    _presentation: (Option<f32>, Tone, Option<Tone>, Option<&'a Binding>)
) -> StatusDot {
    StatusDot::builder().build()
}

pub(crate) fn swatch(
    _role: ColorRole, _label: InternId
) -> Swatch {
    Swatch::builder().build()
}

pub(crate) fn cell(
    _label: Option<InternId>, _highlighted: bool
) -> Cell {
    Cell::builder().build()
}
