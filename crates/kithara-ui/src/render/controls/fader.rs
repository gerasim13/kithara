use std::cell::RefCell;

use iced::{
    Element, Event, Length, Rectangle, Renderer, Theme,
    mouse::{Cursor, Interaction},
    widget::{
        Space,
        canvas::{self, Action, Canvas, Frame, Geometry},
    },
};
use kithara_platform::time::Instant;
use num_traits::cast::AsPrimitive;

use crate::{
    atoms::design::{
        crossfader::Crossfader,
        fader::{Fader, rail_bounds},
    },
    backends::replay_ordered,
    draw::{DrawList, DrawListBuilder, Rect},
    engine::scalar_value,
    interact::{
        CursorShape, Hit, Hover, iced as iced_interact,
        recognizers::{Scalar, ScalarState, Track},
    },
    module::FaderStyle,
    render::{InputOwner, ReadValue, Skin, UiEvent, scalar},
    skin::FaderSkin,
    text::{TextContext, TextResources},
};

/// The whole fader on one canvas, painted by the shared atom. The rail a
/// pointer grabs is carved out of these bounds by [`rail_bounds`], which is the
/// same carve the Masonry host and the engine plan use.
pub(crate) fn fader_slider<'skin>(
    path: &str,
    value: f64,
    label: Option<&str>,
    style: FaderStyle,
    skin: &'skin Skin,
    owner: InputOwner,
) -> Element<'skin, UiEvent> {
    let paint = FaderPaint::new(value, label, style, skin);
    match owner {
        InputOwner::Leaf => FaderProgram::new(path, paint, skin.fader.step).view(),
        InputOwner::Engine => paint.view(),
    }
}

struct FaderProgram<'skin> {
    drag: Scalar,
    drag_step: f64,
    paint: FaderPaint<'skin>,
    path: String,
}

impl<'skin> FaderProgram<'skin> {
    fn new(path: &str, paint: FaderPaint<'skin>, drag_step: f64) -> Self {
        Self {
            drag: Scalar::builder()
                .track(Track::AbsoluteHorizontal)
                .hover(Hover::new(CursorShape::Grab))
                .build(),
            drag_step,
            paint,
            path: path.to_owned(),
        }
    }

    fn view(self) -> Element<'skin, UiEvent> {
        let height = self.paint.metrics.control_height;
        Canvas::new(self)
            .width(Length::Fill)
            .height(Length::Fixed(height))
            .into()
    }
}

impl canvas::Program<UiEvent> for FaderProgram<'_> {
    type State = ScalarState;

    fn draw(
        &self,
        _state: &ScalarState,
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: Cursor,
    ) -> Vec<Geometry> {
        self.paint.geometry(renderer, bounds)
    }

    fn mouse_interaction(
        &self,
        state: &ScalarState,
        bounds: Rectangle,
        cursor: Cursor,
    ) -> Interaction {
        self.drag.cursor(state, &self.rail(bounds, cursor)).into()
    }

    fn update(
        &self,
        state: &mut ScalarState,
        event: &Event,
        bounds: Rectangle,
        cursor: Cursor,
    ) -> Option<Action<UiEvent>> {
        let input = iced_interact::input(event)?;
        let hit = self.rail(bounds, cursor);
        scalar(
            &self.path,
            self.drag
                .on_input(state, input, &hit, Instant::now())
                .map(|value| scalar_value(input, value, Some(self.drag_step))),
        )
    }
}

impl FaderProgram<'_> {
    /// The pointer only drives the rail, not the caption or the padding around
    /// it, so the gesture is measured against the same rectangle that is drawn.
    fn rail(&self, bounds: Rectangle, cursor: Cursor) -> Hit {
        Hit::new(
            cursor.position().map(Into::into),
            rail_bounds(
                bounds.into(),
                self.paint.style,
                self.paint.label.is_some(),
                self.paint.metrics,
            ),
        )
    }
}

struct FaderPaint<'skin> {
    label: Option<String>,
    metrics: FaderSkin,
    painter: Fader,
    resources: &'skin TextResources,
    style: FaderStyle,
    text: RefCell<Option<TextContext>>,
    value: f32,
}

impl<'skin> FaderPaint<'skin> {
    fn new(value: f64, label: Option<&str>, style: FaderStyle, skin: &'skin Skin) -> Self {
        Self {
            label: label.map(ToOwned::to_owned),
            metrics: skin.fader,
            painter: Fader::new(style, skin),
            resources: skin.text_resources(),
            style,
            text: RefCell::new(None),
            value: value.clamp(0.0, 1.0).as_(),
        }
    }

    fn view(self) -> Element<'skin, UiEvent> {
        let height = self.metrics.control_height;
        Canvas::new(self)
            .width(Length::Fill)
            .height(Length::Fixed(height))
            .into()
    }

    fn paint(&self, builder: &mut DrawListBuilder, bounds: Rect) {
        let mut text = self.text.borrow_mut();
        let text = text.get_or_insert_with(|| self.resources.into());
        self.painter
            .paint(builder, text, self.value, self.label.as_deref(), bounds);
    }

    fn geometry(&self, renderer: &Renderer, bounds: Rectangle) -> Vec<Geometry> {
        let mut frame = Frame::new(renderer, bounds.size());
        let mut builder = DrawListBuilder::default();
        self.paint(
            &mut builder,
            Rect {
                h: bounds.height,
                w: bounds.width,
                x: 0.0,
                y: 0.0,
            },
        );
        replay_ordered(&builder.finish(), &mut frame, self.resources);
        vec![frame.into_geometry()]
    }
}

impl canvas::Program<UiEvent> for FaderPaint<'_> {
    type State = ();

    fn draw(
        &self,
        _state: &(),
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: Cursor,
    ) -> Vec<Geometry> {
        self.geometry(renderer, bounds)
    }
}

pub(crate) fn crossfader<'a>(
    path: &str,
    ticks: bool,
    value: Option<&ReadValue<'_>>,
    skin: &'a Skin,
    owner: InputOwner,
) -> Element<'a, UiEvent> {
    let Some(ReadValue::Scalar(value)) = value else {
        return Space::new().into();
    };
    let paint = CrossfaderPaint::new(
        Crossfader::new(ticks, skin),
        value.clamp(0.0, 1.0).as_(),
        skin,
    );
    match owner {
        InputOwner::Leaf => Canvas::new(CrossfaderProgram::new(path, paint))
            .width(Length::Fill)
            .height(Length::Fill)
            .into(),
        InputOwner::Engine => Canvas::new(paint)
            .width(Length::Fill)
            .height(Length::Fill)
            .into(),
    }
}

struct CrossfaderProgram<'skin> {
    drag: Scalar,
    paint: CrossfaderPaint<'skin>,
    path: String,
}

impl<'skin> CrossfaderProgram<'skin> {
    fn new(path: &str, paint: CrossfaderPaint<'skin>) -> Self {
        Self {
            drag: Scalar::builder()
                .track(Track::AbsoluteHorizontal)
                .hover(Hover::new(CursorShape::ResizeH))
                .build(),
            paint,
            path: path.to_owned(),
        }
    }
}

impl canvas::Program<UiEvent> for CrossfaderProgram<'_> {
    type State = CrossfaderState;

    fn draw(
        &self,
        state: &CrossfaderState,
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: Cursor,
    ) -> Vec<Geometry> {
        self.paint.geometry(&state.paint, renderer, bounds)
    }

    fn mouse_interaction(
        &self,
        state: &CrossfaderState,
        bounds: Rectangle,
        cursor: Cursor,
    ) -> Interaction {
        self.drag
            .cursor(&state.drag, &iced_interact::hit(bounds, cursor))
            .into()
    }

    fn update(
        &self,
        state: &mut CrossfaderState,
        event: &Event,
        bounds: Rectangle,
        cursor: Cursor,
    ) -> Option<Action<UiEvent>> {
        let input = iced_interact::input(event)?;
        let hit = iced_interact::hit(bounds, cursor);
        scalar(
            &self.path,
            self.drag
                .on_input(&mut state.drag, input, &hit, Instant::now())
                .map(f64::from),
        )
    }
}

#[derive(Default)]
struct CrossfaderState {
    drag: ScalarState,
    paint: CrossfaderPaintState,
}

#[derive(Default)]
struct CrossfaderPaintState {
    text: RefCell<Option<TextContext>>,
}

struct CrossfaderPaint<'skin> {
    painter: Crossfader,
    resources: &'skin TextResources,
    value: f32,
}

impl<'skin> CrossfaderPaint<'skin> {
    fn new(painter: Crossfader, value: f32, skin: &'skin Skin) -> Self {
        Self {
            painter,
            resources: skin.text_resources(),
            value,
        }
    }

    fn draw_list(&self, state: &CrossfaderPaintState, bounds: Rect) -> DrawList {
        let mut text = state.text.borrow_mut();
        let text = text.get_or_insert_with(|| self.resources.into());
        let mut builder = DrawListBuilder::default();
        self.painter.paint(&mut builder, text, self.value, bounds);
        builder.finish()
    }

    fn geometry(
        &self,
        state: &CrossfaderPaintState,
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
        replay_ordered(&list, &mut frame, self.resources);
        vec![frame.into_geometry()]
    }
}

impl canvas::Program<UiEvent> for CrossfaderPaint<'_> {
    type State = CrossfaderPaintState;

    fn draw(
        &self,
        state: &CrossfaderPaintState,
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: Cursor,
    ) -> Vec<Geometry> {
        self.geometry(state, renderer, bounds)
    }
}

#[cfg(test)]
mod tests {
    use iced::{Point, mouse};
    use kithara_test_utils::kithara;

    use super::*;
    use crate::{builtin, render::ControlAction};

    const CANVAS: Rect = Rect {
        h: 34.0,
        w: 200.0,
        x: 0.0,
        y: 0.0,
    };

    /// The iced adapter owns no picture of its own: it hands its canvas to the
    /// shared atom, which is what the Masonry host draws too.
    #[kithara::test]
    fn the_adapter_draws_exactly_what_the_shared_atom_draws() {
        for (style, label) in [
            (FaderStyle::Default, None),
            (FaderStyle::Default, Some("VOL")),
            (FaderStyle::Volume, None),
        ] {
            let mut actual = DrawListBuilder::default();
            FaderPaint::new(0.5, label, style, builtin::skin()).paint(&mut actual, CANVAS);

            let mut expected = DrawListBuilder::default();
            Fader::new(style, builtin::skin()).paint(
                &mut expected,
                &mut TextContext::from(builtin::skin().text_resources()),
                0.5,
                label,
                CANVAS,
            );

            assert_eq!(
                actual.finish(),
                expected.finish(),
                "{style:?} with caption {label:?}"
            );
        }
    }

    /// A press seeks, and the fraction it seeks to is measured against the rail
    /// the atom drew, not against the whole control.
    #[kithara::test]
    fn a_press_seeks_to_the_fraction_of_the_rail_it_landed_on() {
        let bounds = Rectangle {
            height: CANVAS.h,
            width: CANVAS.w,
            x: 0.0,
            y: 0.0,
        };
        let rail = rail_bounds(CANVAS, FaderStyle::Default, false, builtin::skin().fader);
        let press = Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left));

        const STEP: f64 = 0.01;

        for fraction in [0.0_f32, 0.25, 0.5, 0.75] {
            let paint = FaderPaint::new(0.5, None, FaderStyle::Default, builtin::skin());
            let program = FaderProgram::new("faders/default", paint, STEP);
            let cursor = Cursor::Available(Point::new(rail.x + rail.w * fraction, rail.y + 1.0));
            let mut state = ScalarState::default();
            let action = canvas::Program::update(&program, &mut state, &press, bounds, cursor)
                .unwrap_or_else(|| panic!("a press at {fraction} of the rail must publish"));
            let Some(UiEvent::Control {
                action: ControlAction::SetScalar(value),
                ..
            }) = action.into_inner().0
            else {
                panic!("a press at {fraction} of the rail must set a scalar");
            };

            assert!(
                (value - f64::from(fraction)).abs() <= STEP,
                "a press at {fraction} of the rail set {value}"
            );
        }
    }

    #[cfg(feature = "masonry-host")]
    #[kithara::test]
    fn iced_and_masonry_record_the_same_crossfader() {
        let skin = builtin::skin();
        let bounds = Rect {
            h: 64.0,
            w: 220.0,
            x: 0.0,
            y: 0.0,
        };
        for ticks in [false, true] {
            let iced = CrossfaderPaint::new(Crossfader::new(ticks, skin), 0.8, skin)
                .draw_list(&CrossfaderPaintState::default(), bounds);
            let mut masonry =
                crate::render::masonry::Painted::new(Crossfader::new(ticks, skin), 0.8_f32, skin);

            assert_eq!(
                iced,
                crate::render::masonry::MasonryControl::draw_list(&mut masonry, bounds)
            );
        }
    }
}
