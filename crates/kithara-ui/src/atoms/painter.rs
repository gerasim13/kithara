use crate::{
    atoms::{
        button::{Button, ButtonLabel, VisualState},
        chip::Chip,
        design::{
            cell::Cell, crossfader::Crossfader, fader::Fader, meter::Meter, status_dot::StatusDot,
            swatch::Swatch,
        },
        meter::StereoMeter,
        nav_item::NavItem,
        tab::TabLarge,
        toggle::Binary,
        vu::VerticalVu,
    },
    draw::{DrawListBuilder, Rect},
    render::{Mark, StereoLevels},
    solve::{Length, Size},
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

    /// Whether the pointer resting on or pressing the control changes what it
    /// draws, which decides if a host tracks those edges and repaints on them.
    const READS_POINTER: bool = false;

    fn draw(
        &self,
        list: &mut DrawListBuilder,
        text: &mut TextContext,
        data: &Self::Data,
        bounds: Rect,
        state: VisualState,
    );

    /// The box it asks for when the skin, rather than the row it sits in,
    /// settles an axis.
    ///
    /// A share of the row is the one length a document cannot state — `Dim` has
    /// no portion, and the portions in this repository all come from the skin —
    /// so it is said once here rather than once per host.
    fn length(&self, _text: &mut TextContext, _data: &Self::Data) -> Size<Length> {
        Size::new(Length::Fill, Length::Fill)
    }

    /// How big it actually is, on the axes it settles for itself.
    ///
    /// A zero on an axis means the painter has no opinion there and the row
    /// decides — which is what both hosts already do with a leaf that does not
    /// measure. Only the painters whose [`Self::length`] can answer `Shrink` or
    /// a measured `Fixed` need this; the rest fill what they are given.
    fn measure(&self, _text: &mut TextContext, _data: &Self::Data) -> Size {
        Size::ZERO
    }
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

/// What a nav item is handed each frame: its word, its state, and the mark it
/// shows beside them.
///
/// The mark travels with the word rather than with the skin because reading an
/// authored icon can fail, and a control whose art cannot be read draws nothing
/// at all rather than a row with a hole in it.
pub(crate) struct NavData {
    pub(crate) active: bool,
    pub(crate) label: String,
    pub(crate) mark: Mark,
}

impl ControlPainter for NavItem {
    type Data = NavData;

    fn draw(
        &self,
        list: &mut DrawListBuilder,
        text: &mut TextContext,
        data: &Self::Data,
        bounds: Rect,
        _state: VisualState,
    ) {
        self.paint(list, text, data, bounds);
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

    /// A tab is as wide as its own word: a strip of tabs is a row of headings,
    /// not a set of equal columns, so a tab that filled its share would move
    /// its neighbours whenever a word changed.
    fn length(&self, _text: &mut TextContext, _data: &Self::Data) -> Size<Length> {
        Self::declared_length(self.height())
    }

    fn measure(&self, text: &mut TextContext, data: &Self::Data) -> Size {
        let (width, height) = self.intrinsic_size(text, &data.label);
        Size::new(width, height)
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

    const READS_POINTER: bool = true;

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

    fn length(&self, text: &mut TextContext, data: &Self::Data) -> Size<Length> {
        self.declared(text, &data.label, data.active)
    }

    /// Only the width: every button fills the height of the row it sits in.
    fn measure(&self, text: &mut TextContext, data: &Self::Data) -> Size {
        Size::new(self.intrinsic_width(text, &data.label, data.active), 0.0)
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

impl ControlPainter for Meter {
    type Data = f32;

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

impl ControlPainter for StatusDot {
    type Data = String;

    fn draw(
        &self,
        list: &mut DrawListBuilder,
        text: &mut TextContext,
        data: &Self::Data,
        bounds: Rect,
        _state: VisualState,
    ) {
        self.paint(list, text, data, bounds);
    }
}

/// What a cell is handed each frame: its caption, and whether it is the one
/// picked out.
pub(crate) struct CellData {
    pub(crate) highlighted: bool,
    pub(crate) label: Option<String>,
}

impl ControlPainter for Cell {
    type Data = CellData;

    fn draw(
        &self,
        list: &mut DrawListBuilder,
        text: &mut TextContext,
        data: &Self::Data,
        bounds: Rect,
        _state: VisualState,
    ) {
        self.paint(list, text, data.label.as_deref(), data.highlighted, bounds);
    }
}

impl ControlPainter for Swatch {
    type Data = String;

    fn draw(
        &self,
        list: &mut DrawListBuilder,
        text: &mut TextContext,
        data: &Self::Data,
        bounds: Rect,
        _state: VisualState,
    ) {
        self.paint(list, text, data, bounds);
    }
}
