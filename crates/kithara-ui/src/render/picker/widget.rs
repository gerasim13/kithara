use std::{any::Any, cell::RefCell, rc::Rc};

use iced::{
    Element, Event, Length, Rectangle, Renderer, Size, Theme, Vector,
    advanced::{
        Clipboard, Shell, Widget as IcedWidget,
        layout::{self, Layout},
        mouse, overlay, renderer,
        widget::{self, Operation, Tree},
    },
    widget::canvas::Canvas,
};

use super::{
    overlay::PickerPortal,
    paint::{PickerPaint, picker_selected_index},
    program::{InputProgram, PaintProgram},
};
use crate::{
    engine::{Descriptor, Engine, PickerSnapshot},
    render::{InputOwner, ReadValue, Skin, UiEvent},
    text::TextContext,
};

pub(crate) fn scope_picker<'a>(
    path: &str,
    items: Vec<&'a str>,
    value: Option<&ReadValue<'_>>,
    skin: &'a Skin,
    owner: InputOwner,
) -> Element<'a, UiEvent> {
    let selected = picker_selected_index(value, items.len());
    let paint = Rc::new(PickerPaint::new(items, selected, skin));
    let width = paint.width();
    let height = paint.item_height();
    let anchor: Element<'a, UiEvent> = match owner {
        InputOwner::Leaf => Canvas::new(InputProgram::new(path, Rc::clone(&paint)))
            .width(Length::Fixed(width))
            .height(Length::Fixed(height))
            .into(),
        InputOwner::Engine => Canvas::new(PaintProgram::new(Rc::clone(&paint)))
            .width(Length::Fixed(width))
            .height(Length::Fixed(height))
            .into(),
    };
    Element::new(PickerWidget {
        anchor,
        owner,
        paint,
        path: path.to_owned(),
    })
}

pub(crate) fn sync_picker(path: &str, snapshot: PickerSnapshot) -> impl Operation + '_ {
    struct Sync<'a> {
        path: &'a str,
        snapshot: PickerSnapshot,
    }

    impl Operation for Sync<'_> {
        fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation)) {
            operate(self);
        }

        fn custom(&mut self, _id: Option<&widget::Id>, _bounds: Rectangle, state: &mut dyn Any) {
            if let Some(state) = state.downcast_mut::<PickerState>() {
                state.sync(self.path, self.snapshot);
            }
        }
    }

    Sync { path, snapshot }
}

struct PickerWidget<'a> {
    anchor: Element<'a, UiEvent>,
    owner: InputOwner,
    paint: Rc<PickerPaint<'a>>,
    path: String,
}

impl IcedWidget<UiEvent, Theme, Renderer> for PickerWidget<'_> {
    fn tag(&self) -> widget::tree::Tag {
        widget::tree::Tag::of::<PickerState>()
    }

    fn state(&self) -> widget::tree::State {
        widget::tree::State::new(PickerState::new(
            &self.path,
            self.paint.item_count(),
            self.paint.selected(),
            self.owner,
        ))
    }

    fn diff(&self, tree: &mut Tree) {
        tree.state.downcast_mut::<PickerState>().reconcile(
            &self.path,
            self.paint.item_count(),
            self.paint.selected(),
            self.owner,
        );
    }

    delegate::delegate! {
        to self.anchor.as_widget() {
            fn size(&self) -> Size<Length>;
            fn size_hint(&self) -> Size<Length>;
            fn mouse_interaction(
                &self,
                tree: &Tree,
                layout: Layout<'_>,
                cursor: mouse::Cursor,
                viewport: &Rectangle,
                renderer: &Renderer,
            ) -> mouse::Interaction;
            fn draw(
                &self,
                tree: &Tree,
                renderer: &mut Renderer,
                theme: &Theme,
                style: &renderer::Style,
                layout: Layout<'_>,
                cursor: mouse::Cursor,
                viewport: &Rectangle,
            );
        }
    }

    fn layout(
        &mut self,
        tree: &mut Tree,
        renderer: &Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        self.anchor.as_widget_mut().layout(tree, renderer, limits)
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
        let before = tree.state.downcast_ref::<PickerState>().snapshot();
        self.anchor.as_widget_mut().update(
            tree, event, layout, cursor, renderer, clipboard, shell, viewport,
        );
        let after = tree.state.downcast_ref::<PickerState>().snapshot();
        if before != after {
            shell.request_redraw();
            shell.invalidate_layout();
        }
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout<'_>,
        _renderer: &Renderer,
        operation: &mut dyn Operation,
    ) {
        operation.custom(
            None,
            layout.bounds(),
            tree.state.downcast_mut::<PickerState>(),
        );
    }

    fn overlay<'a>(
        &'a mut self,
        tree: &'a mut Tree,
        layout: Layout<'a>,
        _renderer: &Renderer,
        _viewport: &Rectangle,
        translation: Vector,
    ) -> Option<overlay::Element<'a, UiEvent, Theme, Renderer>> {
        let state = tree.state.downcast_mut::<PickerState>();
        state.snapshot().open.then(|| {
            overlay::Element::new(Box::new(PickerPortal {
                anchor: layout.bounds() + translation,
                owner: self.owner,
                paint: &self.paint,
                path: &self.path,
                state,
            }))
        })
    }
}

#[derive(fieldwork::Fieldwork)]
#[fieldwork(opt_in, get)]
pub(super) struct PickerState {
    engine: Option<Engine>,
    path: String,
    #[field(get, vis = "pub(super)", copy)]
    snapshot: PickerSnapshot,
    #[field(get, vis = "pub(super)")]
    text: RefCell<Option<TextContext>>,
}

impl Default for PickerState {
    fn default() -> Self {
        Self {
            engine: None,
            path: String::new(),
            snapshot: PickerSnapshot {
                open: false,
                highlighted: None,
            },
            text: RefCell::default(),
        }
    }
}

impl PickerState {
    pub(super) fn new(
        path: &str,
        item_count: usize,
        selected: Option<usize>,
        owner: InputOwner,
    ) -> Self {
        let mut state = Self {
            engine: None,
            path: path.to_owned(),
            snapshot: PickerSnapshot {
                open: false,
                highlighted: selected.filter(|index| *index < item_count),
            },
            text: RefCell::default(),
        };
        state.reconcile(path, item_count, selected, owner);
        state
    }

    fn reconcile(
        &mut self,
        path: &str,
        item_count: usize,
        selected: Option<usize>,
        owner: InputOwner,
    ) {
        self.path = path.to_owned();
        match owner {
            InputOwner::Leaf => {
                self.engine
                    .get_or_insert_with(Engine::default)
                    .reconcile([Descriptor::picker(path.to_owned(), item_count, selected)]);
                self.refresh(path);
            }
            InputOwner::Engine => self.engine = None,
        }
    }

    pub(super) fn engine_mut(&mut self) -> Option<&mut Engine> {
        self.engine.as_mut()
    }

    pub(super) fn refresh(&mut self, path: &str) {
        if let Some(snapshot) = self
            .engine
            .as_ref()
            .and_then(|engine| engine.picker_snapshot(path))
        {
            self.snapshot = snapshot;
        }
    }

    fn sync(&mut self, path: &str, snapshot: PickerSnapshot) {
        if self.path == path {
            self.snapshot = snapshot;
        }
    }
}

#[cfg(test)]
mod tests {
    use iced::{
        Element, Event, Length, Pixels, Point, Rectangle, Renderer, Size, Vector,
        advanced::{
            Shell, clipboard,
            layout::{Layout, Limits},
            mouse, overlay,
            widget::Tree,
        },
        widget::Column,
    };
    use iced_renderer::fallback::Renderer as FallbackRenderer;
    use iced_tiny_skia::Renderer as TinySkiaRenderer;
    use kithara_test_utils::kithara;

    use super::*;
    use crate::{
        builtin,
        render::{ControlAction, WindowCommand, fonts::SANS},
        widgets::{Widget, window::WindowSurface},
    };

    fn dispatch(
        element: &mut Element<'_, UiEvent>,
        tree: &mut Tree,
        node: &layout::Node,
        renderer: &Renderer,
        viewport: Size,
        pointer: Point,
    ) -> (Vec<UiEvent>, bool) {
        let event = Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left));
        let cursor = mouse::Cursor::Available(pointer);
        let bounds = Rectangle::with_size(viewport);
        let layout = Layout::new(node);
        let mut clipboard = clipboard::Null;
        let mut messages = Vec::new();
        let mut shell = Shell::new(&mut messages);
        let mut base_cursor = cursor;
        {
            let overlay =
                element
                    .as_widget_mut()
                    .overlay(tree, layout, renderer, &bounds, Vector::ZERO);
            if let Some(overlay) = overlay {
                let mut nested = overlay::Nested::new(overlay);
                let overlay_node = nested.layout(renderer, viewport);
                nested.update(
                    &event,
                    Layout::new(&overlay_node),
                    cursor,
                    renderer,
                    &mut clipboard,
                    &mut shell,
                );
                if !shell.is_event_captured()
                    && nested.mouse_interaction(Layout::new(&overlay_node), cursor, renderer)
                        != mouse::Interaction::None
                {
                    base_cursor = mouse::Cursor::Unavailable;
                }
            }
        }
        if !shell.is_event_captured() {
            element.as_widget_mut().update(
                tree,
                &event,
                layout,
                base_cursor,
                renderer,
                &mut clipboard,
                &mut shell,
                &bounds,
            );
        }
        let captured = shell.is_event_captured();
        drop(shell);
        (messages, captured)
    }

    #[kithara::test]
    fn an_open_popup_captures_before_an_overlapping_window_layer() {
        let skin = builtin::skin();
        let item_height = skin.tree.scope_item_height;
        let picker = scope_picker(
            "library/context",
            vec!["ZVUK", "LOCAL"],
            None,
            skin,
            InputOwner::Leaf,
        );
        let chrome = WindowSurface::drag().view();
        let mut element: Element<'_, UiEvent> = Column::with_children(vec![picker, chrome])
            .width(Length::Fill)
            .height(Length::Fill)
            .into();
        let renderer = FallbackRenderer::Secondary(TinySkiaRenderer::new(SANS, Pixels(14.0)));
        let viewport = Size::new(160.0, item_height * 3.0);
        let mut tree = Tree::new(element.as_widget());
        let node = element.as_widget_mut().layout(
            &mut tree,
            &renderer,
            &Limits::new(Size::ZERO, viewport),
        );

        let (opened, captured) = dispatch(
            &mut element,
            &mut tree,
            &node,
            &renderer,
            viewport,
            Point::new(4.0, item_height / 2.0),
        );
        assert!(opened.is_empty());
        assert!(captured);

        let (selected, captured) = dispatch(
            &mut element,
            &mut tree,
            &node,
            &renderer,
            viewport,
            Point::new(4.0, item_height + item_height / 2.0),
        );
        assert_eq!(
            selected,
            [UiEvent::Control {
                path: "library/context".to_owned(),
                action: ControlAction::SelectIndex(0),
            }]
        );
        assert!(captured);
        assert!(!selected.contains(&UiEvent::Window(WindowCommand::Drag)));
    }
}
