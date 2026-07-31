use iced::{
    Alignment, Element, Event, Length, Padding, Rectangle, Renderer, Size, Theme, Vector,
    advanced::{
        Clipboard, Shell, Widget as IcedWidget,
        layout::{self, Layout},
        mouse, overlay, renderer,
        widget::{self, Operation, Tree},
    },
};

use crate::{
    layout::Axis,
    render::UiEvent,
    solve::{self, Distribution, Input, Measure},
};

pub(super) struct Flex<'a> {
    axis: Axis,
    spacing: f32,
    padding: Padding,
    width: Length,
    height: Length,
    align: Alignment,
    children: Vec<Element<'a, UiEvent>>,
}

impl<'a> Flex<'a> {
    pub(super) fn row(children: impl IntoIterator<Item = Element<'a, UiEvent>>) -> Self {
        Self::with_children(Axis::Horizontal, children)
    }

    pub(super) fn column(children: impl IntoIterator<Item = Element<'a, UiEvent>>) -> Self {
        Self::with_children(Axis::Vertical, children)
    }

    fn with_children(axis: Axis, children: impl IntoIterator<Item = Element<'a, UiEvent>>) -> Self {
        let iterator = children.into_iter();
        let mut flex = Self {
            axis,
            spacing: 0.0,
            padding: Padding::ZERO,
            width: Length::Shrink,
            height: Length::Shrink,
            align: Alignment::Start,
            children: Vec::with_capacity(iterator.size_hint().0),
        };

        for child in iterator {
            flex = flex.push(child);
        }

        flex
    }

    fn push(mut self, child: Element<'a, UiEvent>) -> Self {
        let child_size = child.as_widget().size_hint();

        if !child_size.is_void() {
            self.width = self.width.enclose(child_size.width);
            self.height = self.height.enclose(child_size.height);
            self.children.push(child);
        }

        self
    }

    pub(super) fn spacing(mut self, spacing: f32) -> Self {
        self.spacing = spacing;
        self
    }

    pub(super) fn width(mut self, width: Length) -> Self {
        self.width = width;
        self
    }

    pub(super) fn height(mut self, height: Length) -> Self {
        self.height = height;
        self
    }

    pub(super) fn align(mut self, alignment: Alignment) -> Self {
        self.align = alignment;
        self
    }
}

impl IcedWidget<UiEvent, Theme, Renderer> for Flex<'_> {
    fn size(&self) -> Size<Length> {
        Size::new(self.width, self.height)
    }

    fn size_hint(&self) -> Size<Length> {
        Size::new(self.width, self.height)
    }

    fn layout(
        &mut self,
        tree: &mut Tree,
        renderer: &Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        let limits = match self.axis {
            Axis::Horizontal => *limits,
            Axis::Vertical => limits.max_width(f32::INFINITY),
        };
        let items = self
            .children
            .iter()
            .map(|child| solve::Item::new(child.as_widget().size()))
            .collect::<Vec<_>>();
        let mut measure = IcedMeasure {
            children: &mut self.children,
            trees: &mut tree.children,
            renderer,
            nodes: vec![layout::Node::default(); items.len()],
        };
        let Distribution {
            size,
            items: placements,
        } = solve::resolve(
            Input {
                axis: self.axis,
                limits: &limits,
                width: self.width,
                height: self.height,
                padding: self.padding,
                spacing: self.spacing,
                align_items: self.align,
                items,
            },
            &mut measure,
        );
        let mut nodes = measure.nodes;

        for (node, placement) in nodes.iter_mut().zip(placements) {
            node.move_to_mut(placement.offset);
        }

        layout::Node::with_children(size, nodes)
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
        if layout.bounds().intersection(viewport).is_some() {
            for ((child, tree), layout) in self
                .children
                .iter()
                .zip(&tree.children)
                .zip(layout.children())
                .filter(|(_, layout)| layout.bounds().intersects(viewport))
            {
                child
                    .as_widget()
                    .draw(tree, renderer, theme, style, layout, cursor, viewport);
            }
        }
    }

    fn tag(&self) -> widget::tree::Tag {
        widget::tree::Tag::stateless()
    }

    fn state(&self) -> widget::tree::State {
        widget::tree::State::None
    }

    fn children(&self) -> Vec<Tree> {
        self.children.iter().map(Tree::new).collect()
    }

    fn diff(&self, tree: &mut Tree) {
        tree.diff_children(&self.children);
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout<'_>,
        renderer: &Renderer,
        operation: &mut dyn Operation,
    ) {
        operation.container(None, layout.bounds());
        operation.traverse(&mut |operation| {
            self.children
                .iter_mut()
                .zip(&mut tree.children)
                .zip(layout.children())
                .for_each(|((child, state), layout)| {
                    child
                        .as_widget_mut()
                        .operate(state, layout, renderer, operation);
                });
        });
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
        for ((child, tree), layout) in self
            .children
            .iter_mut()
            .zip(&mut tree.children)
            .zip(layout.children())
        {
            child.as_widget_mut().update(
                tree, event, layout, cursor, renderer, clipboard, shell, viewport,
            );
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
        self.children
            .iter()
            .zip(&tree.children)
            .zip(layout.children())
            .map(|((child, tree), layout)| {
                child
                    .as_widget()
                    .mouse_interaction(tree, layout, cursor, viewport, renderer)
            })
            .max()
            .unwrap_or_default()
    }

    fn overlay<'a>(
        &'a mut self,
        tree: &'a mut Tree,
        layout: Layout<'a>,
        renderer: &Renderer,
        viewport: &Rectangle,
        translation: Vector,
    ) -> Option<overlay::Element<'a, UiEvent, Theme, Renderer>> {
        overlay::from_children(
            &mut self.children,
            tree,
            layout,
            renderer,
            viewport,
            translation,
        )
    }
}

impl<'a> From<Flex<'a>> for Element<'a, UiEvent> {
    fn from(flex: Flex<'a>) -> Self {
        Self::new(flex)
    }
}

struct IcedMeasure<'a, 'element> {
    children: &'a mut [Element<'element, UiEvent>],
    trees: &'a mut [Tree],
    renderer: &'a Renderer,
    nodes: Vec<layout::Node>,
}

impl Measure for IcedMeasure<'_, '_> {
    fn measure(&mut self, index: usize, limits: &layout::Limits) -> Size {
        let node = self.children[index].as_widget_mut().layout(
            &mut self.trees[index],
            self.renderer,
            limits,
        );
        let size = node.size();
        self.nodes[index] = node;
        size
    }
}
