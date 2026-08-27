use std::cell::RefCell;

use iced::{
    Element, Event, Length, Rectangle, Renderer, Size as IcedSize, Theme,
    advanced::{
        Clipboard, Shell, Widget as IcedWidget,
        layout::{self, Layout},
        renderer,
        widget::{self, Tree},
    },
    mouse::Cursor,
    time::Instant as IcedInstant,
    window,
};

use crate::{
    draw::{DrawList, DrawListBuilder, Rect},
    interact::iced as iced_interact,
    render::{
        Skin, UiEvent,
        controls::{PaintState, Probe, snapped},
        custom::{CustomKinds, MountedCustom, Repaint, Size2, SizeLimits, TextMeasurer},
    },
};

/// One registered extension drawn straight into an iced canvas.
///
/// The widget owns its own state, so nothing outside it can say whether its
/// picture moved. It is built every frame and the tessellated geometry is
/// dropped only when the list that came out differs, which is the same bargain
/// the retained host takes for the same leaf.
pub(crate) struct Custom<'a> {
    kind: &'a str,
    kinds: Option<&'a CustomKinds>,
    skin: &'a Skin,
}

impl<'a> Custom<'a> {
    pub(crate) const fn new(kind: &'a str, kinds: Option<&'a CustomKinds>, skin: &'a Skin) -> Self {
        Self { kind, kinds, skin }
    }
}

/// A picture nothing outside the widget can key.
///
/// The probe never holds, so the list is built each frame; `Marks` then
/// compares the list itself, so a frame that drew the same thing still costs no
/// tessellation.
struct Redrawn;

impl Probe for Redrawn {
    type Key = ();

    fn holds(&self, _key: &Self::Key) -> bool {
        false
    }

    fn keep(self) -> Self::Key {}
}

struct CustomState {
    kind: String,
    widget: RefCell<Option<Box<dyn MountedCustom<UiEvent>>>>,
    paint: PaintState<()>,
    drawn: Option<IcedInstant>,
}

impl CustomState {
    fn new(kind: &str, kinds: Option<&CustomKinds>) -> Self {
        let widget = kinds.and_then(|kinds| kinds.make(kind));
        if widget.is_none() {
            // Compiling the document already refused an unregistered kind, so
            // reaching here means this host was handed a different registry
            // than the one that validated it.
            tracing::error!(kind, "no registered widget for this kind");
        }
        Self {
            kind: kind.to_owned(),
            widget: RefCell::new(widget),
            paint: PaintState::default(),
            drawn: None,
        }
    }

    fn repaint(&self) -> Repaint {
        self.widget
            .borrow()
            .as_ref()
            .map_or(Repaint::None, MountedCustom::repaint)
    }
}

impl Custom<'_> {
    /// The state for this leaf, rebuilt when the place it stands in changed
    /// hands: every custom leaf shares one state tag, so the kind is the only
    /// thing that says whether the widget behind it is still the right one.
    fn state_for<'s>(&self, tree: &'s mut Tree) -> &'s mut CustomState {
        let state = tree.state.downcast_mut::<CustomState>();
        if state.kind != self.kind {
            *state = CustomState::new(self.kind, self.kinds);
        }
        state
    }

    fn list(&self, state: &CustomState, bounds: Rect) -> DrawList {
        let mut list = DrawListBuilder::default();
        state.paint.shaped(self.skin.text_resources(), |text| {
            if let Some(widget) = state.widget.borrow_mut().as_mut() {
                widget.paint(&mut list, &mut TextMeasurer::new(text), bounds);
            }
        });
        list.finish()
    }
}

impl IcedWidget<UiEvent, Theme, Renderer> for Custom<'_> {
    fn size(&self) -> IcedSize<Length> {
        IcedSize::new(Length::Fill, Length::Fill)
    }

    fn tag(&self) -> widget::tree::Tag {
        widget::tree::Tag::of::<CustomState>()
    }

    fn state(&self) -> widget::tree::State {
        widget::tree::State::new(CustomState::new(self.kind, self.kinds))
    }

    fn layout(
        &mut self,
        tree: &mut Tree,
        _renderer: &Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        let resources = self.skin.text_resources();
        let state = self.state_for(tree);
        let asked = SizeLimits::new(
            Size2::new(limits.min().width, limits.min().height),
            Size2::new(limits.max().width, limits.max().height),
        );
        let intrinsic = state.paint.shaped(resources, |text| {
            state
                .widget
                .borrow_mut()
                .as_mut()
                .map_or_else(Size2::default, |widget| {
                    widget.measure(&mut TextMeasurer::new(text), asked)
                })
        });
        layout::Node::new(limits.resolve(
            Length::Fill,
            Length::Fill,
            IcedSize::new(intrinsic.w, intrinsic.h),
        ))
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut Renderer,
        _theme: &Theme,
        _style: &renderer::Style,
        layout: Layout<'_>,
        _cursor: Cursor,
        _viewport: &Rectangle,
    ) {
        let bounds = layout.bounds();
        if bounds.width < 1.0 || bounds.height < 1.0 {
            return;
        }
        let bounds = snapped(bounds);
        let state = tree.state.downcast_ref::<CustomState>();
        let local = Rect {
            h: bounds.height,
            w: bounds.width,
            x: 0.0,
            y: 0.0,
        };
        state.paint.mark(Redrawn, || self.list(state, local));
        state.paint.replay(
            renderer,
            bounds,
            |_| Rectangle::with_size(bounds.size()),
            self.skin.text_resources(),
        );
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: Cursor,
        _renderer: &Renderer,
        _clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, UiEvent>,
        _viewport: &Rectangle,
    ) {
        let state = self.state_for(tree);
        if let Event::Window(window::Event::RedrawRequested(now)) = event {
            let elapsed = state
                .drawn
                .replace(*now)
                .map(|last| now.duration_since(last));
            // Read before the tick, the way the retained host does: a widget
            // that stops asking still gets the frame it already asked for.
            let asked = state.repaint();
            if let Some(elapsed) = elapsed
                && let Some(published) = state
                    .widget
                    .borrow_mut()
                    .as_mut()
                    .and_then(|widget| widget.frame(elapsed))
            {
                shell.publish(published);
            }
            if asked != Repaint::None || state.repaint() == Repaint::Continuous {
                shell.request_redraw();
            }
            return;
        }
        let Some(input) = iced_interact::input(event) else {
            return;
        };
        let hit = iced_interact::hit(layout.bounds(), cursor);
        let Some(outcome) = state
            .widget
            .borrow_mut()
            .as_mut()
            .map(|widget| widget.input(input, hit))
        else {
            return;
        };
        let captured = outcome.is_captured();
        if let Some(published) = outcome.value() {
            shell.publish(published);
        }
        if captured {
            shell.capture_event();
        }
        if state.repaint() != Repaint::None {
            shell.request_redraw();
        }
    }
}

impl<'a> From<Custom<'a>> for Element<'a, UiEvent> {
    fn from(custom: Custom<'a>) -> Self {
        Self::new(custom)
    }
}
