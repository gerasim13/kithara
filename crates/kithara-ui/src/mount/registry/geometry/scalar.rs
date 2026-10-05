use crate::{ids::InternId, module::FaderStyle, mount::{Crossfader, Fader, Knob, VuVertical}};

pub(crate) fn crossfader(
    _ticks: bool
) -> Crossfader {
    Crossfader::builder().build()
}

pub(crate) fn fader(
    _style: FaderStyle, _label: Option<InternId>
) -> Fader {
    Fader::builder().build()
}

pub(crate) fn knob(
    _label: Option<InternId>
) -> Knob {
    Knob::builder().build()
}

pub(crate) fn vu_vertical(
    _ticks: bool
) -> VuVertical {
    VuVertical::builder().build()
}
