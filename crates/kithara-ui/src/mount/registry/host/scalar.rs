use crate::{ids::InternId, module::FaderStyle, mount::{Crossfader, Fader, Knob, VuVertical}};

pub(crate) fn crossfader(
    ticks: bool
) -> Crossfader {
    Crossfader::builder().ticks(ticks).build()
}

pub(crate) fn fader(
    style: FaderStyle, label: Option<InternId>
) -> Fader {
    Fader::builder().maybe_label(label).style(style).build()
}

pub(crate) fn knob(
    label: Option<InternId>
) -> Knob {
    Knob::builder().maybe_label(label).build()
}

pub(crate) fn vu_vertical(
    ticks: bool
) -> VuVertical {
    VuVertical::builder().ticks(ticks).build()
}
