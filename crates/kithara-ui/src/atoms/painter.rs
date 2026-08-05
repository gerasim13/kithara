use crate::{
    atoms::{
        button::{Button, ButtonLabel, VisualState},
        chip::Chip,
        design::{crossfader::Crossfader, fader::Fader},
        meter::StereoMeter,
        nav_item::NavItem,
        tab::TabLarge,
        toggle::Binary,
        vu::VerticalVu,
    },
    draw::{DrawListBuilder, Rect},
    render::StereoLevels,
    text::TextContext,
};

/// A neutral painter, drawn the same way by every host.
///
/// The skin is resolved when the painter is built; everything that changes
/// while it is mounted arrives as `Data`. A host that keeps its widgets also
/// needs to be told when that data changes — see the `Retained` half of the
/// contract, which only such a host implements.
pub(crate) trait ControlPainter {
    /// What the host hands the painter each frame: a word for most, the pair a
    /// button swaps between while active, a value for a meter.
    type Data;

    fn draw(
        &self,
        list: &mut DrawListBuilder,
        text: &mut TextContext,
        data: &Self::Data,
        bounds: Rect,
        state: VisualState,
    );
}

/// What a control that shows one word and a state is handed each frame.
pub(crate) struct Labelled {
    pub(crate) active: bool,
    pub(crate) label: String,
}

impl ControlPainter for Chip {
    type Data = Labelled;

    fn draw(
        &self,
        list: &mut DrawListBuilder,
        text: &mut TextContext,
        data: &Self::Data,
        bounds: Rect,
        _state: VisualState,
    ) {
        self.paint(list, text, &data.label, data.active, bounds);
    }
}

impl ControlPainter for NavItem {
    type Data = Labelled;

    fn draw(
        &self,
        list: &mut DrawListBuilder,
        text: &mut TextContext,
        data: &Self::Data,
        bounds: Rect,
        _state: VisualState,
    ) {
        self.paint(list, text, &data.label, data.active, bounds);
    }
}

impl ControlPainter for TabLarge {
    type Data = Labelled;

    fn draw(
        &self,
        list: &mut DrawListBuilder,
        text: &mut TextContext,
        data: &Self::Data,
        bounds: Rect,
        _state: VisualState,
    ) {
        self.paint(list, text, &data.label, data.active, bounds);
    }
}

/// What a fader is handed each frame: its value and the caption beside it.
pub(crate) struct FaderData {
    pub(crate) label: Option<String>,
    pub(crate) value: f32,
}

impl ControlPainter for Fader {
    type Data = FaderData;

    fn draw(
        &self,
        list: &mut DrawListBuilder,
        text: &mut TextContext,
        data: &Self::Data,
        bounds: Rect,
        _state: VisualState,
    ) {
        self.paint(list, text, data.value, data.label.as_deref(), bounds);
    }
}

impl ControlPainter for Crossfader {
    type Data = f32;

    fn draw(
        &self,
        list: &mut DrawListBuilder,
        text: &mut TextContext,
        data: &Self::Data,
        bounds: Rect,
        _state: VisualState,
    ) {
        self.paint(list, text, *data, bounds);
    }
}

impl ControlPainter for VerticalVu {
    type Data = StereoLevels;

    fn draw(
        &self,
        list: &mut DrawListBuilder,
        _text: &mut TextContext,
        data: &Self::Data,
        bounds: Rect,
        _state: VisualState,
    ) {
        self.paint(list, *data, bounds);
    }
}

impl ControlPainter for StereoMeter {
    type Data = StereoLevels;

    fn draw(
        &self,
        list: &mut DrawListBuilder,
        _text: &mut TextContext,
        data: &Self::Data,
        bounds: Rect,
        _state: VisualState,
    ) {
        self.paint(list, *data, bounds);
    }
}

/// What a button is handed each frame: the word for each of its states, and
/// which state it is in.
pub(crate) struct ButtonData {
    pub(crate) active: bool,
    pub(crate) label: ButtonLabel<String>,
}

impl ControlPainter for Button {
    type Data = ButtonData;

    fn draw(
        &self,
        list: &mut DrawListBuilder,
        text: &mut TextContext,
        data: &Self::Data,
        bounds: Rect,
        state: VisualState,
    ) {
        self.paint(list, text, &data.label, data.active, bounds, state);
    }
}

impl ControlPainter for Binary {
    type Data = bool;

    fn draw(
        &self,
        list: &mut DrawListBuilder,
        _text: &mut TextContext,
        data: &Self::Data,
        bounds: Rect,
        _state: VisualState,
    ) {
        self.paint(list, *data, bounds);
    }
}
