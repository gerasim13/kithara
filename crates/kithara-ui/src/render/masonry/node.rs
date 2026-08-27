use std::rc::Rc;

use kithara_platform::time::Duration;
use masonry::{
    accesskit::{Node as AccessNode, Role},
    core::{
        AccessCtx, AllowRawMut, BoxConstraints, ChildrenIds, ComposeCtx, CursorIcon, EventCtx,
        LayoutCtx, PaintCtx, PointerButton as MasonryPointerButton, PointerEvent, PropertiesMut,
        PropertiesRef, QueryCtx, RegisterCtx, TextEvent, Update, UpdateCtx, Widget, WidgetId,
        WidgetPod,
    },
    kurbo::{Point, Size as MasonrySize},
    vello::Scene,
};
use num_traits::cast::AsPrimitive;
use tracing::{Span, trace_span};

use super::{
    Repaint,
    custom::HostAction,
    leaf::{Leaf, cursor_icon},
    mount::NodeLayout,
    picker::{HostedEngine, local_ime_area, sync_ime_area},
};
use crate::{
    atoms::design::corner::{corner_frame, corner_path},
    backends::VelloBackend,
    draw::{DrawListBuilder, Pt, Rect, Rgba, Transform, replay},
    interact::{
        CursorShape, Hit, Input, MOUSE, PointerInput, PointerOwnership, PointerPhase,
        masonry::{
            pointer_button, pointer_position, portable_modifiers, portable_scroll,
            portable_text_input,
        },
    },
    layout::{FrameCorners, FrameSides},
    render::{ReadValue, document::Ctx, shader::ShaderDeclaration, vis::VisFrame},
    solve,
};

#[derive(Clone, Copy, Eq, PartialEq)]
enum NodePointerOwner {
    Leaf,
    Engine,
}

pub(crate) struct Node {
    layout: NodeLayout,
    declared: solve::Size<solve::Length>,
    limits: Option<solve::Limits>,
    children: Vec<WidgetPod<Self>>,
    primary: Option<Box<dyn Fn() -> HostAction>>,
    secondary: Option<Box<dyn Fn() -> HostAction>>,
    background: Option<Rgba>,
    frame: Option<(FrameSides, Rgba, f32)>,
    round: FrameCorners,
    radius: f32,
    double_click: bool,
    pointer: Option<(Pt, Pt)>,
    pointer_owner: Option<NodePointerOwner>,
    engine: Option<Rc<HostedEngine>>,
    transform: Transform,
}

impl Node {
    pub(super) fn new(
        layout: NodeLayout,
        declared: solve::Size<solve::Length>,
        children: Vec<WidgetPod<Self>>,
        background: Option<Rgba>,
        frame: Option<(FrameSides, Rgba, f32)>,
    ) -> Self {
        Self {
            layout,
            declared,
            limits: None,
            children,
            primary: None,
            secondary: None,
            background,
            frame,
            round: FrameCorners::EMPTY,
            radius: 0.0,
            double_click: false,
            pointer: None,
            pointer_owner: None,
            engine: None,
            transform: Transform::IDENTITY,
        }
    }

    pub(crate) fn set_child_limits(
        ctx: &mut LayoutCtx<'_>,
        child: &mut WidgetPod<Self>,
        limits: solve::Limits,
    ) {
        let (node, mut raw) = ctx.get_raw_mut(child);
        if node.limits != Some(limits) {
            node.limits = Some(limits);
            raw.request_layout();
        }
    }

    fn queue_pointer(&self, ctx: &mut EventCtx<'_>, event: &PointerEvent) {
        let PointerEvent::Down(button) = event else {
            return;
        };
        let action = match button.button {
            Some(MasonryPointerButton::Secondary) => self.secondary.as_ref(),
            Some(MasonryPointerButton::Primary) | None => self.primary.as_ref(),
            _ => None,
        };
        if let Some(action) = action {
            ctx.submit_action::<HostAction>(action());
            ctx.set_handled();
        }
    }

    fn leaf_input(
        &mut self,
        ctx: &mut EventCtx<'_>,
        event: &PointerEvent,
        input: Input<'_>,
        hit: Hit,
    ) -> bool {
        let Some(leaf) = self.layout.leaf() else {
            return false;
        };
        let retained = self.pointer_owner == Some(NodePointerOwner::Leaf);
        let outcome = leaf.input(input, hit);
        if !matches!(leaf.repaint(), Repaint::None) {
            ctx.request_anim_frame();
            ctx.request_paint_only();
        }
        if matches!(event, PointerEvent::Down(_)) && hit.over() {
            ctx.request_focus();
        }
        let captured = outcome.is_captured();
        match outcome.ownership() {
            PointerOwnership::Claim if matches!(event, PointerEvent::Down(_)) => {
                ctx.capture_pointer();
                self.pointer_owner = Some(NodePointerOwner::Leaf);
            }
            PointerOwnership::Release if retained => {
                ctx.release_pointer();
                self.pointer_owner = None;
            }
            PointerOwnership::Unchanged | PointerOwnership::Claim | PointerOwnership::Release => {}
        }
        if let Some(action) = outcome.value() {
            ctx.submit_action::<HostAction>(action);
        }
        let exclusive = captured || retained;
        if retained
            && self.pointer_owner.is_some()
            && matches!(event, PointerEvent::Up(_) | PointerEvent::Cancel(_))
        {
            ctx.release_pointer();
            self.pointer_owner = None;
        }
        if exclusive {
            ctx.set_handled();
        }
        exclusive
    }

    /// Shows the value an engine just published for this node's control, without
    /// waiting for the rebuild that follows the gesture.
    pub(crate) fn show_live(&mut self, value: &ReadValue<'_>) -> bool {
        self.layout.leaf().is_some_and(|leaf| leaf.set_read(value))
    }

    pub(crate) fn refresh(&mut self, ctx: Ctx<'_, '_>) -> bool {
        self.layout.leaf().is_some_and(|leaf| leaf.refresh(ctx))
    }

    pub(crate) fn shader_declaration(&self) -> Option<ShaderDeclaration> {
        match &self.layout {
            NodeLayout::Leaf(leaf) => leaf.shader_declaration(),
            NodeLayout::Flex(_)
            | NodeLayout::Measured(_)
            | NodeLayout::Scroll(_)
            | NodeLayout::Stack
            | NodeLayout::Stage => None,
        }
    }

    pub(crate) fn vis_frame(&self) -> Option<VisFrame> {
        match &self.layout {
            NodeLayout::Leaf(Leaf::Vis(vis)) => vis.frame(),
            NodeLayout::Leaf(
                Leaf::Empty
                | Leaf::Control(_)
                | Leaf::Text { .. }
                | Leaf::Custom { .. }
                | Leaf::Shader(_),
            )
            | NodeLayout::Flex(_)
            | NodeLayout::Measured(_)
            | NodeLayout::Scroll(_)
            | NodeLayout::Stack
            | NodeLayout::Stage => None,
        }
    }

    fn engine_input(
        &mut self,
        ctx: &mut EventCtx<'_>,
        event: &PointerEvent,
        input: Input<'_>,
        point: Option<Pt>,
    ) -> bool {
        let Some(engine) = self.engine.as_ref().map(Rc::clone) else {
            return false;
        };
        let retained = self.pointer_owner == Some(NodePointerOwner::Engine);
        let routed = engine.route(input, point);
        let repaint = routed.repaint;
        let (outcome, focused) = (routed.outcome, routed.focused);
        if repaint {
            ctx.request_paint_only();
        }
        if matches!(event, PointerEvent::Down(_)) {
            if focused {
                ctx.request_focus();
            } else if ctx.has_focus_target() {
                ctx.resign_focus();
            }
        }
        sync_ime_area(ctx, &engine);
        let captured = outcome.is_captured();
        match outcome.ownership() {
            PointerOwnership::Claim if matches!(event, PointerEvent::Down(_)) => {
                ctx.capture_pointer();
                self.pointer_owner = Some(NodePointerOwner::Engine);
            }
            PointerOwnership::Release if retained => {
                ctx.release_pointer();
                self.pointer_owner = None;
            }
            PointerOwnership::Unchanged | PointerOwnership::Claim | PointerOwnership::Release => {}
        }
        if let Some(action) = outcome.value() {
            ctx.submit_action::<HostAction>(action);
        }
        let exclusive = captured || retained;
        if retained
            && self.pointer_owner.is_some()
            && matches!(event, PointerEvent::Up(_) | PointerEvent::Cancel(_))
        {
            ctx.release_pointer();
            self.pointer_owner = None;
        }
        if exclusive {
            ctx.set_handled();
        }
        exclusive
    }

    fn leaf_text_input(&mut self, ctx: &mut EventCtx<'_>, input: Input<'_>) -> bool {
        let Some(leaf) = self.layout.leaf() else {
            return false;
        };
        let size = ctx.size();
        let outcome = leaf.input(
            input,
            Hit::new(
                None,
                Rect {
                    x: 0.0,
                    y: 0.0,
                    w: size.width.as_(),
                    h: size.height.as_(),
                },
            ),
        );
        if !matches!(leaf.repaint(), Repaint::None) {
            ctx.request_anim_frame();
            ctx.request_paint_only();
        }
        let captured = outcome.is_captured();
        if let Some(action) = outcome.value() {
            ctx.submit_action::<HostAction>(action);
        }
        if captured {
            ctx.set_handled();
        }
        captured
    }

    fn engine_text_input(&self, ctx: &mut EventCtx<'_>, input: Input<'_>) -> bool {
        let Some(engine) = &self.engine else {
            return false;
        };
        let outcome = engine.route(input, None).outcome;
        sync_ime_area(ctx, engine);
        let captured = outcome.is_captured();
        if let Some(action) = outcome.value() {
            ctx.submit_action::<HostAction>(action);
        }
        if captured {
            ctx.request_paint_only();
            ctx.set_handled();
        }
        captured
    }

    fn route_text_input(&mut self, ctx: &mut EventCtx<'_>, input: Input<'_>) {
        if self.leaf_text_input(ctx, input) {
            return;
        }
        self.engine_text_input(ctx, input);
    }

    fn pointer_input(
        &mut self,
        ctx: &EventCtx<'_>,
        event: &PointerEvent,
    ) -> Option<(Input<'static>, Hit, Option<Pt>)> {
        if let Some(position) = pointer_position(event) {
            let scale = ctx.get_scale_factor();
            let local = ctx.local_position(position);
            self.pointer = Some((
                Pt {
                    x: (position.x / scale).as_(),
                    y: (position.y / scale).as_(),
                },
                Pt {
                    x: local.x.as_(),
                    y: local.y.as_(),
                },
            ));
        }
        let pointer = match event {
            PointerEvent::Down(button) => {
                self.double_click = button.state.count >= 2;
                Some((
                    PointerPhase::Down,
                    button.button.map(pointer_button),
                    button.state.count,
                ))
            }
            PointerEvent::Move(update) => Some((PointerPhase::Move, None, update.current.count)),
            PointerEvent::Up(button) => {
                let phase = if std::mem::take(&mut self.double_click) {
                    PointerPhase::DoubleClick
                } else {
                    PointerPhase::Up
                };
                Some((phase, button.button.map(pointer_button), button.state.count))
            }
            PointerEvent::Leave(_) => Some((PointerPhase::Leave, None, 0)),
            PointerEvent::Cancel(_) => Some((PointerPhase::Cancel, None, 0)),
            PointerEvent::Enter(_) | PointerEvent::Scroll(_) | PointerEvent::Gesture(_) => None,
        };
        let (window, local) = if matches!(event, PointerEvent::Leave(_) | PointerEvent::Cancel(_)) {
            (None, None)
        } else {
            self.pointer.unzip()
        };
        let size = ctx.size();
        let hit = Hit::new(
            local,
            Rect {
                x: 0.0,
                y: 0.0,
                w: size.width.as_(),
                h: size.height.as_(),
            },
        );
        let input = match (pointer, event) {
            (Some((phase, button, clicks)), _) => {
                Input::Pointer(PointerInput::new(MOUSE, button, phase, window, clicks))
            }
            (None, PointerEvent::Scroll(scroll)) => {
                Input::Wheel(portable_scroll(scroll.delta, ctx.get_scale_factor())?)
            }
            (None, _) => return None,
        };
        Some((input, hit, window))
    }

    fn paint_background(&self, bounds: Rect, list: &mut DrawListBuilder) {
        let Some(color) = self.background else {
            return;
        };
        if self.rounded() {
            list.fill_path(corner_path(bounds, self.radius, self.round), color);
        } else {
            list.fill_rect(bounds, color);
        }
    }

    /// Whether this node stands at a window corner the skin gives a radius to.
    /// Everywhere else the box is square, and the square path is the one that
    /// matches the other host pixel for pixel.
    const fn rounded(&self) -> bool {
        self.round.any() && self.radius > 0.0
    }

    /// A frame is drawn over whatever the node holds, not under it: a child
    /// that fills its parent to the edge would otherwise paint the side away,
    /// which is what the other host, stacking the frame above the body, never
    /// does.
    fn paint_frame(&self, bounds: Rect, list: &mut DrawListBuilder) {
        let Some((sides, color, width)) = self.frame else {
            return;
        };
        if self.rounded() {
            corner_frame(list, bounds, self.radius, self.round, sides, color, width);
            return;
        }
        // A frame belongs to the box it frames, so each side is filled just
        // inside it. A line stroked along the edge is centred on the edge, so
        // half of it falls outside the box and a width of one pixel lands
        // across two rows instead of on one — a whole pixel away from where
        // the other host puts the same side.
        let right = bounds.x + (bounds.w - width).max(0.0);
        let bottom = bounds.y + (bounds.h - width).max(0.0);
        let mut side = |x: f32, y: f32, w: f32, h: f32| {
            list.fill_rect(
                Rect {
                    x,
                    y,
                    w: w.max(0.0),
                    h: h.max(0.0),
                },
                color,
            );
        };
        if sides.top {
            side(bounds.x, bounds.y, bounds.w, width);
        }
        if sides.right {
            side(right, bounds.y, width, bounds.h);
        }
        if sides.bottom {
            side(bounds.x, bottom, bounds.w, width);
        }
        if sides.left {
            side(bounds.x, bounds.y, width, bounds.h);
        }
    }
}

/// What the mount sets on a node, and what a refresh sets on it afterwards.
///
/// These are the node's own state rather than anything it computes, and the
/// only callers are the tree builder and the render root; keeping them apart
/// from the widget's behaviour is what says so.
impl Node {
    /// Puts this node where the document now says it is, and answers whether
    /// that moved it. A pose only changes what the node paints, so a node that
    /// moved needs repainting and nothing else re-measured.
    pub(crate) fn place(&mut self, transform: Transform) -> bool {
        let moved = self.transform != transform;
        self.transform = transform;
        moved
    }

    /// What this node runs when it is pressed, and what it runs when it is
    /// pressed with the other button.
    pub(super) fn set_actions(
        &mut self,
        primary: Option<Box<dyn Fn() -> HostAction>>,
        secondary: Option<Box<dyn Fn() -> HostAction>>,
    ) {
        self.primary = primary;
        self.secondary = secondary;
    }

    /// Which of this node's corners are the window's own, and how far they are
    /// rounded. Both come from the mount: the corners from where the layout
    /// puts the node, the radius from the skin.
    pub(super) const fn set_round(&mut self, round: FrameCorners, radius: f32) {
        self.round = round;
        self.radius = radius;
    }

    pub(super) fn set_engine(&mut self, engine: Rc<HostedEngine>) {
        self.engine = Some(engine);
    }

    /// Whether this node draws through a pass of its own rather than into the
    /// scene, which is what makes it something the host has to declare.
    pub(super) const fn is_native(&self) -> bool {
        matches!(
            &self.layout,
            NodeLayout::Leaf(Leaf::Shader(_) | Leaf::Vis(_))
        )
    }

    /// Where this node draws, relative to the box the layout gave it.
    #[cfg(test)]
    pub(super) const fn transform(&self) -> Transform {
        self.transform
    }
}

impl AllowRawMut for Node {}

impl Widget for Node {
    type Action = HostAction;

    fn on_pointer_event(
        &mut self,
        ctx: &mut EventCtx<'_>,
        _props: &mut PropertiesMut<'_>,
        event: &PointerEvent,
    ) {
        let Some((input, hit, point)) = self.pointer_input(ctx, event) else {
            self.queue_pointer(ctx, event);
            return;
        };
        // A window under the pointer takes the wheel before anything below it,
        // and only while it still has somewhere to go.
        if self.layout.wheel(input) {
            ctx.set_handled();
            ctx.request_layout();
            return;
        }
        match self.pointer_owner {
            Some(NodePointerOwner::Leaf) => {
                self.leaf_input(ctx, event, input, hit);
                return;
            }
            Some(NodePointerOwner::Engine) => {
                self.engine_input(ctx, event, input, point);
                return;
            }
            None => {}
        }
        if self.leaf_input(ctx, event, input, hit) {
            return;
        }
        if self.pointer_owner.is_some() {
            return;
        }
        if self.engine_input(ctx, event, input, point) {
            return;
        }
        if self.pointer_owner.is_some() {
            return;
        }
        self.queue_pointer(ctx, event);
    }

    fn on_text_event(
        &mut self,
        ctx: &mut EventCtx<'_>,
        _props: &mut PropertiesMut<'_>,
        event: &TextEvent,
    ) {
        if let TextEvent::Keyboard(event) = event {
            self.route_text_input(
                ctx,
                Input::ModifiersChanged(portable_modifiers(event.modifiers)),
            );
        }
        if let Some(input) = portable_text_input(event) {
            self.route_text_input(ctx, input);
        }
    }

    fn register_children(&mut self, ctx: &mut RegisterCtx<'_>) {
        for child in &mut self.children {
            ctx.register_child(child);
        }
    }

    fn update(&mut self, ctx: &mut UpdateCtx<'_>, _props: &mut PropertiesMut<'_>, event: &Update) {
        if matches!(event, Update::WidgetAdded | Update::StashedChanged(false))
            && let Some(leaf) = self.layout.leaf()
        {
            leaf.added(ctx);
        }
        if let Update::HoveredChanged(hovered) = event
            && let Some(leaf) = self.layout.leaf()
            && leaf.hover(*hovered)
        {
            ctx.request_paint_only();
        }
        if matches!(event, Update::FocusChanged(false))
            && let Some(engine) = &self.engine
        {
            engine.clear_focus();
        }
    }

    fn on_anim_frame(
        &mut self,
        ctx: &mut UpdateCtx<'_>,
        _props: &mut PropertiesMut<'_>,
        interval: u64,
    ) {
        if ctx.is_stashed() {
            return;
        }
        if let Some(leaf) = self.layout.leaf() {
            let repaint = leaf.repaint();
            if let Some(action) = leaf.frame(Duration::from_nanos(interval)) {
                ctx.submit_action::<HostAction>(action);
            }
            leaf.animate(ctx, repaint);
        }
    }

    fn layout(
        &mut self,
        ctx: &mut LayoutCtx<'_>,
        _props: &mut PropertiesMut<'_>,
        constraints: &BoxConstraints,
    ) -> MasonrySize {
        let limits = self.limits.unwrap_or_else(|| {
            solve::Limits::new(
                solve::Size::new(
                    constraints.min().width.as_(),
                    constraints.min().height.as_(),
                ),
                solve::Size::new(
                    constraints.max().width.as_(),
                    constraints.max().height.as_(),
                ),
            )
        });
        let size = self
            .layout
            .layout(ctx, &mut self.children, limits, self.declared);
        MasonrySize::new(f64::from(size.width), f64::from(size.height))
    }

    fn compose(&mut self, ctx: &mut ComposeCtx<'_>) {
        if let Some(engine) = &self.engine {
            if let Some(area) = local_ime_area(engine, ctx.window_transform()) {
                ctx.set_ime_area(area);
            } else {
                ctx.clear_ime_area();
            }
        }
    }

    fn paint(&mut self, ctx: &mut PaintCtx<'_>, _props: &PropertiesRef<'_>, scene: &mut Scene) {
        let size = ctx.size();
        let bounds = Rect {
            h: size.height.as_(),
            w: size.width.as_(),
            x: 0.0,
            y: 0.0,
        };
        let mut list = DrawListBuilder::default();
        self.paint_background(bounds, &mut list);
        replay(&list.finish(), &mut VelloBackend::new(scene));
        if let Some(leaf) = self.layout.leaf() {
            leaf.paint(bounds, self.transform, scene);
        }
    }

    /// What this node draws over its children rather than under them: a
    /// window's indicator belongs above the rows it scrolls, and a frame
    /// belongs on the edge of the box whatever fills it. Masonry appends this
    /// scene after the children and outside the clip, which is exactly where a
    /// mark on the node's own edge has to land.
    fn post_paint(
        &mut self,
        ctx: &mut PaintCtx<'_>,
        _props: &PropertiesRef<'_>,
        scene: &mut Scene,
    ) {
        let size = ctx.size();
        let mut list = DrawListBuilder::default();
        let bounds = Rect {
            h: size.height.as_(),
            w: size.width.as_(),
            x: 0.0,
            y: 0.0,
        };
        self.layout.indicate(bounds, &mut list);
        self.paint_frame(bounds, &mut list);
        replay(&list.finish(), &mut VelloBackend::new(scene));
    }

    fn accessibility_role(&self) -> Role {
        Role::GenericContainer
    }

    fn accessibility(
        &mut self,
        _ctx: &mut AccessCtx<'_>,
        _props: &PropertiesRef<'_>,
        _node: &mut AccessNode,
    ) {
    }

    fn children_ids(&self) -> ChildrenIds {
        self.children.iter().map(WidgetPod::id).collect()
    }

    fn accepts_pointer_interaction(&self) -> bool {
        self.primary.is_some()
            || self.secondary.is_some()
            || self.layout.accepts_input()
            || self.engine.is_some()
    }

    fn accepts_focus(&self) -> bool {
        self.layout.accepts_input() || self.engine.is_some()
    }

    fn accepts_text_input(&self) -> bool {
        self.layout.accepts_text_input()
            || self
                .engine
                .as_ref()
                .is_some_and(|engine| engine.accepts_text_input())
    }

    fn get_cursor(&self, ctx: &QueryCtx<'_>, pos: Point) -> CursorIcon {
        let local = ctx.window_transform().inverse() * pos;
        let size = ctx.size();
        let hit = Hit::new(
            Some(Pt {
                x: local.x.as_(),
                y: local.y.as_(),
            }),
            Rect {
                x: 0.0,
                y: 0.0,
                w: size.width.as_(),
                h: size.height.as_(),
            },
        );
        let leaf = match &self.layout {
            NodeLayout::Leaf(leaf) => leaf.cursor(&hit),
            NodeLayout::Flex(_)
            | NodeLayout::Measured(_)
            | NodeLayout::Scroll(_)
            | NodeLayout::Stack
            | NodeLayout::Stage => CursorShape::None,
        };
        let engine = self.engine.as_ref().map_or(CursorShape::None, |engine| {
            engine.cursor(Pt {
                x: pos.x.as_(),
                y: pos.y.as_(),
            })
        });
        let shape = match self.pointer_owner {
            Some(NodePointerOwner::Leaf) => leaf,
            None if leaf != CursorShape::None => leaf,
            Some(NodePointerOwner::Engine) | None => engine,
        };
        cursor_icon(shape)
    }

    fn make_trace_span(&self, id: WidgetId) -> Span {
        trace_span!("KitharaMasonryNode", id = id.trace())
    }
}

#[cfg(test)]
impl Node {
    pub(crate) fn set_child_stashed(
        this: &mut masonry::core::WidgetMut<'_, Self>,
        child: usize,
        stashed: bool,
    ) {
        this.ctx
            .set_stashed(&mut this.widget.children[child], stashed);
    }
}

#[cfg(test)]
mod tests {
    use kithara_test_utils::kithara;

    use super::*;
    use crate::draw::{DrawCmd, Geom};

    /// The node the checks below paint.
    struct Fixture;

    impl Fixture {
        /// A box wide enough to tell one side of a frame from another.
        const BOX: Rect = Rect {
            h: 40.0,
            w: 100.0,
            x: 0.0,
            y: 0.0,
        };

        /// The colour its frame is drawn in.
        const INK: Rgba = Rgba {
            a: 1.0,
            b: 0.5,
            g: 0.4,
            r: 0.3,
        };

        /// The ground it lays down under its children.
        const PAPER: Rgba = Rgba {
            a: 1.0,
            b: 0.1,
            g: 0.1,
            r: 0.1,
        };
    }

    fn framed() -> Node {
        Node::new(
            NodeLayout::Leaf(Leaf::Empty),
            solve::Size::new(solve::Length::Fill, solve::Length::Fill),
            Vec::new(),
            Some(Fixture::PAPER),
            Some((FrameSides::default(), Fixture::INK, 1.0)),
        )
    }

    /// The same node, standing at every corner of a window the skin rounds.
    fn at_a_window_corner() -> Node {
        let mut node = framed();
        node.set_round(FrameCorners::ALL, 6.0);
        node
    }

    fn painted(node: &Node, paint: impl FnOnce(&Node, &mut DrawListBuilder)) -> Vec<DrawCmd> {
        let mut list = DrawListBuilder::default();
        paint(node, &mut list);
        list.finish().commands().to_vec()
    }

    fn drawn(paint: impl FnOnce(&Node, &mut DrawListBuilder)) -> Vec<DrawCmd> {
        let node = framed();
        let mut list = DrawListBuilder::default();
        paint(&node, &mut list);
        list.finish().commands().to_vec()
    }

    fn rects(commands: &[DrawCmd]) -> Vec<Rect> {
        commands
            .iter()
            .filter_map(|command| match command {
                DrawCmd::Fill {
                    geom: Geom::Rect(rect),
                    ..
                } => Some(*rect),
                _ => None,
            })
            .collect()
    }

    #[kithara::test]
    fn the_pass_under_a_node_s_children_lays_down_its_ground_alone() {
        let commands = drawn(|node, list| node.paint_background(Fixture::BOX, list));

        assert_eq!(
            rects(&commands),
            vec![Fixture::BOX],
            "the pass children paint over must carry the ground and nothing else: {commands:?}"
        );
    }

    #[kithara::test]
    fn a_node_puts_every_side_of_its_frame_in_the_pass_over_its_children() {
        let commands = drawn(|node, list| node.paint_frame(Fixture::BOX, list));

        assert_eq!(
            rects(&commands),
            vec![
                Rect {
                    h: 1.0,
                    w: Fixture::BOX.w,
                    x: Fixture::BOX.x,
                    y: Fixture::BOX.y,
                },
                Rect {
                    h: Fixture::BOX.h,
                    w: 1.0,
                    x: Fixture::BOX.x + Fixture::BOX.w - 1.0,
                    y: Fixture::BOX.y,
                },
                Rect {
                    h: 1.0,
                    w: Fixture::BOX.w,
                    x: Fixture::BOX.x,
                    y: Fixture::BOX.y + Fixture::BOX.h - 1.0,
                },
                Rect {
                    h: Fixture::BOX.h,
                    w: 1.0,
                    x: Fixture::BOX.x,
                    y: Fixture::BOX.y,
                },
            ],
            "each side belongs just inside the box it frames: {commands:?}"
        );
    }

    /// At a window corner the ground is one path and not a rectangle: a
    /// rectangle has no corner to take off.
    #[kithara::test]
    fn a_node_at_a_window_corner_lays_its_ground_down_as_a_path() {
        let commands = painted(&at_a_window_corner(), |node, list| {
            node.paint_background(Fixture::BOX, list);
        });

        assert!(
            matches!(
                commands.as_slice(),
                [DrawCmd::Fill {
                    geom: Geom::Path(_),
                    ..
                }]
            ),
            "a rounded ground is one path: {commands:?}"
        );
    }

    /// The frame at a window corner is one band that follows the corner round,
    /// not the four straight sides a square box is framed with.
    #[kithara::test]
    fn a_node_at_a_window_corner_draws_its_frame_as_one_band() {
        let commands = painted(&at_a_window_corner(), |node, list| {
            node.paint_frame(Fixture::BOX, list);
        });

        assert!(
            matches!(commands.as_slice(), [DrawCmd::Clip { .. }]),
            "a rounded frame is one clipped band: {commands:?}"
        );
    }

    /// A node the skin gives a radius but the layout puts nowhere near a window
    /// corner is framed with the same four sides as any other box.
    #[kithara::test]
    fn a_radius_alone_does_not_round_a_node_away_from_the_window_corner() {
        let mut node = framed();
        node.set_round(FrameCorners::EMPTY, 6.0);

        let commands = painted(&node, |node, list| node.paint_frame(Fixture::BOX, list));

        assert_eq!(rects(&commands).len(), 4, "{commands:?}");
    }

    #[kithara::test]
    fn a_node_with_no_frame_draws_nothing_over_its_children() {
        let node = Node::new(
            NodeLayout::Leaf(Leaf::Empty),
            solve::Size::new(solve::Length::Fill, solve::Length::Fill),
            Vec::new(),
            Some(Fixture::PAPER),
            None,
        );
        let mut list = DrawListBuilder::default();
        node.paint_frame(Fixture::BOX, &mut list);

        assert!(
            list.finish().commands().is_empty(),
            "a node the skin gives no frame must leave the pass over its children empty"
        );
    }
}
