use iced::{
    Element, Event, Length, Point, Rectangle, Renderer, Size, Theme,
    advanced::{
        Clipboard, Shell, Widget as IcedWidget,
        layout::{self, Layout},
        mouse, renderer,
        widget::{Tree, tree},
    },
};

use crate::{
    draw::Pt,
    interact::{Propagation, iced as iced_interact, recognizers::Carry},
    render::{ControlAction, Snap, UiEvent, control_event},
};

/// Puts one child of a stage at a point in it, and lets the pointer carry it
/// where the document says it may be carried.
pub(crate) fn placed<'a>(
    path: String,
    at: Pt,
    carried: bool,
    snap: Option<Snap>,
    child: Element<'a, UiEvent>,
) -> Element<'a, UiEvent> {
    Element::new(Placed {
        child,
        snap,
        at,
        path,
        carried,
    })
}

struct Placed<'a> {
    child: Element<'a, UiEvent>,
    snap: Option<Snap>,
    at: Pt,
    path: String,
    carried: bool,
}

impl Placed<'_> {
    /// The box the child ended up in, which is what the pointer is tested
    /// against: the placement itself is the whole stage.
    fn child_bounds(layout: Layout<'_>) -> Rectangle {
        layout
            .children()
            .next()
            .map_or_else(|| layout.bounds(), |child| child.bounds())
    }

    /// Where a corner in the window lands in the stage that holds it.
    fn in_stage(corner: Pt, layout: Layout<'_>) -> Pt {
        let origin = layout.bounds();
        Pt {
            x: corner.x - origin.x,
            y: corner.y - origin.y,
        }
    }
}

impl IcedWidget<UiEvent, Theme, Renderer> for Placed<'_> {
    fn children(&self) -> Vec<Tree> {
        vec![Tree::new(&self.child)]
    }

    fn diff(&self, tree: &mut Tree) {
        tree.diff_children(std::slice::from_ref(&self.child));
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut Renderer,
        theme: &Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        let Some(child) = layout.children().next() else {
            return;
        };
        self.child.as_widget().draw(
            &tree.children[0],
            renderer,
            theme,
            style,
            child,
            cursor,
            viewport,
        );
    }

    fn layout(
        &mut self,
        tree: &mut Tree,
        renderer: &Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        let room = limits.max();
        let child = self
            .child
            .as_widget_mut()
            .layout(
                &mut tree.children[0],
                renderer,
                &layout::Limits::new(Size::ZERO, room),
            )
            .move_to(Point::new(self.at.x, self.at.y));
        layout::Node::with_children(room, vec![child])
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        let carry = tree.state.downcast_ref::<Carry>();
        if self.carried && (carry.is_carried() || cursor.is_over(Self::child_bounds(layout))) {
            return carry.cursor().into();
        }
        let Some(child) = layout.children().next() else {
            return mouse::Interaction::default();
        };
        self.child.as_widget().mouse_interaction(
            &tree.children[0],
            child,
            cursor,
            viewport,
            renderer,
        )
    }

    /// A placement fills the stage it is in and puts its child inside that,
    /// which is what lets the child stand anywhere in the scene rather than
    /// only where a container would have put it.
    fn size(&self) -> Size<Length> {
        Size::new(Length::Fill, Length::Fill)
    }

    fn state(&self) -> tree::State {
        tree::State::new(Carry::default())
    }

    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<Carry>()
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, UiEvent>,
        viewport: &Rectangle,
    ) {
        if let Some(child) = layout.children().next() {
            self.child.as_widget_mut().update(
                &mut tree.children[0],
                event,
                child,
                cursor,
                renderer,
                clipboard,
                shell,
                viewport,
            );
        }
        if !self.carried || shell.is_event_captured() {
            return;
        }
        let Some(input) = iced_interact::input(event) else {
            return;
        };
        let hit = iced_interact::hit(Self::child_bounds(layout), cursor);
        let outcome = tree.state.downcast_mut::<Carry>().on_input(input, &hit);
        if let Some(corner) = outcome.value() {
            let at = Self::in_stage(corner, layout);
            let at = self.snap.as_ref().map_or(at, |snap| snap.take(at));
            shell.publish(control_event(&self.path, ControlAction::Place(at)));
        }
        if outcome.propagation() == Propagation::Captured {
            shell.capture_event();
        }
    }
}
