use std::cell::RefCell;

use iced::{
    Element, Event, Length, Rectangle, Renderer, Theme,
    mouse::{self, Cursor},
    widget::{
        Space,
        canvas::{self, Action, Canvas, Geometry},
    },
};
use kithara_platform::time::Instant;

use crate::{
    atoms::{button::VisualState, meter::StereoMeter, painter::ControlPainter, vu::VerticalVu},
    backends::replay_ordered,
    draw::{DrawList, DrawListBuilder, Rect},
    interact::{
        CursorShape, Hover, iced as iced_interact,
        recognizers::{Scalar, ScalarState, Track, click},
    },
    render::{InputOwner, ReadValue, Skin, UiEvent, activate, scalar},
    text::{TextContext, TextResources},
};

pub(crate) fn vu_vertical<'a>(
    path: &str,
    ticks: bool,
    value: Option<&ReadValue<'_>>,
    skin: &'a Skin,
    owner: InputOwner,
) -> Element<'a, UiEvent> {
    let Some(ReadValue::Stereo(levels)) = value else {
        return Space::new().into();
    };
    let paint = Paint::new(VerticalVu::new(ticks, skin), *levels, skin);
    match owner {
        InputOwner::Leaf => {
            Gesture::drag(path, paint, Track::AbsoluteVertical, CursorShape::ResizeV).view()
        }
        InputOwner::Engine => paint.view(),
    }
}

pub(crate) fn vu_stereo<'a>(
    path: &str,
    value: Option<&ReadValue<'_>>,
    skin: &'a Skin,
    owner: InputOwner,
) -> Element<'a, UiEvent> {
    let Some(ReadValue::Stereo(levels)) = value else {
        return Space::new().into();
    };
    let paint = Paint::new(StereoMeter::new(skin), *levels, skin);
    match owner {
        InputOwner::Leaf => {
            Gesture::drag(path, paint, Track::AbsoluteHorizontal, CursorShape::ResizeH).view()
        }
        InputOwner::Engine => paint.view(),
    }
}

/// One neutral painter drawn straight into an iced canvas.
///
/// Both hosts replay the same [`DrawList`], so there is one adapter rather than
/// one per control: what a control looks like is settled in its painter, and
/// neither host gets an opinion about it.
pub(crate) struct Paint<'skin, Painter>
where
    Painter: ControlPainter,
{
    data: Painter::Data,
    painter: Painter,
    text_resources: &'skin TextResources,
}

/// What one painted canvas keeps between frames: the shaping context, the
/// geometry it last tessellated, and the list that geometry was drawn from.
#[derive(Default)]
pub(crate) struct PaintState {
    drawn: RefCell<Option<DrawList>>,
    geometry: canvas::Cache,
    text: RefCell<Option<TextContext>>,
}

impl PaintState {
    /// Drops the kept geometry when the list behind it changed, and reports
    /// whether it had to. That answer is the whole of this cache, so it is
    /// returned rather than inferred from the pixels.
    ///
    /// The key is the drawn list itself and not a hash of what went into it.
    /// An input hash has to name everything a painter reads — the value, the
    /// box, and the shape the skin gave it — and anything it forgets freezes a
    /// control on the screen. Two equal lists draw the same picture by
    /// construction, and equality also settles the floats for free: `-0.0`
    /// compares equal to `0.0`, which is the normalisation the lsq wheel spells
    /// out with `OrderedFloat` for its own path caches.
    fn refresh(&self, list: &DrawList) -> bool {
        let mut drawn = self.drawn.borrow_mut();
        if drawn.as_ref() == Some(list) {
            return false;
        }
        self.geometry.clear();
        *drawn = Some(list.clone());
        true
    }
}

impl<'skin, Painter> Paint<'skin, Painter>
where
    Painter: ControlPainter + 'skin,
{
    pub(crate) fn new(painter: Painter, data: Painter::Data, skin: &'skin Skin) -> Self {
        Self {
            data,
            painter,
            text_resources: skin.text_resources(),
        }
    }

    pub(crate) fn view(self) -> Element<'skin, UiEvent> {
        self.sized(Length::Fill, Length::Fill)
    }

    /// Sized to what the control asked for rather than to its parent, for the
    /// controls whose skin fixes their box.
    pub(crate) fn sized(self, width: Length, height: Length) -> Element<'skin, UiEvent> {
        Canvas::new(self).width(width).height(height).into()
    }

    pub(crate) fn draw_list(&self, state: &PaintState, bounds: Rect) -> DrawList {
        let mut text = state.text.borrow_mut();
        let text = text.get_or_insert_with(|| self.text_resources.into());
        let mut builder = DrawListBuilder::default();
        self.painter
            .draw(&mut builder, text, &self.data, bounds, VisualState::Idle);
        builder.finish()
    }

    fn geometry(
        &self,
        state: &PaintState,
        renderer: &Renderer,
        bounds: Rectangle,
    ) -> Vec<Geometry> {
        let list = self.draw_list(
            state,
            Rect {
                h: bounds.height,
                w: bounds.width,
                x: 0.0,
                y: 0.0,
            },
        );
        state.refresh(&list);
        vec![state.geometry.draw(renderer, bounds.size(), |frame| {
            replay_ordered(&list, frame, self.text_resources);
        })]
    }
}

impl<Painter> canvas::Program<UiEvent> for Paint<'_, Painter>
where
    Painter: ControlPainter,
{
    type State = PaintState;

    fn draw(
        &self,
        state: &PaintState,
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: Cursor,
    ) -> Vec<Geometry> {
        self.geometry(state, renderer, bounds)
    }
}

/// A painted control that also answers a gesture.
///
/// Pressing and dragging differ only in what the pointer means, so one wrapper
/// carries both rather than two near-identical canvases.
pub(crate) struct Gesture<'skin, Painter>
where
    Painter: ControlPainter,
{
    paint: Paint<'skin, Painter>,
    path: String,
    recognize: Recognize,
}

/// What the pointer means to a control: a press it activates on, or a drag
/// along one axis that sets a scalar.
enum Recognize {
    Press,
    Drag(Box<Scalar>),
}

/// What a gesturing canvas keeps between frames: the gesture, and the shaping
/// context the painter draws through.
#[derive(Default)]
pub(crate) struct GestureState {
    drag: ScalarState,
    paint: PaintState,
}

impl<'skin, Painter> Gesture<'skin, Painter>
where
    Painter: ControlPainter + 'skin,
{
    pub(crate) fn press(path: &str, paint: Paint<'skin, Painter>) -> Self {
        Self {
            paint,
            path: path.to_owned(),
            recognize: Recognize::Press,
        }
    }

    fn drag(path: &str, paint: Paint<'skin, Painter>, track: Track, cursor: CursorShape) -> Self {
        Self {
            paint,
            path: path.to_owned(),
            recognize: Recognize::Drag(Box::new(
                Scalar::builder()
                    .track(track)
                    .hover(Hover::new(cursor))
                    .build(),
            )),
        }
    }

    pub(crate) fn view(self) -> Element<'skin, UiEvent> {
        Canvas::new(self)
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }
}

impl<Painter> canvas::Program<UiEvent> for Gesture<'_, Painter>
where
    Painter: ControlPainter,
{
    type State = GestureState;

    fn draw(
        &self,
        state: &GestureState,
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: Cursor,
    ) -> Vec<Geometry> {
        self.paint.geometry(&state.paint, renderer, bounds)
    }

    fn mouse_interaction(
        &self,
        state: &GestureState,
        bounds: Rectangle,
        cursor: Cursor,
    ) -> mouse::Interaction {
        let hit = iced_interact::hit(bounds, cursor);
        match &self.recognize {
            Recognize::Press => Hover::new(CursorShape::Pointer).cursor(false, &hit).into(),
            Recognize::Drag(drag) => drag.cursor(&state.drag, &hit).into(),
        }
    }

    fn update(
        &self,
        state: &mut GestureState,
        event: &Event,
        bounds: Rectangle,
        cursor: Cursor,
    ) -> Option<Action<UiEvent>> {
        let input = iced_interact::input(event)?;
        let hit = iced_interact::hit(bounds, cursor);
        match &self.recognize {
            Recognize::Press => activate(&self.path, click::on_input(input, &hit)),
            Recognize::Drag(drag) => scalar(
                &self.path,
                drag.on_input(&mut state.drag, input, &hit, Instant::now())
                    .map(f64::from),
            ),
        }
    }
}

#[cfg(all(test, feature = "masonry-host"))]
mod tests {
    use kithara_test_utils::kithara;

    use super::*;
    use crate::{
        atoms::{
            design::{cell::Cell, meter::Meter, status_dot::StatusDot, swatch::Swatch},
            painter::CellData,
            toggle::Binary,
        },
        builtin,
        module::{ButtonStyle, ChipStyle, Tone},
        render::{
            StereoLevels,
            masonry::{MasonryControl, Painted},
        },
        skin::ColorRole,
    };

    const LEVELS: StereoLevels = StereoLevels {
        l: 0.6,
        r: 0.4,
        volume: 0.8,
    };

    #[kithara::test]
    fn iced_and_masonry_record_the_same_vertical_vu() {
        let skin = builtin::skin();
        let bounds = Rect {
            h: 96.0,
            w: 38.0,
            x: 0.0,
            y: 0.0,
        };
        for ticks in [false, true] {
            let iced = Paint::new(VerticalVu::new(ticks, skin), LEVELS, skin)
                .draw_list(&PaintState::default(), bounds);
            let mut masonry = Painted::new(VerticalVu::new(ticks, skin), LEVELS, skin);

            assert_eq!(iced, MasonryControl::draw_list(&mut masonry, bounds));
        }
    }

    /// The switch is the first control both hosts reach through the same
    /// generic adapter, so this is also the check that the adapter itself does
    /// not add or drop anything on the way.
    #[kithara::test]
    fn iced_and_masonry_record_the_same_switch() {
        let skin = builtin::skin();
        for (name, painter, bounds) in [
            (
                "toggle",
                Binary::toggle as fn(&Skin) -> Binary,
                Rect {
                    h: 14.0,
                    w: 26.0,
                    x: 0.0,
                    y: 0.0,
                },
            ),
            (
                "checkbox",
                Binary::checkbox as fn(&Skin) -> Binary,
                Rect {
                    h: 14.0,
                    w: 14.0,
                    x: 0.0,
                    y: 0.0,
                },
            ),
        ] {
            for active in [false, true] {
                let iced = Paint::new(painter(skin), active, skin)
                    .draw_list(&PaintState::default(), bounds);
                let mut masonry = Painted::new(painter(skin), active, skin);

                assert_eq!(
                    iced,
                    MasonryControl::draw_list(&mut masonry, bounds),
                    "the two hosts must record the same {name}"
                );
            }
        }
    }

    /// A meter with no endpoint behind it is an empty track under both hosts,
    /// not an empty box under one of them.
    #[kithara::test]
    fn iced_and_masonry_record_the_same_meter() {
        let skin = builtin::skin();
        let bounds = Rect {
            h: 10.0,
            w: 100.0,
            x: 0.0,
            y: 0.0,
        };
        for level in [0.0, 0.5, 1.0] {
            let iced =
                Paint::new(Meter::new(skin), level, skin).draw_list(&PaintState::default(), bounds);
            let mut masonry = Painted::new(Meter::new(skin), level, skin);

            assert_eq!(iced, MasonryControl::draw_list(&mut masonry, bounds));
        }
    }

    #[kithara::test]
    fn iced_and_masonry_record_the_same_cell() {
        let skin = builtin::skin();
        let bounds = Rect {
            h: 36.0,
            w: 40.0,
            x: 0.0,
            y: 0.0,
        };
        for label in [None, Some("A1".to_owned())] {
            for highlighted in [false, true] {
                let data = || CellData {
                    highlighted,
                    label: label.clone(),
                };
                let iced = Paint::new(Cell::new(skin), data(), skin)
                    .draw_list(&PaintState::default(), bounds);
                let mut masonry = Painted::new(Cell::new(skin), data(), skin);

                assert_eq!(iced, MasonryControl::draw_list(&mut masonry, bounds));
            }
        }
    }

    #[kithara::test]
    fn iced_and_masonry_record_the_same_swatch() {
        let skin = builtin::skin();
        let bounds = Rect {
            h: 78.0,
            w: 120.0,
            x: 0.0,
            y: 0.0,
        };
        for role in [ColorRole::Accent, ColorRole::BgInset] {
            let iced = Paint::new(Swatch::new(role, skin), "ACCENT".to_owned(), skin)
                .draw_list(&PaintState::default(), bounds);
            let mut masonry = Painted::new(Swatch::new(role, skin), "ACCENT".to_owned(), skin);

            assert_eq!(iced, MasonryControl::draw_list(&mut masonry, bounds));
        }
    }

    #[kithara::test]
    fn iced_and_masonry_record_the_same_status_dot() {
        let skin = builtin::skin();
        let bounds = Rect {
            h: 18.0,
            w: 64.0,
            x: 0.0,
            y: 0.0,
        };
        for tone in [Tone::Neutral, Tone::Accent, Tone::Success, Tone::Danger] {
            let iced = Paint::new(StatusDot::new(tone, skin), "LIVE".to_owned(), skin)
                .draw_list(&PaintState::default(), bounds);
            let mut masonry = Painted::new(StatusDot::new(tone, skin), "LIVE".to_owned(), skin);

            assert_eq!(iced, MasonryControl::draw_list(&mut masonry, bounds));
        }
    }

    #[kithara::test]
    fn iced_and_masonry_record_the_same_stereo_meter() {
        let skin = builtin::skin();
        let bounds = Rect {
            h: 24.0,
            w: 120.0,
            x: 0.0,
            y: 0.0,
        };
        let iced = Paint::new(StereoMeter::new(skin), LEVELS, skin)
            .draw_list(&PaintState::default(), bounds);
        let mut masonry = Painted::new(StereoMeter::new(skin), LEVELS, skin);

        assert_eq!(iced, MasonryControl::draw_list(&mut masonry, bounds));
    }
}

/// What the cache is allowed to keep, and what it must drop.
#[cfg(test)]
mod cached {
    use kithara_test_utils::kithara;

    use super::*;
    use crate::{
        atoms::{design::meter::Meter, toggle::Binary},
        builtin,
    };

    const BOX: Rect = Rect {
        h: 40.0,
        w: 120.0,
        x: 0.0,
        y: 0.0,
    };

    /// One frame of the immediate-mode host: the element is built afresh, and
    /// the canvas state is the one thing that survived the last frame.
    fn frame<Painter>(
        state: &PaintState,
        painter: Painter,
        data: Painter::Data,
        bounds: Rect,
    ) -> bool
    where
        Painter: ControlPainter,
    {
        let paint = Paint::new(painter, data, builtin::skin());
        state.refresh(&paint.draw_list(state, bounds))
    }

    /// The host rebuilds the whole element tree every frame. A control whose
    /// value did not move must not be tessellated again — and must be the
    /// moment it does.
    #[kithara::test]
    fn an_unchanged_control_keeps_the_geometry_it_drew() {
        let skin = builtin::skin();
        let state = PaintState::default();

        assert!(
            frame(&state, Meter::new(skin), 0.5, BOX),
            "the first frame draws"
        );
        assert!(
            !frame(&state, Meter::new(skin), 0.5, BOX),
            "an unchanged control must keep what it drew"
        );
        assert!(
            frame(&state, Meter::new(skin), 0.75, BOX),
            "a control whose value moved must draw again"
        );
    }

    /// The same value in a different box is a different picture.
    #[kithara::test]
    fn a_control_that_was_resized_draws_again() {
        let skin = builtin::skin();
        let state = PaintState::default();
        let wider = Rect {
            w: BOX.w + 1.0,
            ..BOX
        };

        assert!(frame(&state, Meter::new(skin), 0.5, BOX));
        assert!(frame(&state, Meter::new(skin), 0.5, wider));
    }

    /// The canvas state belongs to a place in the tree, not to a control, so a
    /// document that puts a different control there must get a different
    /// picture. Keying on what a painter *reads* would miss this: a switch and
    /// a checkbox are one painter type and read the same `false`.
    #[kithara::test]
    fn a_control_replaced_by_another_does_not_keep_its_picture() {
        let skin = builtin::skin();
        let state = PaintState::default();

        assert!(frame(&state, Binary::toggle(skin), false, BOX));
        assert!(
            frame(&state, Binary::checkbox(skin), false, BOX),
            "a checkbox must not be left showing a switch"
        );
    }
}
