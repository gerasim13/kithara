use std::cell::RefCell;

use iced::{
    Element, Event, Length, Rectangle, Renderer, Theme,
    mouse::{Cursor, Interaction},
    widget::canvas::{self, Action, Canvas, Frame, Geometry},
};
use kithara_platform::time::Instant;

use crate::{
    atoms::knob::Knob,
    backends::IcedBackend,
    draw::{DrawListBuilder, Rect, replay},
    interact::{
        CursorShape, Hover, Outcome, iced as iced_interact,
        recognizers::{Scalar, ScalarState, WheelStep},
    },
    render::{ControlAction, Skin, UiEvent},
    skin::KnobSkin,
    text::{TextContext, TextResources},
};

pub(super) struct KnobProgram<'data, 'skin> {
    knob: Knob<'data, 'skin>,
    metrics: KnobSkin,
    drag: Scalar,
    path: String,
    text_resources: &'skin TextResources,
}

impl<'data, 'skin> KnobProgram<'data, 'skin> {
    pub(super) fn new(
        path: &str,
        label: Option<&'data str>,
        value: f32,
        skin: &'skin Skin,
    ) -> Self {
        const RESET_VALUE: f32 = 0.5;

        let metrics = skin.knob;
        Self {
            knob: Knob::new(label, value, skin),
            metrics,
            drag: Scalar::builder()
                .value(value)
                .range(metrics.drag_range)
                .hover(Hover::new(CursorShape::ResizeV))
                .reset(RESET_VALUE)
                .wheel(WheelStep {
                    value,
                    step: metrics.wheel_step,
                })
                .build(),
            path: path.to_owned(),
            text_resources: skin.text_resources(),
        }
    }

    pub(super) fn view(self) -> Element<'skin, UiEvent>
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
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: Cursor,
    ) -> Vec<Geometry> {
        let mut frame = Frame::new(renderer, bounds.size());
        let (dial, caption) = self.rects(bounds);
        let mut text = state.text.borrow_mut();
        let text = text.get_or_insert_with(|| self.text_resources.into());
        let mut builder = DrawListBuilder::default();
        self.knob.paint(&mut builder, text, dial, caption);
        replay(
            &builder.finish(),
            &mut IcedBackend::new(&mut frame, self.text_resources),
        );
        vec![frame.into_geometry()]
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
                .on_input(&mut state.drag, input, &hit, Instant::now()),
        )
    }
}

#[derive(Default)]
pub(super) struct KnobState {
    drag: ScalarState,
    text: RefCell<Option<TextContext>>,
}

fn scalar(path: &str, outcome: Outcome) -> Option<Action<UiEvent>> {
    if let Some(value) = outcome.value() {
        return Some(
            Action::publish(UiEvent::Control {
                path: path.to_owned(),
                action: ControlAction::SetScalar(f64::from(value)),
            })
            .and_capture(),
        );
    }
    outcome.is_captured().then(Action::capture)
}

impl KnobProgram<'_, '_> {
    fn rects(&self, bounds: Rectangle) -> (Rect, Rect) {
        const DIAMETER_SCALE: f32 = 2.0;

        let metrics = self.metrics;
        let caption_height = metrics.label_height.min(bounds.height);
        let available = (bounds.height - caption_height).max(0.0);
        let gap = metrics.label_gap.min(available);
        let dial_height = available - gap;
        let inset = metrics
            .outer_inset
            .min(bounds.width / DIAMETER_SCALE)
            .min(dial_height / DIAMETER_SCALE);
        (
            Rect {
                h: dial_height - inset * DIAMETER_SCALE,
                w: bounds.width - inset * DIAMETER_SCALE,
                x: inset,
                y: inset,
            },
            Rect {
                h: caption_height,
                w: bounds.width,
                x: 0.0,
                y: dial_height + gap,
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use kithara_test_utils::kithara;

    use super::*;
    use crate::{builtin, ids::SourceUri};

    #[kithara::test]
    fn caption_row_and_outer_inset_are_carved_from_the_bounds() {
        let origin = SourceUri("knob.kskin.ron".to_owned());
        let skin = Skin::resolve(builtin::skin_doc().clone(), &origin).unwrap();
        let bounds = Rectangle {
            height: 39.0,
            width: 28.0,
            x: 0.0,
            y: 0.0,
        };
        let metrics = skin.knob;

        let program = KnobProgram::new("mixer/gain", Some("GAIN"), 0.5, &skin);
        let (dial, caption) = program.rects(bounds);
        let unlabelled = KnobProgram::new("mixer/gain", None, 0.5, &skin);

        assert_eq!(
            dial.h + metrics.outer_inset * 2.0 + metrics.label_gap + caption.h,
            bounds.height
        );
        assert_eq!(caption.h, metrics.label_height);
        assert!(caption.y >= dial.y + dial.h);
        assert_eq!(program.rects(bounds), unlabelled.rects(bounds));
    }
}
