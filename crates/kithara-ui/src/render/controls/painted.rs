use std::cell::RefCell;

use iced::{
    Element, Event, Length, Rectangle, Renderer, Theme,
    mouse::{self, Cursor},
    widget::{
        Space,
        canvas::{self, Action, Canvas, Frame, Geometry},
    },
};
use kithara_platform::time::Instant;

use crate::{
    atoms::{button::VisualState, meter::StereoMeter, painter::ControlPainter, vu::VerticalVu},
    backends::IcedBackend,
    draw::{DrawList, DrawListBuilder, Rect, replay},
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

/// The shaping context one painted canvas reuses between frames.
#[derive(Default)]
pub(crate) struct PaintState {
    text: RefCell<Option<TextContext>>,
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
        let mut frame = Frame::new(renderer, bounds.size());
        let list = self.draw_list(
            state,
            Rect {
                h: bounds.height,
                w: bounds.width,
                x: 0.0,
                y: 0.0,
            },
        );
        replay(
            &list,
            &mut IcedBackend::new(&mut frame, self.text_resources),
        );
        vec![frame.into_geometry()]
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
        atoms::toggle::Binary,
        builtin,
        render::{
            StereoLevels,
            masonry::{MasonryControl, Painted},
        },
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
