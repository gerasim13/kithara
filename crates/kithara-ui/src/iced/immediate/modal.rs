use iced::{
    Border, Color, Element, Event, Length, Point, Rectangle, Renderer, Shadow, Size, Theme, Vector,
    advanced::{
        Clipboard, Layout, Renderer as _, Shell, Widget,
        layout::{Limits, Node},
        mouse::{Cursor, Interaction},
        overlay,
        overlay::Group,
        renderer::{self, Quad},
        widget::{Operation, Tree},
    },
    keyboard::{self, Key, key::Named},
};

use crate::{
    draw::Rect,
    hosts::{layer::ModalChrome, solve},
    render::Skin,
};

/// A modal: nothing in flow, and while open a scrim over the whole window
/// with the content centred above it, taking every input.
pub(crate) struct Modal<'a, Message> {
    content: Element<'a, Message>,
    on_close: Message,
    chrome: ModalChrome,
    open: bool,
}

impl<'a, Message> Modal<'a, Message> {
    pub(crate) fn new(
        content: Element<'a, Message>,
        open: bool,
        on_close: Message,
        skin: &Skin,
    ) -> Self {
        Self {
            content,
            on_close,
            open,
            chrome: ModalChrome::new(skin),
        }
    }
}

impl<Message> Widget<Message, Theme, Renderer> for Modal<'_, Message>
where
    Message: Clone,
{
    fn children(&self) -> Vec<Tree> {
        vec![Tree::new(&self.content)]
    }

    fn diff(&self, tree: &mut Tree) {
        tree.diff_children(&[self.content.as_widget()]);
    }

    fn draw(
        &self,
        _tree: &Tree,
        _renderer: &mut Renderer,
        _theme: &Theme,
        _style: &renderer::Style,
        _layout: Layout<'_>,
        _cursor: Cursor,
        _viewport: &Rectangle,
    ) {
    }

    fn layout(&mut self, _tree: &mut Tree, _renderer: &Renderer, _limits: &Limits) -> Node {
        Node::new(Size::ZERO)
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        _layout: Layout<'b>,
        _renderer: &Renderer,
        _viewport: &Rectangle,
        _translation: Vector,
    ) -> Option<overlay::Element<'b, Message, Theme, Renderer>> {
        if !self.open {
            return None;
        }
        let [content_tree] = tree.children.as_mut_slice() else {
            return None;
        };
        Some(overlay::Element::new(Box::new(Above(Surface {
            content: &mut self.content,
            tree: content_tree,
            on_close: self.on_close.clone(),
            chrome: self.chrome,
        }))))
    }

    fn size(&self) -> Size<Length> {
        Size::new(Length::Shrink, Length::Shrink)
    }
}

impl<'a, Message> From<Modal<'a, Message>> for Element<'a, Message>
where
    Message: Clone + 'a,
{
    fn from(modal: Modal<'a, Message>) -> Self {
        Self::new(modal)
    }
}

/// Holds the surface one overlay level above the floating layers of the page,
/// so it draws over them and takes input before them whatever their order.
struct Above<'a, 'b, Message>(Surface<'a, 'b, Message>);

impl<Message> overlay::Overlay<Message, Theme, Renderer> for Above<'_, '_, Message>
where
    Message: Clone,
{
    fn draw(
        &self,
        _renderer: &mut Renderer,
        _theme: &Theme,
        _style: &renderer::Style,
        _layout: Layout<'_>,
        _cursor: Cursor,
    ) {
    }

    fn layout(&mut self, _renderer: &Renderer, bounds: Size) -> Node {
        Node::new(bounds)
    }

    fn overlay<'c>(
        &'c mut self,
        _layout: Layout<'c>,
        _renderer: &Renderer,
    ) -> Option<overlay::Element<'c, Message, Theme, Renderer>> {
        let Surface {
            content,
            tree,
            on_close,
            chrome,
        } = &mut self.0;
        let surface = overlay::Element::new(Box::new(Surface {
            content: &mut **content,
            tree,
            on_close: on_close.clone(),
            chrome: *chrome,
        }));
        Some(Group::with_children(vec![surface]).overlay())
    }
}

struct Surface<'a, 'b, Message> {
    content: &'b mut Element<'a, Message>,
    tree: &'b mut Tree,
    on_close: Message,
    chrome: ModalChrome,
}

fn rectangle(rect: Rect) -> Rectangle {
    Rectangle::new(Point::new(rect.x, rect.y), Size::new(rect.w, rect.h))
}

impl<Message> Surface<'_, '_, Message> {
    fn surface(&self, layout: Layout<'_>) -> Rectangle {
        layout
            .children()
            .next()
            .map_or_else(Rectangle::default, |content| {
                content.bounds().expand(self.chrome.border_width)
            })
    }
}

impl<Message> overlay::Overlay<Message, Theme, Renderer> for Surface<'_, '_, Message>
where
    Message: Clone,
{
    fn draw(
        &self,
        renderer: &mut Renderer,
        theme: &Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: Cursor,
    ) {
        let chrome = self.chrome;
        let surface = self.surface(layout);
        renderer.fill_quad(
            Quad {
                bounds: layout.bounds(),
                ..Quad::default()
            },
            Color::from(chrome.scrim),
        );
        renderer.fill_quad(
            Quad {
                bounds: surface,
                border: Border {
                    color: chrome.border.into(),
                    width: chrome.border_width,
                    radius: chrome.radius.into(),
                },
                shadow: Shadow {
                    color: chrome.shadow.into(),
                    offset: Vector::new(chrome.shadow_offset.x, chrome.shadow_offset.y),
                    blur_radius: chrome.blur,
                },
                ..Quad::default()
            },
            Color::from(chrome.background),
        );
        if let Some(content) = layout.children().next() {
            self.content
                .as_widget()
                .draw(self.tree, renderer, theme, style, content, cursor, &surface);
        }
        for tick in chrome.ticks(Rect::from(surface)) {
            renderer.fill_quad(
                Quad {
                    bounds: rectangle(tick),
                    ..Quad::default()
                },
                Color::from(chrome.tick),
            );
        }
    }

    fn layout(&mut self, renderer: &Renderer, bounds: Size) -> Node {
        let viewport = solve::Size::new(bounds.width, bounds.height);
        let room = self.chrome.room(viewport);
        let content = self.content.as_widget_mut().layout(
            self.tree,
            renderer,
            &Limits::new(Size::ZERO, Size::new(room.width, room.height)),
        );
        let size = content.size();
        let surface = self
            .chrome
            .surface(solve::Size::new(size.width, size.height), viewport);
        let at = self.chrome.content(surface);
        Node::with_children(bounds, vec![content.move_to(Point::from(at))])
    }

    fn mouse_interaction(
        &self,
        layout: Layout<'_>,
        cursor: Cursor,
        renderer: &Renderer,
    ) -> Interaction {
        let surface = self.surface(layout);
        layout
            .children()
            .next()
            .map_or(Interaction::None, |content| {
                self.content
                    .as_widget()
                    .mouse_interaction(self.tree, content, cursor, &surface, renderer)
            })
            .max(Interaction::Idle)
    }

    fn operate(&mut self, layout: Layout<'_>, renderer: &Renderer, operation: &mut dyn Operation) {
        if let Some(content) = layout.children().next() {
            self.content
                .as_widget_mut()
                .operate(self.tree, content, renderer, operation);
        }
    }

    fn update(
        &mut self,
        event: &Event,
        layout: Layout<'_>,
        cursor: Cursor,
        renderer: &Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
    ) {
        let surface = self.surface(layout);
        if let Some(content) = layout.children().next() {
            self.content.as_widget_mut().update(
                self.tree, event, content, cursor, renderer, clipboard, shell, &surface,
            );
        }
        if shell.is_event_captured() {
            return;
        }
        if matches!(
            event,
            Event::Keyboard(keyboard::Event::KeyPressed {
                key: Key::Named(Named::Escape),
                ..
            })
        ) {
            shell.publish(self.on_close.clone());
        }
        if matches!(
            event,
            Event::Mouse(_)
                | Event::Touch(_)
                | Event::Keyboard(
                    keyboard::Event::KeyPressed { .. } | keyboard::Event::KeyReleased { .. }
                )
                | Event::InputMethod(_)
        ) {
            shell.capture_event();
        }
    }
}
