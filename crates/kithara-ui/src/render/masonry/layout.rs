use masonry::{
    core::{BoxConstraints, LayoutCtx, WidgetPod},
    kurbo::{Point, Size as MasonrySize},
};
use num_traits::cast::AsPrimitive;

use super::{
    flex::{box_constraints, normalized},
    leaf::Leaf,
    node::Node,
};
use crate::solve;

pub(crate) enum NodeLayout {
    Leaf(Leaf),
    Flex(super::flex::Flex),
    Stack,
}

impl NodeLayout {
    pub(crate) fn layout(
        &mut self,
        ctx: &mut LayoutCtx<'_>,
        children: &mut [WidgetPod<Node>],
        limits: solve::Limits,
        declared: solve::Size<solve::Length>,
    ) -> solve::Size {
        match self {
            Self::Leaf(leaf) => {
                let intrinsic = leaf.measure(limits);
                limits.resolve(declared.width, declared.height, intrinsic)
            }
            Self::Flex(flex) => {
                let intrinsic = flex.layout(ctx, children, limits);
                limits.resolve(declared.width, declared.height, intrinsic)
            }
            Self::Stack => stack(ctx, children, limits, declared),
        }
    }

    pub(crate) const fn leaf(&mut self) -> Option<&mut Leaf> {
        match self {
            Self::Leaf(leaf) => Some(leaf),
            Self::Flex(_) | Self::Stack => None,
        }
    }

    pub(crate) fn accepts_input(&self) -> bool {
        matches!(self, Self::Leaf(leaf) if leaf.accepts_input())
    }

    pub(crate) fn accepts_text_input(&self) -> bool {
        matches!(self, Self::Leaf(leaf) if leaf.accepts_text_input())
    }
}

fn stack(
    ctx: &mut LayoutCtx<'_>,
    children: &mut [WidgetPod<Node>],
    limits: solve::Limits,
    declared: solve::Size<solve::Length>,
) -> solve::Size {
    let inner = normalized(limits.width(declared.width).height(declared.height).loose());
    let intrinsic = children.first_mut().map_or(solve::Size::ZERO, |first| {
        Node::set_child_limits(ctx, first, inner);
        let size = ctx.run_layout(first, &box_constraints(inner));
        solve::Size::new(size.width.as_(), size.height.as_())
    });
    let size = limits.resolve(declared.width, declared.height, intrinsic);
    let exact = solve::Limits::new(size, size);
    for child in children {
        Node::set_child_limits(ctx, child, exact);
        ctx.run_layout(
            child,
            &BoxConstraints::tight(MasonrySize::new(
                f64::from(size.width),
                f64::from(size.height),
            )),
        );
        ctx.place_child(child, Point::ORIGIN);
    }
    size
}
