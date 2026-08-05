use std::cell::RefCell;

use iced::{
    Element, Event, Length, Rectangle, Renderer, Theme,
    mouse::{Cursor, Interaction},
    widget::canvas::{self, Action, Canvas, Frame, Geometry},
};
use kithara_platform::time::Instant;

use crate::{
    atoms::knob::Knob,
    backends::replay_ordered,
    draw::{DrawList, DrawListBuilder, Rect},
    interact::{
        CursorShape, Hover, iced as iced_interact,
        recognizers::{Scalar, ScalarState, Track, WheelStep},
    },
    render::{Skin, UiEvent, scalar},
    text::{TextContext, TextResources},
};

pub(crate) struct KnobProgram<'data, 'skin> {
    paint: KnobPaint<'data, 'skin>,
    drag: Scalar,
    path: String,
}

impl<'data, 'skin> KnobProgram<'data, 'skin> {
    pub(crate) fn new(
        path: &str,
        label: Option<&'data str>,
        value: f32,
        skin: &'skin Skin,
    ) -> Self {
        const RESET_VALUE: f32 = 0.5;

        let metrics = skin.knob;
        Self {
            paint: KnobPaint::new(label, value, skin),
            drag: Scalar::builder()
                .track(Track::RelativeVertical {
                    range: metrics.drag_range,
                    value,
                })
                .hover(Hover::new(CursorShape::ResizeV))
                .reset(RESET_VALUE)
                .wheel(WheelStep {
                    value,
                    step: metrics.wheel_step,
                })
                .build(),
            path: path.to_owned(),
        }
    }

    pub(crate) fn view(self) -> Element<'skin, UiEvent>
    where
        'data: 'skin,
    {
        Canvas::new(self)
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }
}

impl canvas::Program<UiEvent> for KnobProgram<'_, '_> {
    type State = KnobState;

    fn draw(
        &self,
        state: &KnobState,
        renderer: &Renderer,
        theme: &Theme,
        bounds: Rectangle,
        cursor: Cursor,
    ) -> Vec<Geometry> {
        self.paint
            .draw(&state.paint, renderer, theme, bounds, cursor)
    }

    fn mouse_interaction(
        &self,
        state: &KnobState,
        bounds: Rectangle,
        cursor: Cursor,
    ) -> Interaction {
        self.drag
            .cursor(&state.drag, &iced_interact::hit(bounds, cursor))
            .into()
    }

    fn update(
        &self,
        state: &mut KnobState,
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
pub(crate) struct KnobState {
    drag: ScalarState,
    paint: KnobPaintState,
}

pub(crate) struct KnobPaint<'data, 'skin> {
    knob: Knob,
    label: Option<&'data str>,
    text_resources: &'skin TextResources,
    value: f32,
}

impl<'data, 'skin> KnobPaint<'data, 'skin> {
    pub(crate) fn new(label: Option<&'data str>, value: f32, skin: &'skin Skin) -> Self {
        Self {
            knob: Knob::new(skin),
            label,
            text_resources: skin.text_resources(),
            value,
        }
    }

    pub(crate) fn view(self) -> Element<'skin, UiEvent>
    where
        'data: 'skin,
    {
        Canvas::new(self)
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }
}

impl canvas::Program<UiEvent> for KnobPaint<'_, '_> {
    type State = KnobPaintState;

    fn draw(
        &self,
        state: &KnobPaintState,
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: Cursor,
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
        replay_ordered(&list, &mut frame, self.text_resources);
        vec![frame.into_geometry()]
    }
}

#[derive(Default)]
pub(crate) struct KnobPaintState {
    text: RefCell<Option<TextContext>>,
}

impl KnobPaint<'_, '_> {
    pub(crate) fn draw_list(&self, state: &KnobPaintState, bounds: Rect) -> DrawList {
        let mut text = state.text.borrow_mut();
        let text = text.get_or_insert_with(|| self.text_resources.into());
        let mut builder = DrawListBuilder::default();
        self.knob
            .paint(&mut builder, text, self.value, self.label, bounds);
        builder.finish()
    }
}

#[cfg(all(test, feature = "masonry-host"))]
mod tests {
    use kithara_test_utils::kithara;

    use super::*;
    use crate::{builtin, ids::SourceUri, render::masonry::MasonryControl};

    #[kithara::test]
    fn iced_and_masonry_record_the_same_labelled_knob() {
        let origin = SourceUri("knob.kskin.ron".to_owned());
        let skin = Skin::resolve(builtin::skin_doc().clone(), &origin).unwrap();
        let bounds = Rect {
            h: 39.0,
            w: 28.0,
            x: 0.0,
            y: 0.0,
        };
        let iced =
            KnobPaint::new(Some("GAIN"), 0.25, &skin).draw_list(&KnobPaintState::default(), bounds);
        let mut masonry =
            crate::render::masonry::MasonryKnob::new(Some("GAIN".to_owned()), 0.25, &skin);

        assert_eq!(iced, MasonryControl::draw_list(&mut masonry, bounds));
    }
}
