use iced::{
    Event, Point, Rectangle, Renderer, Size, Theme, Vector,
    advanced::{
        Clipboard, Renderer as _, Shell,
        graphics::geometry::Renderer as _,
        layout::{self, Layout},
        mouse, overlay, renderer,
    },
    event,
    widget::canvas::Frame,
};
use kithara_platform::time::Instant;
use num_traits::cast::AsPrimitive;

use super::{paint::PickerPaint, program::targets, widget::PickerState};
use crate::{
    backends::IcedBackend,
    draw::replay,
    interact::iced as iced_interact,
    render::{InputOwner, UiEvent, engine as engine_event},
};

pub(super) struct PickerOverlay<'a, 'b> {
    pub(super) anchor: Rectangle,
    pub(super) owner: InputOwner,
    pub(super) paint: &'a PickerPaint<'a>,
    pub(super) path: &'a str,
    pub(super) state: &'b mut PickerState,
}

impl overlay::Overlay<UiEvent, Theme, Renderer> for PickerOverlay<'_, '_> {
    fn layout(&mut self, _renderer: &Renderer, _bounds: Size) -> layout::Node {
        layout::Node::new(Size::new(
            self.anchor.width,
            self.paint.item_height() * AsPrimitive::<f32>::as_(self.paint.item_count()),
        ))
        .move_to(Point::new(
            self.anchor.x,
            self.anchor.y + self.anchor.height,
        ))
    }

    fn update(
        &mut self,
        event: &Event,
        _layout: Layout<'_>,
        cursor: mouse::Cursor,
        _renderer: &Renderer,
        _clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, UiEvent>,
    ) {
        if matches!(self.owner, InputOwner::Engine) {
            return;
        }
        let Some(input) = iced_interact::input(event) else {
            return;
        };
        let before = self.state.snapshot();
        let targets = targets(
            self.path,
            self.anchor,
            cursor,
            true,
            self.paint.item_count(),
            self.paint.item_height(),
        );
        let emission = self
            .state
            .engine_mut()
            .and_then(|engine| engine.handle(input, &targets, Instant::now()));
        self.state.refresh(self.path);
        if let Some(action) = emission
            .and_then(|emission| engine_event(&emission.path, emission.child, emission.outcome))
        {
            let (message, redraw, status) = action.into_inner();
            shell.request_redraw_at(redraw);
            if let Some(message) = message {
                shell.publish(message);
            }
            if status == event::Status::Captured {
                shell.capture_event();
            }
        }
        if before != self.state.snapshot() {
            shell.request_redraw();
            shell.invalidate_layout();
        }
    }

    fn mouse_interaction(
        &self,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        _renderer: &Renderer,
    ) -> mouse::Interaction {
        match self.owner {
            InputOwner::Leaf if cursor.is_over(layout.bounds()) => mouse::Interaction::Pointer,
            InputOwner::Leaf | InputOwner::Engine => mouse::Interaction::None,
        }
    }

    fn draw(
        &self,
        renderer: &mut Renderer,
        _theme: &Theme,
        _style: &renderer::Style,
        layout: Layout<'_>,
        _cursor: mouse::Cursor,
    ) {
        let bounds = layout.bounds();
        if bounds.width < 1.0 || bounds.height < 1.0 {
            return;
        }
        renderer.with_translation(Vector::new(bounds.x, bounds.y), |renderer| {
            let mut frame = Frame::new(renderer, bounds.size());
            let mut text = self.state.text().borrow_mut();
            let text = text.get_or_insert_with(|| self.paint.skin().text_resources().into());
            let commands =
                self.paint
                    .popup_commands(text, bounds.width, self.state.snapshot().highlighted);
            replay(
                &commands,
                &mut IcedBackend::new(&mut frame, self.paint.skin().text_resources()),
            );
            renderer.draw_geometry(frame.into_geometry());
        });
    }
}
