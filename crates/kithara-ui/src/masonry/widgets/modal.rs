use std::rc::Rc;

use masonry::{
    accesskit::{Node as AccessNode, Role},
    core::{
        AccessCtx, BoxConstraints, ChildrenIds, EventCtx, LayoutCtx, NewWidget, PaintCtx,
        PointerEvent, PropertiesMut, PropertiesRef, QueryCtx, RegisterCtx, Widget, WidgetId,
        WidgetPod, WidgetRef, find_widget_under_pointer,
    },
    kurbo::{Affine, Point, Rect as MasonryRect, Size as MasonrySize, Vec2},
    vello::Scene,
};
use num_traits::cast::AsPrimitive;
use tracing::{Span, trace_span};

use crate::{
    backends::{VelloBackend, paint_color},
    draw::{DrawListBuilder, Rect, replay},
    hosts::{
        layer::ModalChrome,
        solve::{self, Size},
    },
    masonry::retained::{
        custom::HostAction,
        node::Node,
        popover::{PopoverState, fit_content},
    },
    render::Skin,
};

/// The layer a modal stands in: a scrim over the whole window and the framed
/// content centred on it, taking every press the content does not.
pub(crate) struct ModalLayer {
    chrome: ModalChrome,
    state: Rc<PopoverState>,
    declared: Size<solve::Length>,
    child: WidgetPod<Node>,
}

impl ModalLayer {
    pub(crate) fn new(
        content: NewWidget<Node>,
        declared: Size<solve::Length>,
        state: Rc<PopoverState>,
        skin: &Skin,
    ) -> Self {
        Self {
            declared,
            state,
            chrome: ModalChrome::new(skin),
            child: content.to_pod(),
        }
    }
}

fn masonry_rect(rect: Rect) -> MasonryRect {
    MasonryRect::new(
        f64::from(rect.x),
        f64::from(rect.y),
        f64::from(rect.x + rect.w),
        f64::from(rect.y + rect.h),
    )
}

fn draw_rect(rect: MasonryRect) -> Rect {
    Rect {
        x: rect.x0.as_(),
        y: rect.y0.as_(),
        w: rect.width().as_(),
        h: rect.height().as_(),
    }
}

impl Widget for ModalLayer {
    type Action = HostAction;

    fn accepts_pointer_interaction(&self) -> bool {
        true
    }

    fn accessibility(
        &mut self,
        _ctx: &mut AccessCtx<'_>,
        _props: &PropertiesRef<'_>,
        _node: &mut AccessNode,
    ) {
    }

    fn accessibility_role(&self) -> Role {
        Role::Dialog
    }

    fn children_ids(&self) -> ChildrenIds {
        ChildrenIds::from_slice(&[self.child.id()])
    }

    fn find_widget_under_pointer<'ctx>(
        &'ctx self,
        ctx: QueryCtx<'ctx>,
        pos: Point,
    ) -> Option<WidgetRef<'ctx, dyn Widget>> {
        self.state.standing()?;
        find_widget_under_pointer(self, ctx, pos)
    }

    fn layout(
        &mut self,
        ctx: &mut LayoutCtx<'_>,
        _props: &mut PropertiesMut<'_>,
        constraints: &BoxConstraints,
    ) -> MasonrySize {
        let viewport = constraints.max();
        let standing = self.state.standing().is_some();
        ctx.set_stashed(&mut self.child, !standing);
        if !standing {
            self.state.stand(MasonryRect::ZERO);
            return viewport;
        }
        let window = Size::new(viewport.width.as_(), viewport.height.as_());
        let content = fit_content(
            ctx,
            &mut self.child,
            self.declared,
            self.chrome.room(window),
        );
        let surface = self.chrome.surface(content, window);
        let at = self.chrome.content(surface);
        ctx.place_child(&mut self.child, Point::from(at));
        self.state.stand(masonry_rect(surface));
        viewport
    }

    fn make_trace_span(&self, id: WidgetId) -> Span {
        trace_span!("KitharaModalLayer", id = id.trace())
    }

    fn on_pointer_event(
        &mut self,
        ctx: &mut EventCtx<'_>,
        _props: &mut PropertiesMut<'_>,
        _event: &PointerEvent,
    ) {
        ctx.set_handled();
    }

    /// Draws the scrim over the window, then the shadow and the framed
    /// surface under the content.
    fn paint(&mut self, ctx: &mut PaintCtx<'_>, _props: &PropertiesRef<'_>, scene: &mut Scene) {
        if self.state.standing().is_none() {
            return;
        }
        let chrome = self.chrome;
        let viewport = ctx.size();
        let mut scrim = DrawListBuilder::default();
        scrim.fill_rect(
            Rect {
                x: 0.0,
                y: 0.0,
                w: viewport.width.as_(),
                h: viewport.height.as_(),
            },
            chrome.scrim,
        );
        replay(&scrim.finish(), &mut VelloBackend::new(scene));
        let surface = self.state.surface();
        let offset = Vec2::new(
            f64::from(chrome.shadow_offset.x),
            f64::from(chrome.shadow_offset.y),
        );
        scene.draw_blurred_rounded_rect(
            Affine::IDENTITY,
            surface + offset,
            paint_color(chrome.shadow),
            f64::from(chrome.radius),
            f64::from(chrome.blur / 2.0),
        );
        let surface = draw_rect(surface);
        let inset = chrome.border_width / 2.0;
        let mut frame = DrawListBuilder::default();
        frame.fill_rounded_rect(surface, chrome.radius, chrome.background);
        frame.stroke_rounded_rect(
            Rect {
                x: surface.x + inset,
                y: surface.y + inset,
                w: surface.w - chrome.border_width,
                h: surface.h - chrome.border_width,
            },
            chrome.radius,
            chrome.border,
            chrome.border_width,
        );
        replay(&frame.finish(), &mut VelloBackend::new(scene));
    }

    /// Draws the corner ticks over the content.
    fn post_paint(
        &mut self,
        _ctx: &mut PaintCtx<'_>,
        _props: &PropertiesRef<'_>,
        scene: &mut Scene,
    ) {
        if self.state.standing().is_none() {
            return;
        }
        let mut ticks = DrawListBuilder::default();
        for tick in self.chrome.ticks(draw_rect(self.state.surface())) {
            ticks.fill_rect(tick, self.chrome.tick);
        }
        replay(&ticks.finish(), &mut VelloBackend::new(scene));
    }

    fn register_children(&mut self, ctx: &mut RegisterCtx<'_>) {
        ctx.register_child(&mut self.child);
    }
}
