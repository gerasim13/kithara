use std::cell::RefCell;

use iced::{
    Event, Renderer, Size, Theme,
    advanced::{
        Clipboard, Shell,
        layout::{Layout, Node},
        mouse, overlay,
        overlay::{Element, Nested},
        renderer,
        widget::Operation,
    },
};

use crate::render::UiEvent;

pub(crate) fn hosted_picker_overlay<'a>(
    child: Element<'a, UiEvent, Theme, Renderer>,
    route: impl for<'b> FnMut(&Event, mouse::Cursor, &mut Shell<'b, UiEvent>) -> bool + 'a,
) -> Element<'a, UiEvent, Theme, Renderer> {
    Element::new(Box::new(HostedPickerPortal {
        route,
        child: RefCell::new(Nested::new(child)),
    }))
}

struct HostedPickerPortal<'a, F> {
    route: F,
    child: RefCell<Nested<'a, UiEvent, Theme, Renderer>>,
}

impl<F> overlay::Overlay<UiEvent, Theme, Renderer> for HostedPickerPortal<'_, F>
where
    F: for<'a> FnMut(&Event, mouse::Cursor, &mut Shell<'a, UiEvent>) -> bool,
{
    fn draw(
        &self,
        _renderer: &mut Renderer,
        _theme: &Theme,
        _style: &renderer::Style,
        _layout: Layout<'_>,
        _cursor: mouse::Cursor,
    ) {
    }

    fn layout(&mut self, _renderer: &Renderer, _bounds: Size) -> Node {
        Node::new(Size::ZERO)
    }

    fn overlay<'a>(
        &'a mut self,
        _layout: Layout<'a>,
        _renderer: &Renderer,
    ) -> Option<Element<'a, UiEvent, Theme, Renderer>> {
        Some(Element::new(Box::new(HostedPickerLayer {
            child: &self.child,
            route: &mut self.route,
        })))
    }
}

struct HostedPickerLayer<'a, 'child, F> {
    route: &'a mut F,
    child: &'a RefCell<Nested<'child, UiEvent, Theme, Renderer>>,
}

impl<F> overlay::Overlay<UiEvent, Theme, Renderer> for HostedPickerLayer<'_, '_, F>
where
    F: for<'a> FnMut(&Event, mouse::Cursor, &mut Shell<'a, UiEvent>) -> bool,
{
    fn update(
        &mut self,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, UiEvent>,
    ) {
        if (self.route)(event, cursor, shell) {
            return;
        }
        self.child
            .borrow_mut()
            .update(event, layout, cursor, renderer, clipboard, shell);
    }

    delegate::delegate! {
        to self.child.borrow_mut() {
            fn layout(&mut self, renderer: &Renderer, bounds: Size) -> Node;
            fn mouse_interaction(
                &self,
                layout: Layout<'_>,
                cursor: mouse::Cursor,
                renderer: &Renderer,
            ) -> mouse::Interaction;
            fn draw(
                &self,
                renderer: &mut Renderer,
                theme: &Theme,
                style: &renderer::Style,
                layout: Layout<'_>,
                cursor: mouse::Cursor,
            );
            fn operate(&mut self, layout: Layout<'_>, renderer: &Renderer, operation: &mut dyn Operation);
        }
    }
}
