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
    overlay::PickerOverlay,
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

    fn size(&self) -> Size<Length> {
        self.anchor.as_widget().size()
    }

    fn size_hint(&self) -> Size<Length> {
        self.anchor.as_widget().size_hint()
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

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        self.anchor
            .as_widget()
            .mouse_interaction(tree, layout, cursor, viewport, renderer)
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
        self.anchor
            .as_widget()
            .draw(tree, renderer, theme, style, layout, cursor, viewport);
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
            let popup = overlay::Element::new(Box::new(PickerOverlay {
                anchor: layout.bounds() + translation,
                owner: self.owner,
                paint: &self.paint,
                path: &self.path,
                state,
            }));
            overlay::Group::with_children(vec![popup]).overlay()
        })
    }
}

pub(super) struct PickerState {
    engine: Option<Engine>,
    path: String,
    snapshot: PickerSnapshot,
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

    pub(super) const fn snapshot(&self) -> PickerSnapshot {
        self.snapshot
    }

    fn sync(&mut self, path: &str, snapshot: PickerSnapshot) {
        if self.path == path {
            self.snapshot = snapshot;
        }
    }

    pub(super) const fn text(&self) -> &RefCell<Option<TextContext>> {
        &self.text
    }
}
