use std::rc::Rc;

use masonry::{
    core::{BoxConstraints, LayoutCtx, WidgetPod},
    kurbo::{Point, Size as MasonrySize},
};
use num_traits::cast::AsPrimitive;

use super::{built::BlockState, node::Node};
use crate::{
    layout::Axis,
    module::MeasureAxis,
    render::document::Band,
    solve::{self, Distribution, Input, Measure},
};

pub(crate) struct Flex {
    axis: Axis,
    /// The axis whose room decides which children stand, when they come and go
    /// with the room at all.
    measure: Option<MeasureAxis>,
    width: solve::Length,
    height: solve::Length,
    padding: solve::Padding,
    spacing: f32,
    alignment: solve::Alignment,
    main_alignment: solve::Alignment,
    children: Vec<ChildLayout>,
    /// Which children the room reached, refilled at every layout. A retained
    /// flow is laid out again for every frame that resizes it, so this is
    /// storage the widget keeps rather than a list it builds each time.
    stands: Vec<bool>,
    /// Which child each item the solver asks about actually is, refilled
    /// beside `stands`.
    slots: Vec<usize>,
}

#[derive(Clone)]
pub(crate) struct ChildLayout {
    band: Band,
    /// Whether the document hides this child, when the child is a block at
    /// all. A block stands here beside the band because both answer the same
    /// question - whether this child is in the picture - and neither of them
    /// rebuilds the flow to answer it.
    block: Option<Rc<BlockState>>,
    natural: solve::Size<solve::Length>,
    declared: Option<solve::Size<solve::Length>>,
    main_minimum: Option<f32>,
    main_weight: Option<f32>,
}

impl ChildLayout {
    pub(crate) const fn natural(
        natural: solve::Size<solve::Length>,
        main_minimum: Option<f32>,
    ) -> Self {
        Self {
            band: Band::ALWAYS,
            block: None,
            natural,
            declared: None,
            main_minimum,
            main_weight: None,
        }
    }

    pub(crate) const fn weighted(
        natural: solve::Size<solve::Length>,
        declared: solve::Size<solve::Length>,
        main_weight: f32,
    ) -> Self {
        Self {
            band: Band::ALWAYS,
            block: None,
            natural,
            declared: Some(declared),
            main_minimum: None,
            main_weight: Some(main_weight),
        }
    }

    /// The band of room this child stands in.
    pub(crate) const fn within(mut self, band: Band) -> Self {
        self.band = band;
        self
    }

    /// The block that says whether the document shows this child at all.
    pub(crate) fn blocked(mut self, block: Option<Rc<BlockState>>) -> Self {
        self.block = block;
        self
    }
}

impl Flex {
    pub(crate) const fn new(
        axis: Axis,
        width: solve::Length,
        height: solve::Length,
        padding: solve::Padding,
        spacing: f32,
        alignment: solve::Alignment,
        children: Vec<ChildLayout>,
    ) -> Self {
        Self {
            axis,
            measure: None,
            width,
            height,
            padding,
            spacing,
            alignment,
            main_alignment: solve::Alignment::Start,
            children,
            stands: Vec::new(),
            slots: Vec::new(),
        }
    }

    /// Names the axis whose room decides which children stand.
    pub(crate) const fn measure(mut self, axis: Option<MeasureAxis>) -> Self {
        self.measure = axis;
        self
    }

    /// Records which children stand: the ones the room reached and the
    /// document did not hide.
    fn stand(&mut self, limits: solve::Limits) {
        self.stands.clear();
        let room = self.measure.map(|axis| match axis {
            MeasureAxis::Width => limits.max().width,
            MeasureAxis::Height => limits.max().height,
        });
        self.stands.extend(self.children.iter().map(|child| {
            let reached = room.is_none_or(|room| child.band.stands(room));
            let shown = !child.block.as_ref().is_some_and(|block| block.is_hidden());
            reached && shown
        }));
    }

    pub(crate) const fn align_main(mut self, alignment: solve::Alignment) -> Self {
        self.main_alignment = alignment;
        self
    }

    pub(crate) fn layout(
        &mut self,
        ctx: &mut LayoutCtx<'_>,
        children: &mut [WidgetPod<Node>],
        limits: solve::Limits,
    ) -> solve::Size {
        let outer_limits = limits.width(self.width).height(self.height);
        let inner_limits = outer_limits.shrink(self.padding).loose();
        self.stand(outer_limits);
        let Self { stands, slots, .. } = &mut *self;
        slots.clear();
        slots.extend(
            stands
                .iter()
                .enumerate()
                .filter_map(|(index, on)| on.then_some(index)),
        );
        // A cell the room did not reach is stashed, not placed at an empty box:
        // an empty box still lets every leaf inside it draw at its own natural
        // size, and it drew them at the flow's own origin. Masonry skips a
        // stashed child for layout, paint and accessibility, which is what the
        // immediate host does by leaving the cell out of the flow entirely.
        //
        // This stands before the solver, which measures a standing cell by
        // laying it out: a cell the room has just grown to reach has to be back
        // in the picture before anything asks it for a size.
        for (index, on) in self.stands.iter().enumerate() {
            ctx.set_stashed(&mut children[index], !on);
        }
        let items = self
            .slots
            .iter()
            .map(|slot| {
                let layout = &self.children[*slot];
                let declared = layout.declared.unwrap_or(layout.natural);
                layout.main_weight.map_or_else(
                    || solve::Item::new(declared, layout.main_minimum),
                    |weight| solve::Item::weighted(declared, weight),
                )
            })
            .collect();
        let mut measure = MasonryMeasure {
            children,
            layouts: &self.children,
            stood: vec![None; self.slots.len()],
            slots: &self.slots,
            ctx,
        };
        let Distribution { size, mut items } = solve::resolve(
            Input {
                axis: self.axis,
                limits: &inner_limits,
                width: self.width,
                height: self.height,
                padding: solve::Padding::default(),
                spacing: self.spacing,
                align_items: self.alignment,
                items,
            },
            &mut measure,
        );
        let content_main = items.last().map_or(0.0, |item| match self.axis {
            Axis::Horizontal => item.offset.x + item.size.width,
            Axis::Vertical => item.offset.y + item.size.height,
        });
        let available_main = match self.axis {
            Axis::Horizontal => size.width,
            Axis::Vertical => size.height,
        };
        let free = (available_main - content_main).max(0.0);
        let offset = match self.main_alignment {
            solve::Alignment::Start => 0.0,
            solve::Alignment::Center => free / 2.0,
            solve::Alignment::End => free,
        };
        for item in &mut items {
            match self.axis {
                Axis::Horizontal => item.offset.x += offset,
                Axis::Vertical => item.offset.y += offset,
            }
        }
        let fitted = fit_padding(self.padding, size, outer_limits.max());
        for (index, (slot, item)) in self.slots.iter().zip(items).enumerate() {
            let placed = match measure.stood[index] {
                Some((limits, size)) if size == item.size => limits,
                _ => solve::Limits::new(item.size, item.size),
            };
            let child = &mut measure.children[*slot];
            Node::set_child_limits(measure.ctx, child, placed);
            measure.ctx.run_layout(child, &box_constraints(placed));
            measure.ctx.place_child(
                child,
                Point::new(
                    f64::from(item.offset.x + fitted.left),
                    f64::from(item.offset.y + fitted.top),
                ),
            );
        }

        outer_limits
            .shrink(fitted)
            .resolve(self.width, self.height, size)
            .expand(fitted)
    }
}

struct MasonryMeasure<'a, 'ctx> {
    children: &'a mut [WidgetPod<Node>],
    layouts: &'a [ChildLayout],
    /// Which child each item the solver asks about actually is: the solver sees
    /// only the cells that stand.
    slots: &'a [usize],
    /// Where measuring left each cell: the limits it was last laid out under
    /// and the size it answered there.
    ///
    /// Measuring a cell lays out everything inside it, and so does placing it,
    /// so a cell placed at a size it was not measured at is walked twice - and
    /// a cell inside that cell is walked twice for each of those, which is how
    /// a page seven boxes deep comes to lay itself out seventy-five times over.
    /// A cell the solver hands exactly the size it answered with is already
    /// standing right, and asking Masonry for that same box again is answered
    /// from its own layout cache rather than by walking the cell again.
    stood: Vec<Option<(solve::Limits, solve::Size)>>,
    ctx: &'a mut LayoutCtx<'ctx>,
}

impl Measure for MasonryMeasure<'_, '_> {
    fn measure(&mut self, index: usize, limits: &solve::Limits) -> solve::Size {
        let slot = self.slots[index];
        let declared = self.layouts[slot].declared;
        let child_limits = declared.map_or(*limits, |declared| {
            limits.width(declared.width).height(declared.height).loose()
        });
        let child_limits = normalized(child_limits);
        let child = &mut self.children[slot];
        Node::set_child_limits(self.ctx, child, child_limits);
        let intrinsic = self.ctx.run_layout(child, &box_constraints(child_limits));
        let intrinsic = solve::Size::new(intrinsic.width.as_(), intrinsic.height.as_());
        self.stood[index] = Some((child_limits, intrinsic));
        declared.map_or(intrinsic, |declared| {
            limits
                .width(declared.width)
                .height(declared.height)
                .resolve(declared.width, declared.height, intrinsic)
        })
    }
}

pub(crate) fn box_constraints(limits: solve::Limits) -> BoxConstraints {
    let limits = normalized(limits);
    BoxConstraints::new(
        MasonrySize::new(
            f64::from(limits.min().width),
            f64::from(limits.min().height),
        ),
        MasonrySize::new(
            f64::from(limits.max().width),
            f64::from(limits.max().height),
        ),
    )
}

pub(crate) fn normalized(limits: solve::Limits) -> solve::Limits {
    let min = solve::Size::new(limits.min().width.max(0.0), limits.min().height.max(0.0));
    let max = solve::Size::new(
        limits.max().width.max(min.width),
        limits.max().height.max(min.height),
    );
    solve::Limits::with_compression(min, max, limits.compression())
}

fn fit_padding(padding: solve::Padding, inner: solve::Size, outer: solve::Size) -> solve::Padding {
    let available_width = (outer.width - inner.width).max(0.0);
    let available_height = (outer.height - inner.height).max(0.0);
    let top = padding.top.min(available_height);
    let left = padding.left.min(available_width);
    solve::Padding {
        top,
        right: padding.right.min(available_width - left),
        bottom: padding.bottom.min(available_height - top),
        left,
    }
}
