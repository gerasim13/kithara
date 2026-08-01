use iced::{
    Element, Event, Length, Rectangle, Renderer, Theme,
    mouse::{Cursor, Interaction},
    widget::canvas::{self, Action, Canvas, Frame, Geometry},
};
use kithara_platform::time::Instant;
use num_traits::cast::AsPrimitive;

use crate::{
    backends::IcedBackend,
    draw::{DrawListBuilder, Rect, Rgba, replay},
    engine::scalar_value,
    interact::{
        CursorShape, Hover, iced as iced_interact,
        recognizers::{Scalar, ScalarState, Track},
    },
    render::{InputOwner, Skin, UiEvent, scalar},
    skin::{FaderSkin, FrameSkin},
    text::TextResources,
};

pub(crate) fn fader_slider<'skin>(
    path: &str,
    value: f64,
    skin: &'skin Skin,
    owner: InputOwner,
) -> Element<'skin, UiEvent> {
    let paint = FaderPaint::new(value, skin);
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
        let height = self.paint.metrics.slider_height;
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
        self.drag
            .cursor(state, &iced_interact::hit(bounds, cursor))
            .into()
    }

    fn update(
        &self,
        state: &mut ScalarState,
        event: &Event,
        bounds: Rectangle,
        cursor: Cursor,
    ) -> Option<Action<UiEvent>> {
        let input = iced_interact::input(event)?;
        let hit = iced_interact::hit(bounds, cursor);
        scalar(
            &self.path,
            self.drag
                .on_input(state, input, &hit, Instant::now())
                .map(|value| scalar_value(input, value, Some(self.drag_step))),
        )
    }
}

struct FaderPaint<'skin> {
    accent: Rgba,
    background: Rgba,
    handle: Rgba,
    handle_border: Rgba,
    metrics: FaderSkin,
    rail_border: Rgba,
    resources: &'skin TextResources,
    value: f32,
}

impl<'skin> FaderPaint<'skin> {
    fn new(value: f64, skin: &'skin Skin) -> Self {
        let metrics = skin.fader;
        Self {
            accent: skin.palette.accent.into(),
            background: skin.palette.bg_deep.into(),
            handle: skin.rgba(metrics.handle_color),
            handle_border: skin.rgba(metrics.handle_frame.border),
            metrics,
            rail_border: skin.rgba(metrics.rail_frame.border),
            resources: skin.text_resources(),
            value: value.clamp(0.0, 1.0).as_(),
        }
    }

    fn view(self) -> Element<'skin, UiEvent> {
        let height = self.metrics.slider_height;
        Canvas::new(self)
            .width(Length::Fill)
            .height(Length::Fixed(height))
            .into()
    }

    fn paint(&self, builder: &mut DrawListBuilder, bounds: Rect) {
        let handle_width = f32::from(self.metrics.handle_width);
        let offset = (bounds.w - handle_width) * self.value;
        let split = offset + handle_width / 2.0;
        let rail_y = (bounds.h - self.metrics.rail_width) / 2.0;
        let accent = Rect {
            h: self.metrics.rail_width,
            w: split,
            x: 0.0,
            y: rail_y,
        };
        let background = Rect {
            h: self.metrics.rail_width,
            w: bounds.w - split,
            x: split,
            y: rail_y,
        };
        let handle = Rect {
            h: bounds.h,
            w: handle_width,
            x: offset,
            y: 0.0,
        };

        paint_quad(
            builder,
            accent,
            self.metrics.rail_frame,
            self.accent,
            self.rail_border,
        );
        paint_quad(
            builder,
            background,
            self.metrics.rail_frame,
            self.background,
            self.rail_border,
        );
        paint_quad(
            builder,
            handle,
            self.metrics.handle_frame,
            self.handle,
            self.handle_border,
        );
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
        replay(
            &builder.finish(),
            &mut IcedBackend::new(&mut frame, self.resources),
        );
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

fn paint_quad(
    builder: &mut DrawListBuilder,
    bounds: Rect,
    frame: FrameSkin,
    fill: Rgba,
    border: Rgba,
) {
    builder.fill_rounded_rect(bounds, frame.radius, fill);
    if frame.border_width <= 0.0 {
        return;
    }
    let inset = frame.border_width / 2.0;
    builder.stroke_rounded_rect(
        Rect {
            h: (bounds.h - frame.border_width).max(0.0),
            w: (bounds.w - frame.border_width).max(0.0),
            x: bounds.x + inset,
            y: bounds.y + inset,
        },
        frame.radius,
        border,
        frame.border_width,
    );
}

#[cfg(test)]
mod tests {
    use iced::{Point, mouse};
    use kithara_test_utils::kithara;

    use super::*;
    use crate::{builtin, render::ControlAction};

    #[kithara::test]
    fn default_fader_paints_the_two_bordered_rails_then_the_handle() {
        let skin = builtin::skin();
        let paint = FaderPaint::new(0.5, skin);
        let mut actual = DrawListBuilder::default();
        paint.paint(
            &mut actual,
            Rect {
                h: 16.0,
                w: 200.0,
                x: 0.0,
                y: 0.0,
            },
        );

        let mut expected = DrawListBuilder::default();
        expected.fill_rect(
            Rect {
                h: 6.0,
                w: 100.0,
                x: 0.0,
                y: 5.0,
            },
            skin.palette.accent.into(),
        );
        expected.stroke_rounded_rect(
            Rect {
                h: 5.0,
                w: 99.0,
                x: 0.5,
                y: 5.5,
            },
            0.0,
            skin.rgba(skin.fader.rail_frame.border),
            1.0,
        );
        expected.fill_rect(
            Rect {
                h: 6.0,
                w: 100.0,
                x: 100.0,
                y: 5.0,
            },
            skin.palette.bg_deep.into(),
        );
        expected.stroke_rounded_rect(
            Rect {
                h: 5.0,
                w: 99.0,
                x: 100.5,
                y: 5.5,
            },
            0.0,
            skin.rgba(skin.fader.rail_frame.border),
            1.0,
        );
        expected.fill_rect(
            Rect {
                h: 16.0,
                w: 9.0,
                x: 95.5,
                y: 0.0,
            },
            skin.rgba(skin.fader.handle_color),
        );

        assert_eq!(actual.finish(), expected.finish());
    }

    #[kithara::test]
    fn leaf_fader_matches_the_iced_step_boundaries_exactly() {
        let bounds = Rectangle {
            height: 16.0,
            width: 200.0,
            x: 0.0,
            y: 0.0,
        };
        let press = Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left));

        for (x, expected) in [(29.0, 0.14), (30.0, 0.15), (31.0, 0.16)] {
            let paint = FaderPaint::new(0.5, builtin::skin());
            let program = FaderProgram::new("faders/default", paint, 0.01);
            let cursor = Cursor::Available(Point::new(x, 8.0));
            let mut state = ScalarState::default();
            let action = canvas::Program::update(&program, &mut state, &press, bounds, cursor)
                .unwrap_or_else(|| panic!("rail x {x} must publish"));

            assert_eq!(
                action.into_inner().0,
                Some(UiEvent::Control {
                    path: "faders/default".to_owned(),
                    action: ControlAction::SetScalar(expected),
                }),
                "rail x {x}"
            );
        }
    }
}
