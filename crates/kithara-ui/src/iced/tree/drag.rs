use std::{any::Any, cell::RefCell};

use iced::{
    Element, Event, Length, Rectangle, Renderer, Size, Theme, Vector,
    advanced::{
        Clipboard, Renderer as _, Shell, Widget as IcedWidget,
        layout::{self, Layout},
        mouse::{self, Cursor},
        overlay, renderer,
        widget::{self, Operation, Tree, tree::Tag},
    },
};

use super::host::Zone;
use crate::{
    hosts::{
        drag::{Carried, DragSession},
        window::DragGhost,
    },
    iced::{layer::draw_host_layer, table::carried},
    render::{Published, Skin},
    shaping::TextContext,
};

pub(super) struct DragRoot<'a> {
    child: Element<'a, Published>,
    skin: &'a Skin,
}

pub(super) fn drag_root<'a>(
    child: Element<'a, Published>,
    skin: &'a Skin,
) -> Element<'a, Published> {
    Element::new(DragRoot { child, skin })
}

pub(super) struct Root {
    pub(super) session: DragSession,
    ghost: DragGhost,
    hovered: Option<String>,
    text: RefCell<Option<TextContext>>,
}

impl Root {
    fn follow(
        &mut self,
        shell: &mut Shell<'_, Published>,
        update: impl FnOnce(&mut Shell<'_, Published>),
        mut operate: impl FnMut(&mut dyn Operation),
    ) {
        let mut published = Vec::new();
        let mut local = Shell::new(&mut published);
        update(&mut local);
        if local.is_event_captured() {
            shell.capture_event();
        }
        if local.is_layout_invalid() {
            shell.invalidate_layout();
        }
        if local.are_widgets_invalid() {
            shell.invalidate_widgets();
        }
        shell.request_redraw_at(local.redraw_request());
        shell.input_method_mut().merge(local.input_method());
        drop(local);
        let mut followed = false;
        for event in published {
            if !matches!(event, Published::Carry { .. }) {
                shell.publish(event);
                continue;
            }
            followed = true;
            let dropped = self.session.follow(&event, |table, index| {
                let mut lookup = Lookup {
                    table,
                    index,
                    found: None,
                };
                operate(&mut lookup);
                lookup.found
            });
            if let Some(dropped) = dropped {
                shell.publish(dropped);
            }
        }
        if !followed {
            return;
        }
        self.ghost.carry(self.session.label());
        shell.request_redraw();
    }
}

struct DragOverlay<'a> {
    overlay: overlay::Element<'a, Published, Theme, Renderer>,
    root: &'a mut Root,
}

impl overlay::Overlay<Published, Theme, Renderer> for DragOverlay<'_> {
    delegate::delegate! {
        to self.overlay.as_overlay_mut() {
            fn layout(&mut self, renderer: &Renderer, bounds: Size) -> layout::Node;
            fn operate(
                &mut self,
                layout: Layout<'_>,
                renderer: &Renderer,
                operation: &mut dyn Operation,
            );
        }
        to self.overlay.as_overlay() {
            fn draw(
                &self,
                renderer: &mut Renderer,
                theme: &Theme,
                style: &renderer::Style,
                layout: Layout<'_>,
                cursor: Cursor,
            );
            fn mouse_interaction(
                &self,
                layout: Layout<'_>,
                cursor: Cursor,
                renderer: &Renderer,
            ) -> mouse::Interaction;
            fn index(&self) -> f32;
        }
    }

    fn update(
        &mut self,
        event: &Event,
        layout: Layout<'_>,
        cursor: Cursor,
        renderer: &Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Published>,
    ) {
        let overlay = RefCell::new(&mut self.overlay);
        self.root.follow(
            shell,
            |local| {
                overlay
                    .borrow_mut()
                    .as_overlay_mut()
                    .update(event, layout, cursor, renderer, clipboard, local);
            },
            |operation| {
                overlay
                    .borrow_mut()
                    .as_overlay_mut()
                    .operate(layout, renderer, operation);
            },
        );
    }

    fn overlay<'b>(
        &'b mut self,
        layout: Layout<'b>,
        renderer: &Renderer,
    ) -> Option<overlay::Element<'b, Published, Theme, Renderer>> {
        let overlay = self.overlay.as_overlay_mut().overlay(layout, renderer)?;
        Some(overlay::Element::new(Box::new(DragOverlay {
            overlay,
            root: &mut *self.root,
        })))
    }
}

struct Lookup<'p> {
    table: &'p str,
    index: usize,
    found: Option<Carried>,
}

impl Operation for Lookup<'_> {
    fn custom(&mut self, _id: Option<&widget::Id>, _bounds: Rectangle, state: &mut dyn Any) {
        if self.found.is_none() {
            self.found = carried(state, self.table, self.index);
        }
    }

    fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation)) {
        if self.found.is_none() {
            operate(self);
        }
    }
}

struct Hover<'p> {
    zone: Option<&'p str>,
}

impl Operation for Hover<'_> {
    fn custom(&mut self, _id: Option<&widget::Id>, _bounds: Rectangle, state: &mut dyn Any) {
        if let Some(zone) = state.downcast_mut::<Zone>() {
            zone.hover(self.zone);
        }
    }

    fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation)) {
        operate(self);
    }
}

impl IcedWidget<Published, Theme, Renderer> for DragRoot<'_> {
    fn children(&self) -> Vec<Tree> {
        vec![Tree::new(&self.child)]
    }

    fn diff(&self, tree: &mut Tree) {
        let root = tree.state.downcast_mut::<Root>();
        root.ghost = DragGhost::new(root.session.label(), self.skin);
        tree.diff_children(std::slice::from_ref(&self.child));
    }

    fn state(&self) -> widget::tree::State {
        widget::tree::State::new(Root {
            session: DragSession::default(),
            ghost: DragGhost::new(None, self.skin),
            hovered: None,
            text: RefCell::new(None),
        })
    }

    fn tag(&self) -> Tag {
        Tag::of::<Root>()
    }

    fn layout(
        &mut self,
        tree: &mut Tree,
        renderer: &Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        let Tree {
            state, children, ..
        } = tree;
        let node = self
            .child
            .as_widget_mut()
            .layout(&mut children[0], renderer, limits);
        let root = state.downcast_mut::<Root>();
        if root.session.hovered() != root.hovered.as_deref() {
            root.hovered = root.session.hovered().map(str::to_owned);
            let mut hover = Hover {
                zone: root.hovered.as_deref(),
            };
            self.child.as_widget_mut().operate(
                &mut children[0],
                Layout::new(&node),
                renderer,
                &mut hover,
            );
        }
        node
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut Renderer,
        theme: &Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: Cursor,
        viewport: &Rectangle,
    ) {
        self.child.as_widget().draw(
            &tree.children[0],
            renderer,
            theme,
            style,
            layout,
            cursor,
            viewport,
        );
        let root = tree.state.downcast_ref::<Root>();
        if root.session.label().is_some() {
            let mut text = root.text.borrow_mut();
            let text = text.get_or_insert_with(|| self.skin.text_resources().into());
            let layer = root.ghost.layer(
                cursor.position().map(Into::into),
                layout.bounds().into(),
                text,
            );
            renderer.with_layer(layout.bounds(), |renderer| {
                draw_host_layer(renderer, &layer, self.skin.text_resources());
            });
        }
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: Cursor,
        renderer: &Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Published>,
        viewport: &Rectangle,
    ) {
        let Tree {
            state, children, ..
        } = tree;
        let root = state.downcast_mut::<Root>();
        if root.session.label().is_some()
            && matches!(event, Event::Mouse(mouse::Event::CursorMoved { .. }))
        {
            shell.request_redraw();
        }
        let child = RefCell::new((&mut self.child, &mut children[0]));
        root.follow(
            shell,
            |local| {
                let (child, tree) = &mut *child.borrow_mut();
                child.as_widget_mut().update(
                    tree, event, layout, cursor, renderer, clipboard, local, viewport,
                );
            },
            |operation| {
                let (child, tree) = &mut *child.borrow_mut();
                child
                    .as_widget_mut()
                    .operate(tree, layout, renderer, operation);
            },
        );
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout<'_>,
        cursor: Cursor,
        viewport: &Rectangle,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        self.child.as_widget().mouse_interaction(
            &tree.children[0],
            layout,
            cursor,
            viewport,
            renderer,
        )
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout<'_>,
        renderer: &Renderer,
        operation: &mut dyn Operation,
    ) {
        self.child
            .as_widget_mut()
            .operate(&mut tree.children[0], layout, renderer, operation);
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout<'b>,
        renderer: &Renderer,
        viewport: &Rectangle,
        translation: Vector,
    ) -> Option<overlay::Element<'b, Published, Theme, Renderer>> {
        let Tree {
            state, children, ..
        } = tree;
        let overlay = self.child.as_widget_mut().overlay(
            &mut children[0],
            layout,
            renderer,
            viewport,
            translation,
        )?;
        Some(overlay::Element::new(Box::new(DragOverlay {
            overlay,
            root: state.downcast_mut::<Root>(),
        })))
    }

    delegate::delegate! {
        to self.child.as_widget() {
            fn size(&self) -> Size<Length>;
            fn size_hint(&self) -> Size<Length>;
        }
    }
}
