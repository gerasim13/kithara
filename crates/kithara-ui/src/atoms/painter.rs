use crate::{
    atoms::{
        bar::preset::{Preset, PresetData},
        button::{Button, ButtonLabel, VisualState},
        deck::summary::{Loaded, Summary},
        design::{
            fader::Fader,
            status_dot::{StatusDot, StatusDotData},
        },
        label::Telemetry,
        tab::TabLarge,
        wave::face::{Drawn, Wave},
    },
    draw::{DrawListBuilder, Rect},
    hosts::{icons::Mark, solve::Size},
    interact::Hit,
    shaping::TextContext,
};

/// Transient per-cell state owned by an indexed control adapter.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct IndexedVisual {
    pub(crate) hovered: Option<usize>,
    pub(crate) pressed_origin: Option<usize>,
}

/// A neutral painter, drawn the same way by every host.
///
/// The skin is resolved when the painter is built; everything that changes
/// while it is mounted arrives as `Data`. A host that keeps its widgets also
/// needs to be told when that data changes — see the `Retained` half of the
/// contract, which only such a host implements.
pub(crate) trait ControlPainter: Clone + PartialEq {
    /// What the host hands the painter each frame: a word for most, the pair a
    /// button swaps between while active, a value for a meter.
    type Data: Clone + PartialEq;

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

    /// Draws adapter-owned indexed state; ordinary painters keep normal drawing.
    fn draw_indexed(
        &self,
        list: &mut DrawListBuilder,
        text: &mut TextContext,
        data: &Self::Data,
        bounds: Rect,
        _visual: IndexedVisual,
    ) {
        self.draw(list, text, data, bounds, VisualState::Idle);
    }

    /// The part of its box the pointer works, when that is not all of it.
    ///
    /// A fader is a rail with a caption beside it, and the hand drives the rail
    /// alone — measuring the gesture against the whole control would put the
    /// zero of the value under the caption. Only the painter knows where it put
    /// the rail, so it is asked rather than told.
    fn grip_bounds(&self, _data: &Self::Data, bounds: Rect) -> Rect {
        bounds
    }

    /// Resolves the painted cell under the pointer.
    fn index_at(&self, _data: &Self::Data, hit: &Hit, count: usize) -> Option<usize> {
        hit.uniform_horizontal_index(count)
    }

    /// How big it actually is, on the axes it settles for itself.
    ///
    /// A zero on an axis means the painter has no opinion there and the row
    /// decides — which is what both hosts already do with a leaf that does not
    /// measure. Only the painters whose host length can answer `Shrink` or a
    /// measured `Fixed` need this; the rest fill what they are given.
    fn measure(&self, _text: &mut TextContext, _data: &Self::Data) -> Size {
        Size::ZERO
    }
}

/// What a control that shows one word and a state is handed each frame.
#[derive(Clone, PartialEq)]
pub(crate) struct Labelled {
    pub(crate) label: String,
    pub(crate) active: bool,
}

/// What a nav item is handed each frame: its word, its state, and the mark it
/// shows beside them.
///
/// The mark travels with the word rather than with the skin because reading an
/// authored icon can fail, and a control whose art cannot be read draws nothing
/// at all rather than a row with a hole in it.
#[derive(Clone, PartialEq)]
pub(crate) struct NavData {
    pub(crate) mark: Mark,
    pub(crate) label: String,
    pub(crate) active: bool,
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

    fn measure(&self, text: &mut TextContext, data: &Self::Data) -> Size {
        let (width, height) = self.intrinsic_size(text, &data.label);
        Size::new(width, height)
    }
}

/// What a control that sets one fraction and captions it is handed each frame.
#[derive(Clone, PartialEq)]
pub(crate) struct Captioned {
    pub(crate) label: Option<String>,
    pub(crate) value: f32,
}

impl ControlPainter for Fader {
    type Data = Captioned;

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

    fn grip_bounds(&self, data: &Self::Data, bounds: Rect) -> Rect {
        self.rail(bounds, data.label.is_some())
    }
}

/// What a button is handed each frame: the word for each of its states, and
/// which state it is in.
#[derive(Clone, PartialEq)]
pub(crate) struct ButtonData {
    pub(crate) label: ButtonLabel<String>,
    pub(crate) active: bool,
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
}

impl ControlPainter for StatusDot {
    type Data = StatusDotData;

    fn draw(
        &self,
        list: &mut DrawListBuilder,
        text: &mut TextContext,
        data: &StatusDotData,
        bounds: Rect,
        _state: VisualState,
    ) {
        self.paint_with_state(list, text, &data.label, bounds, data.active);
    }

    /// Only the width: a dot fills the height of the row it sits in.
    fn measure(&self, text: &mut TextContext, data: &StatusDotData) -> Size {
        Size::new(self.intrinsic_width(text, &data.label), 0.0)
    }
}

/// What a cell is handed each frame: its caption, and whether it is the one
/// picked out.
#[derive(Clone, PartialEq)]
pub(crate) struct CellData {
    pub(crate) label: Option<String>,
    pub(crate) highlighted: bool,
}

impl ControlPainter for Preset {
    type Data = PresetData;

    const READS_POINTER: bool = true;

    fn draw(
        &self,
        list: &mut DrawListBuilder,
        text: &mut TextContext,
        data: &Self::Data,
        bounds: Rect,
        _state: VisualState,
    ) {
        self.paint(list, text, data, bounds, IndexedVisual::default());
    }

    fn draw_indexed(
        &self,
        list: &mut DrawListBuilder,
        text: &mut TextContext,
        data: &Self::Data,
        bounds: Rect,
        visual: IndexedVisual,
    ) {
        self.paint(list, text, data, bounds, visual);
    }

    fn index_at(&self, data: &Self::Data, hit: &Hit, _count: usize) -> Option<usize> {
        self.hit_index(data, hit.area(), hit.inside()?)
    }
}

/// The naming panel steps aside while the pointer is on the waveform, so the
/// hand can see the shape it is about to scrub.
impl ControlPainter for Wave {
    type Data = Drawn;

    const READS_POINTER: bool = true;

    fn draw(
        &self,
        list: &mut DrawListBuilder,
        text: &mut TextContext,
        data: &Self::Data,
        bounds: Rect,
        state: VisualState,
    ) {
        self.paint(list, text, data, bounds, matches!(state, VisualState::Idle));
    }
}

impl ControlPainter for Summary {
    type Data = Loaded;

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

    /// Only the width: a headline fills the height of the panel it sits in.
    fn measure(&self, text: &mut TextContext, data: &Self::Data) -> Size {
        Size::new(self.intrinsic_width(text, data), 0.0)
    }
}

impl ControlPainter for Telemetry {
    type Data = f64;

    fn draw(
        &self,
        list: &mut DrawListBuilder,
        text: &mut TextContext,
        data: &Self::Data,
        bounds: Rect,
        _state: VisualState,
    ) {
        self.paint(list, text, &self.format(*data), bounds);
    }

    /// Only the width: a reading fills the height of the row it sits in.
    fn measure(&self, text: &mut TextContext, data: &Self::Data) -> Size {
        Size::new(self.intrinsic_width(text, &self.format(*data)), 0.0)
    }
}
