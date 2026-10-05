use std::{cell::Cell, rc::Rc};

use masonry::{
    accesskit::{Node as AccessNode, Role},
    core::{
        AccessCtx, BoxConstraints, ChildrenIds, CursorIcon, EventCtx, LayoutCtx, PaintCtx,
        PointerEvent, PropertiesMut, PropertiesRef, QueryCtx, RegisterCtx, Widget, WidgetId,
        WidgetRef, find_widget_under_pointer,
    },
    kurbo::{Point, Size},
    vello::Scene,
};
use num_traits::cast::AsPrimitive;
use tracing::{Span, trace_span};

use crate::{
    backends::VelloBackend,
    draw::{Pt, Rect, replay},
    interact::{CursorShape, masonry::cursor_icon},
    render::{
        DragGhost, HostLayer, Published, Skin, WindowCommand, WindowEdge, WindowSurface,
        masonry::custom::HostAction,
    },
    shaping::TextContext,
};

pub(crate) struct WindowLayer {
    active: Option<WindowCommand>,
    ghost: Option<DragGhost>,
    map_event: Rc<dyn Fn(Published) -> HostAction>,
    pointer: Rc<Cell<Option<Pt>>>,
    text: TextContext,
    resize_edges: bool,
    resize_edge: f32,
}

impl WindowLayer {
    pub(crate) fn new(
        ghost: Option<DragGhost>,
        resize_edges: bool,
        pointer: Rc<Cell<Option<Pt>>>,
        map_event: Rc<dyn Fn(Published) -> HostAction>,
        skin: &Skin,
    ) -> Self {
        Self {
            ghost,
            map_event,
            pointer,
            resize_edges,
            active: None,
            resize_edge: skin.window.resize_edge,
            text: TextContext::from(skin.text_resources()),
        }
    }

    fn bounds(size: Size) -> Rect {
        Rect {
            x: 0.0,
            y: 0.0,
            w: size.width.as_(),
            h: size.height.as_(),
        }
    }

    /// Takes up what the pointer is carrying now, and says whether that changed
    /// what this layer draws.
    pub(in crate::render) fn carry(&mut self, label: Option<&str>) -> bool {
        self.ghost.as_mut().is_some_and(|ghost| ghost.carry(label))
    }

    fn command_at(&self, size: Size, pointer: Option<Pt>) -> Option<WindowCommand> {
        self.resize_layer(size)
            .and_then(|layer| layer.action_at(pointer).copied())
    }

    fn resize_layer(&self, size: Size) -> Option<HostLayer<WindowCommand>> {
        self.resize_edges
            .then(|| WindowSurface::frame(Self::bounds(size), self.resize_edge))
    }
}

impl Widget for WindowLayer {
    type Action = HostAction;

    fn accessibility(
        &mut self,
        _ctx: &mut AccessCtx<'_>,
        _props: &PropertiesRef<'_>,
        _node: &mut AccessNode,
    ) {
    }

    fn accessibility_role(&self) -> Role {
        Role::GenericContainer
    }

    fn children_ids(&self) -> ChildrenIds {
        ChildrenIds::new()
    }

    fn find_widget_under_pointer<'ctx>(
        &'ctx self,
        ctx: QueryCtx<'ctx>,
        pos: Point,
    ) -> Option<WidgetRef<'ctx, dyn Widget>> {
        let local = ctx.window_transform().inverse() * pos;
        let pointer = Some(Pt {
            x: local.x.as_(),
            y: local.y.as_(),
        });
        self.command_at(ctx.size(), pointer)
            .and_then(|_| find_widget_under_pointer(self, ctx, pos))
    }

    fn get_cursor(&self, ctx: &QueryCtx<'_>, pos: Point) -> CursorIcon {
        let local = ctx.window_transform().inverse() * pos;
        let pointer = Some(Pt {
            x: local.x.as_(),
            y: local.y.as_(),
        });
        let cursor = self.active.map_or_else(
            || {
                self.resize_layer(ctx.size())
                    .map_or(CursorShape::None, |layer| layer.cursor_at(pointer))
            },
            command_cursor,
        );
        cursor_icon(cursor)
    }

    fn layout(
        &mut self,
        _ctx: &mut LayoutCtx<'_>,
        _props: &mut PropertiesMut<'_>,
        constraints: &BoxConstraints,
    ) -> Size {
        constraints.max()
    }

    fn make_trace_span(&self, id: WidgetId) -> Span {
        trace_span!("KitharaWindowLayer", id = id.trace())
    }

    fn on_pointer_event(
        &mut self,
        ctx: &mut EventCtx<'_>,
        _props: &mut PropertiesMut<'_>,
        event: &PointerEvent,
    ) {
        if let PointerEvent::Down(button) = event {
            let position = button.state.logical_position();
            let pointer = Some(Pt {
                x: position.x.as_(),
                y: position.y.as_(),
            });
            if let Some(command) = self.command_at(ctx.size(), pointer) {
                self.active = Some(command);
                ctx.submit_action::<HostAction>((self.map_event)(Published::window(command)));
                ctx.capture_pointer();
                ctx.set_handled();
                return;
            }
        }
        if ctx.is_pointer_capture_target() {
            if matches!(event, PointerEvent::Move(_)) {
                ctx.request_paint_only();
            }
            if matches!(event, PointerEvent::Up(_) | PointerEvent::Cancel(_)) {
                self.active = None;
                ctx.release_pointer();
                ctx.request_cursor_icon_change();
            }
            ctx.set_handled();
        }
    }

    fn paint(&mut self, ctx: &mut PaintCtx<'_>, _props: &PropertiesRef<'_>, scene: &mut Scene) {
        let Some(ghost) = &self.ghost else {
            return;
        };
        let layer = ghost.layer(self.pointer.get(), Self::bounds(ctx.size()), &mut self.text);
        replay(layer.draw(), &mut VelloBackend::new(scene));
    }

    fn register_children(&mut self, _ctx: &mut RegisterCtx<'_>) {}
}

const fn command_cursor(command: WindowCommand) -> CursorShape {
    match command {
        WindowCommand::Resize(WindowEdge::North | WindowEdge::South) => CursorShape::ResizeV,
        WindowCommand::Resize(WindowEdge::East | WindowEdge::West) => CursorShape::ResizeH,
        WindowCommand::Resize(WindowEdge::NorthWest | WindowEdge::SouthEast) => {
            CursorShape::ResizeDiagonalDown
        }
        WindowCommand::Resize(WindowEdge::NorthEast | WindowEdge::SouthWest) => {
            CursorShape::ResizeDiagonalUp
        }
        WindowCommand::Drag
        | WindowCommand::Minimize
        | WindowCommand::ToggleMaximize
        | WindowCommand::ToggleFullScreen
        | WindowCommand::Close => CursorShape::None,
    }
}
