use iced::{
    Element, Event, Length, Point, Rectangle, Renderer, Size, Theme, Vector,
    advanced::{
        Clipboard, Shell, Widget as IcedWidget,
        layout::{self, Layout},
        mouse, overlay, renderer,
        widget::{self, Tree},
    },
};

use crate::{
    draw::{Pt, Rect},
    interact::{Input, Outcome, iced as iced_interact},
    render::{HostLayer, UiEvent, WindowCommand, draw_host_layer, window as window_event},
    text::TextResources,
};

pub(crate) trait WindowLayerProgram {
    type State: Default + 'static;

    fn size(&self) -> Size<Length>;

    fn layer(
        &self,
        state: &Self::State,
        bounds: Rect,
        pointer: Option<Pt>,
    ) -> HostLayer<WindowCommand>;

    fn hit_layer(&self, state: &Self::State, bounds: Rect) -> HostLayer<WindowCommand> {
        self.layer(state, bounds, None)
    }

    fn update(
        &self,
        _state: &mut Self::State,
        input: Input<'_>,
        layer: &HostLayer<WindowCommand>,
        pointer: Option<Pt>,
    ) -> (Outcome<WindowCommand>, bool) {
        (layer.handle(input, pointer), false)
    }

    fn resources(&self) -> Option<&TextResources>;
}

pub(crate) fn window_layer<'a>(program: impl WindowLayerProgram + 'a) -> Element<'a, UiEvent> {
    Element::new(WindowLayerLeaf { program })
}

struct WindowLayerLeaf<P> {
    program: P,
}

impl<P> IcedWidget<UiEvent, Theme, Renderer> for WindowLayerLeaf<P>
where
    P: WindowLayerProgram,
{
    fn tag(&self) -> widget::tree::Tag {
        widget::tree::Tag::of::<P::State>()
    }

    fn state(&self) -> widget::tree::State {
        widget::tree::State::new(P::State::default())
    }

    fn size(&self) -> Size<Length> {
        self.program.size()
    }

    fn layout(
        &mut self,
        _tree: &mut Tree,
        _renderer: &Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        let size = self.size();
        layout::Node::new(limits.resolve(size.width, size.height, Size::ZERO))
    }

    fn draw(
        &self,
        _tree: &Tree,
        _renderer: &mut Renderer,
        _theme: &Theme,
        _style: &renderer::Style,
        _layout: Layout<'_>,
        _cursor: mouse::Cursor,
        _viewport: &Rectangle,
    ) {
    }

    fn overlay<'a>(
        &'a mut self,
        tree: &'a mut Tree,
        layout: Layout<'a>,
        _renderer: &Renderer,
        _viewport: &Rectangle,
        translation: Vector,
    ) -> Option<overlay::Element<'a, UiEvent, Theme, Renderer>> {
        Some(overlay::Element::new(Box::new(WindowLayerOverlay {
            bounds: (layout.bounds() + translation).into(),
            program: &self.program,
            state: tree.state.downcast_mut::<P::State>(),
        })))
    }
}

struct WindowLayerOverlay<'a, P>
where
    P: WindowLayerProgram,
{
    bounds: Rect,
    program: &'a P,
    state: &'a mut P::State,
}

impl<P> overlay::Overlay<UiEvent, Theme, Renderer> for WindowLayerOverlay<'_, P>
where
    P: WindowLayerProgram,
{
    fn layout(&mut self, _renderer: &Renderer, _bounds: Size) -> layout::Node {
        layout::Node::new(Size::new(self.bounds.w, self.bounds.h))
            .move_to(Point::new(self.bounds.x, self.bounds.y))
    }

    fn update(
        &mut self,
        event: &Event,
        _layout: Layout<'_>,
        pointer: mouse::Cursor,
        _renderer: &Renderer,
        _clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, UiEvent>,
    ) {
        let Some(input) = iced_interact::input(event) else {
            return;
        };
        let pointer = pointer.position().map(Into::into);
        let layer = self.program.hit_layer(self.state, self.bounds);
        let (outcome, redraw) = self.program.update(self.state, input, &layer, pointer);
        if redraw {
            shell.request_redraw();
        }
        let captured = outcome.is_captured();
        if let Some(command) = outcome.value() {
            if let Some(action) = window_event(command, Outcome::set(())) {
                let (message, redraw, status) = action.into_inner();
                shell.request_redraw_at(redraw);
                if let Some(message) = message {
                    shell.publish(message);
                }
                if status == iced::event::Status::Captured {
                    shell.capture_event();
                }
            }
        } else if captured {
            shell.capture_event();
        }
    }

    fn mouse_interaction(
        &self,
        _layout: Layout<'_>,
        pointer: mouse::Cursor,
        _renderer: &Renderer,
    ) -> mouse::Interaction {
        let pointer = pointer.position().map(Into::into);
        self.program
            .hit_layer(self.state, self.bounds)
            .cursor_at(pointer)
            .into()
    }

    fn draw(
        &self,
        renderer: &mut Renderer,
        _theme: &Theme,
        _style: &renderer::Style,
        _layout: Layout<'_>,
        pointer: mouse::Cursor,
    ) {
        let layer = self
            .program
            .layer(self.state, self.bounds, pointer.position().map(Into::into));
        if let Some(resources) = self.program.resources() {
            draw_host_layer(renderer, &layer, resources);
        }
    }
}

#[cfg(test)]
mod tests {
    use iced::{
        Pixels,
        advanced::{
            clipboard,
            layout::{Layout, Limits},
            widget::Tree,
        },
        mouse::{Button, Cursor},
    };
    use iced_renderer::fallback::Renderer as FallbackRenderer;
    use iced_tiny_skia::Renderer as TinySkiaRenderer;
    use kithara_test_utils::kithara;

    use super::*;
    use crate::{
        builtin,
        draw::DrawList,
        interact::CursorShape,
        render::{LayerHit, WindowCommand, fonts::SANS},
    };

    struct TestProgram {
        resources: TextResources,
    }

    impl WindowLayerProgram for TestProgram {
        type State = ();

        fn size(&self) -> Size<Length> {
            Size::new(Length::Fixed(20.0), Length::Fixed(10.0))
        }

        fn layer(
            &self,
            _state: &(),
            bounds: Rect,
            _pointer: Option<Pt>,
        ) -> HostLayer<WindowCommand> {
            HostLayer::new(
                bounds,
                DrawList::default(),
                vec![LayerHit::new(
                    bounds,
                    CursorShape::Grab,
                    WindowCommand::Drag,
                )],
            )
        }

        fn resources(&self) -> Option<&TextResources> {
            Some(&self.resources)
        }
    }

    #[kithara::test]
    fn a_window_layer_leaf_is_inert_below_and_emits_from_its_overlay() {
        let mut element = window_layer(TestProgram {
            resources: builtin::skin().text_resources().clone(),
        });
        let renderer = FallbackRenderer::Secondary(TinySkiaRenderer::new(SANS, Pixels(14.0)));
        let viewport = Size::new(100.0, 60.0);
        let bounds = Rectangle::with_size(viewport);
        let mut tree = Tree::new(element.as_widget());
        let node = element.as_widget_mut().layout(
            &mut tree,
            &renderer,
            &Limits::new(Size::ZERO, viewport),
        );
        assert_eq!(node.size(), Size::new(20.0, 10.0));

        let event = Event::Mouse(mouse::Event::ButtonPressed(Button::Left));
        let pointer = Cursor::Available(Point::new(5.0, 5.0));
        let mut clipboard = clipboard::Null;
        let mut messages = Vec::new();
        let mut shell = Shell::new(&mut messages);
        element.as_widget_mut().update(
            &mut tree,
            &event,
            Layout::new(&node),
            pointer,
            &renderer,
            &mut clipboard,
            &mut shell,
            &bounds,
        );
        assert!(!shell.is_event_captured());

        let mut overlay = element
            .as_widget_mut()
            .overlay(
                &mut tree,
                Layout::new(&node),
                &renderer,
                &bounds,
                Vector::ZERO,
            )
            .unwrap_or_else(|| panic!("a window layer leaf must expose an overlay"));
        let overlay_node = overlay.as_overlay_mut().layout(&renderer, viewport);
        overlay.as_overlay_mut().update(
            &event,
            Layout::new(&overlay_node),
            pointer,
            &renderer,
            &mut clipboard,
            &mut shell,
        );
        assert!(shell.is_event_captured());
        assert_eq!(
            overlay
                .as_overlay()
                .mouse_interaction(Layout::new(&overlay_node), pointer, &renderer,),
            mouse::Interaction::Grab
        );
        drop(shell);
        assert_eq!(messages, [UiEvent::Window(WindowCommand::Drag)]);
    }
}
