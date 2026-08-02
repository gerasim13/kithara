use iced::{
    Element, Event, Length, Rectangle, Renderer, Size, Theme,
    advanced::{
        Clipboard, Shell, Widget as IcedWidget,
        layout::{self, Layout},
        mouse, renderer,
        widget::{Operation, Tree},
    },
    widget::canvas::{Canvas, Program},
};

use crate::render::UiEvent;

pub(super) trait ScrollCanvasState: Default {
    fn reconcile_scroll(
        &mut self,
        path: &str,
        row_count: usize,
        row_height: f32,
        row_right_inset: f32,
    );
    fn set_scroll_viewport(&mut self, height: f32);
}

pub(super) struct ScrollCanvas<P>
where
    P: Program<UiEvent, Theme, Renderer>,
    P::State: ScrollCanvasState,
{
    canvas: Canvas<P, UiEvent>,
    path: String,
    row_count: usize,
    row_height: f32,
    row_right_inset: f32,
}

impl<P> ScrollCanvas<P>
where
    P: Program<UiEvent, Theme, Renderer>,
    P::State: ScrollCanvasState,
{
    pub(super) fn new(
        program: P,
        path: &str,
        row_count: usize,
        row_height: f32,
        row_right_inset: f32,
    ) -> Self {
        Self {
            canvas: Canvas::new(program)
                .width(Length::Fill)
                .height(Length::Fill),
            path: path.to_owned(),
            row_count,
            row_height,
            row_right_inset,
        }
    }

    pub(super) fn view<'a>(self) -> Element<'a, UiEvent>
    where
        P: 'a,
    {
        Element::new(self)
    }
}

impl<P> IcedWidget<UiEvent, Theme, Renderer> for ScrollCanvas<P>
where
    P: Program<UiEvent, Theme, Renderer>,
    P::State: ScrollCanvasState,
{
    fn tag(&self) -> iced::advanced::widget::tree::Tag {
        self.canvas.tag()
    }

    fn state(&self) -> iced::advanced::widget::tree::State {
        let mut state = P::State::default();
        state.reconcile_scroll(
            &self.path,
            self.row_count,
            self.row_height,
            self.row_right_inset,
        );
        iced::advanced::widget::tree::State::new(state)
    }

    fn diff(&self, tree: &mut Tree) {
        self.canvas.diff(tree);
        tree.state.downcast_mut::<P::State>().reconcile_scroll(
            &self.path,
            self.row_count,
            self.row_height,
            self.row_right_inset,
        );
    }

    fn size(&self) -> Size<Length> {
        self.canvas.size()
    }

    fn size_hint(&self) -> Size<Length> {
        self.canvas.size_hint()
    }

    fn layout(
        &mut self,
        tree: &mut Tree,
        renderer: &Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        let node = self.canvas.layout(tree, renderer, limits);
        tree.state
            .downcast_mut::<P::State>()
            .set_scroll_viewport(node.size().height);
        node
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
        self.canvas.update(
            tree, event, layout, cursor, renderer, clipboard, shell, viewport,
        );
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        self.canvas
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
        self.canvas
            .draw(tree, renderer, theme, style, layout, cursor, viewport);
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout<'_>,
        _renderer: &Renderer,
        operation: &mut dyn Operation,
    ) {
        operation.custom(None, layout.bounds(), tree.state.downcast_mut::<P::State>());
    }
}
