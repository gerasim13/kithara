use std::cell::RefCell;

use iced::{
    Element, Event, Length, Rectangle, Renderer, Size, Theme, Vector,
    advanced::{
        Renderer as _, Widget as IcedWidget,
        graphics::geometry::Renderer as _,
        layout::{self, Layout},
        renderer,
        widget::{self, Tree},
    },
    mouse::{Cursor, Interaction},
    widget::canvas::{self, Action, Canvas, Frame, Geometry},
};

use crate::{
    atoms::{
        chrome::{ChromeChevron, ChromeLabel, footer_role},
        text::Text,
    },
    backends::replay_ordered,
    draw::{DrawListBuilder, Rect},
    interact::{CursorShape, Hover, iced as iced_interact, recognizers::click},
    render::{InputOwner, Skin, UiEvent, controls::snapped, toggle_module},
    shaping::TextContext,
};

/// The footer carries the one word a module resolves for itself, so it owns it
/// where the rest of the chrome borrows what the document already interned.
#[derive(Clone)]
pub(crate) enum ChromeLeaf<'a> {
    Chip(&'a str),
    Title(&'a str),
    Footer(String),
    HorizontalLine,
    VerticalLine,
}

pub(crate) fn chrome_leaf<'a>(leaf: ChromeLeaf<'a>, skin: &'a Skin) -> Element<'a, UiEvent> {
    Element::new(LeafPaint { leaf, skin })
}

struct LeafPaint<'data, 'skin> {
    leaf: ChromeLeaf<'data>,
    skin: &'skin Skin,
}

#[derive(Default)]
struct LeafState {
    text: RefCell<Option<TextContext>>,
}

impl LeafPaint<'_, '_> {
    fn lengths(&self) -> Size<Length> {
        match &self.leaf {
            ChromeLeaf::Chip(_) | ChromeLeaf::Title(_) => Size::new(Length::Shrink, Length::Fill),
            ChromeLeaf::Footer(_) => Size::new(Length::Fill, Length::Fill),
            ChromeLeaf::HorizontalLine => Size::new(
                Length::Fill,
                Length::Fixed(self.skin.chrome.inner_line_width),
            ),
            ChromeLeaf::VerticalLine => Size::new(
                Length::Fixed(self.skin.chrome.inner_line_width),
                Length::Fill,
            ),
        }
    }

    /// The label this leaf draws, and what it says. A line has neither, and a
    /// footer is a word with no box around it.
    fn label(&self) -> Option<(ChromeLabel, &str)> {
        match &self.leaf {
            ChromeLeaf::Chip(label) => Some((ChromeLabel::chip(self.skin), label)),
            ChromeLeaf::Title(title) => Some((ChromeLabel::title(self.skin), title)),
            ChromeLeaf::Footer(_) | ChromeLeaf::HorizontalLine | ChromeLeaf::VerticalLine => None,
        }
    }

    fn paint(&self, builder: &mut DrawListBuilder, text: &mut TextContext, bounds: Rect) {
        if let Some((label, content)) = self.label() {
            label.paint(builder, text, content, bounds);
            return;
        }
        match &self.leaf {
            ChromeLeaf::Footer(content) => Text::new(
                content,
                footer_role(self.skin),
                self.skin.chrome.footer_pad,
                self.skin,
            )
            .paint(builder, text, bounds),
            _ => builder.fill_rect(bounds, self.skin.rgba(self.skin.chrome.inner_line)),
        }
    }
}

impl IcedWidget<UiEvent, Theme, Renderer> for LeafPaint<'_, '_> {
    fn size(&self) -> Size<Length> {
        self.lengths()
    }

    fn tag(&self) -> widget::tree::Tag {
        widget::tree::Tag::of::<LeafState>()
    }

    fn state(&self) -> widget::tree::State {
        widget::tree::State::new(LeafState::default())
    }

    fn layout(
        &mut self,
        tree: &mut Tree,
        _renderer: &Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        let intrinsic = if let Some((label, content)) = self.label() {
            let state = tree.state.downcast_mut::<LeafState>();
            let mut text = state.text.borrow_mut();
            let text = text.get_or_insert_with(|| self.skin.text_resources().into());
            let (width, height) = label.intrinsic(text, content);
            Size::new(width, height)
        } else {
            Size::ZERO
        };
        let lengths = self.lengths();
        layout::Node::new(limits.resolve(lengths.width, lengths.height, intrinsic))
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
        let state = tree.state.downcast_ref::<LeafState>();
        let mut text = state.text.borrow_mut();
        let text = text.get_or_insert_with(|| self.skin.text_resources().into());
        renderer.with_translation(Vector::new(bounds.x, bounds.y), |renderer| {
            let mut frame = Frame::new(renderer, bounds.size());
            let mut builder = DrawListBuilder::default();
            self.paint(
                &mut builder,
                text,
                Rect {
                    h: bounds.height,
                    w: bounds.width,
                    x: 0.0,
                    y: 0.0,
                },
            );
            replay_ordered(&builder.finish(), &mut frame, self.skin.text_resources());
            renderer.draw_geometry(frame.into_geometry());
        });
    }
}

pub(crate) fn header_chevron<'a>(
    module: &str,
    collapsed: bool,
    skin: &'a Skin,
    owner: InputOwner,
) -> Element<'a, UiEvent> {
    let paint = ChevronPaint {
        chevron: ChromeChevron::new(skin),
        collapsed,
        skin,
    };
    match owner {
        InputOwner::Leaf => Canvas::new(ChevronProgram {
            module: module.to_owned(),
            paint,
        })
        .width(Length::Fill)
        .height(Length::Fill)
        .into(),
        InputOwner::Engine => Canvas::new(paint)
            .width(Length::Fill)
            .height(Length::Fill)
            .into(),
    }
}

struct ChevronProgram<'skin> {
    module: String,
    paint: ChevronPaint<'skin>,
}

impl canvas::Program<UiEvent> for ChevronProgram<'_> {
    type State = ();

    fn draw(
        &self,
        _state: &(),
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: Cursor,
    ) -> Vec<Geometry> {
        self.paint.geometry(renderer, bounds)
    }

    fn mouse_interaction(&self, _state: &(), bounds: Rectangle, cursor: Cursor) -> Interaction {
        Hover::new(CursorShape::Pointer)
            .cursor(false, &iced_interact::hit(bounds, cursor))
            .into()
    }

    fn update(
        &self,
        _state: &mut (),
        event: &Event,
        bounds: Rectangle,
        cursor: Cursor,
    ) -> Option<Action<UiEvent>> {
        let input = iced_interact::input(event)?;
        let hit = iced_interact::hit(bounds, cursor);
        toggle_module(&self.module, click::on_input(input, &hit))
    }
}

/// The chevron drawn over the header, through the same neutral mark the
/// retained host draws in its own cell.
struct ChevronPaint<'skin> {
    chevron: ChromeChevron,
    collapsed: bool,
    skin: &'skin Skin,
}

impl ChevronPaint<'_> {
    fn geometry(&self, renderer: &Renderer, bounds: Rectangle) -> Vec<Geometry> {
        let mut frame = Frame::new(renderer, bounds.size());
        let mut list = DrawListBuilder::default();
        self.chevron.paint(
            &mut list,
            Rect {
                h: bounds.height,
                w: bounds.width,
                x: 0.0,
                y: 0.0,
            },
            self.collapsed,
        );
        replay_ordered(&list.finish(), &mut frame, self.skin.text_resources());
        vec![frame.into_geometry()]
    }
}

impl canvas::Program<UiEvent> for ChevronPaint<'_> {
    type State = ();

    fn draw(
        &self,
        _state: &(),
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: Cursor,
    ) -> Vec<Geometry> {
        self.geometry(renderer, bounds)
    }
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;

    use iced::{
        Pixels, Point,
        advanced::{graphics::text::font_system, layout::Limits, widget::Tree},
        alignment::Vertical,
        event, mouse,
        widget::container,
        window::RedrawRequest,
    };
    use iced_renderer::fallback::Renderer as FallbackRenderer;
    use iced_tiny_skia::Renderer as TinySkiaRenderer;
    use kithara_test_utils::kithara;

    use super::*;
    use crate::{
        builtin,
        draw::DrawCmd,
        render::{IcedSkin, fonts},
        skin::FontWeight,
    };

    /// The word iced lays out on its own, against which the painted leaf is
    /// measured: shaped the way every string this toolkit sets is shaped.
    fn shaped(content: &'static str) -> iced::widget::Text<'static> {
        iced::widget::Text::new(content).shaping(iced::widget::text::Shaping::Advanced)
    }

    fn headless_renderer() -> Renderer {
        let mut system = font_system()
            .write()
            .unwrap_or_else(|error| panic!("iced font system must be available: {error}"));
        for bytes in fonts::FONT_BYTES {
            system.load_font(Cow::Borrowed(bytes));
        }
        drop(system);

        FallbackRenderer::Secondary(TinySkiaRenderer::new(fonts::SANS, Pixels(14.0)))
    }

    fn measured_size(mut element: Element<'_, UiEvent>, renderer: &Renderer) -> Size {
        let mut tree = Tree::new(element.as_widget());
        element
            .as_widget_mut()
            .layout(
                &mut tree,
                renderer,
                &Limits::new(Size::ZERO, Size::new(320.0, 80.0)),
            )
            .size()
    }

    #[kithara::test]
    fn chrome_leaves_paint_through_the_retained_builder() {
        let skin = builtin::skin();
        let leaf = LeafPaint {
            leaf: ChromeLeaf::Chip("FX"),
            skin,
        };
        let mut builder = DrawListBuilder::default();
        let mut text = TextContext::from(skin.text_resources());
        leaf.paint(
            &mut builder,
            &mut text,
            Rect {
                h: skin.chrome.header_height,
                w: 40.0,
                x: 0.0,
                y: 0.0,
            },
        );
        let list = builder.finish();

        assert!(matches!(
            list.commands(),
            [DrawCmd::Fill { .. }, DrawCmd::Text { content, .. }] if content == "FX"
        ));

        let line = LeafPaint {
            leaf: ChromeLeaf::HorizontalLine,
            skin,
        };
        let mut builder = DrawListBuilder::default();
        line.paint(
            &mut builder,
            &mut text,
            Rect {
                h: skin.chrome.inner_line_width,
                w: 80.0,
                x: 0.0,
                y: 0.0,
            },
        );
        assert!(matches!(
            builder.finish().commands(),
            [DrawCmd::Fill { .. }]
        ));

        let title = LeafPaint {
            leaf: ChromeLeaf::Title("DECK"),
            skin,
        };
        let mut builder = DrawListBuilder::default();
        title.paint(
            &mut builder,
            &mut text,
            Rect {
                h: skin.chrome.header_height,
                w: 80.0,
                x: 0.0,
                y: 0.0,
            },
        );
        assert!(matches!(
            builder.finish().commands(),
            [DrawCmd::Fill { .. }, DrawCmd::Text { content, .. }] if content == "DECK"
        ));

        let line = LeafPaint {
            leaf: ChromeLeaf::VerticalLine,
            skin,
        };
        let mut builder = DrawListBuilder::default();
        line.paint(
            &mut builder,
            &mut text,
            Rect {
                h: skin.chrome.header_height,
                w: skin.chrome.inner_line_width,
                x: 0.0,
                y: 0.0,
            },
        );
        assert!(matches!(
            builder.finish().commands(),
            [DrawCmd::Fill { .. }]
        ));
    }

    #[kithara::test]
    fn painted_header_text_keeps_the_iced_intrinsic_size() {
        let skin = builtin::skin();
        let metrics = skin.chrome;
        let renderer = headless_renderer();
        let chip: Element<'_, UiEvent> = container(
            shaped("FX")
                .font(fonts::mono(FontWeight::Normal))
                .size(metrics.chip_text_size)
                .color(skin.color(metrics.chip_text)),
        )
        .padding([0.0, metrics.chip_pad])
        .height(Length::Fill)
        .align_y(Vertical::Center)
        .into();
        let title: Element<'_, UiEvent> = container(
            shaped("DECK")
                .font(fonts::display(FontWeight::Medium))
                .size(metrics.title_text_size)
                .color(skin.color(metrics.title_text)),
        )
        .padding([0.0, metrics.chip_pad])
        .height(Length::Fill)
        .align_y(Vertical::Center)
        .into();

        for (old, painted) in [
            (chip, chrome_leaf(ChromeLeaf::Chip("FX"), skin)),
            (title, chrome_leaf(ChromeLeaf::Title("DECK"), skin)),
        ] {
            let old = measured_size(old, &renderer);
            let painted = measured_size(painted, &renderer);
            assert!((painted.width - old.width).abs() < 0.001);
            assert!((painted.height - old.height).abs() < 0.001);
        }
    }

    #[kithara::test]
    fn the_leaf_header_canvas_publishes_the_module_toggle() {
        let program = ChevronProgram {
            module: "app-deck".to_owned(),
            paint: ChevronPaint {
                chevron: ChromeChevron::new(builtin::skin()),
                collapsed: false,
                skin: builtin::skin(),
            },
        };
        let bounds = Rectangle {
            height: 28.0,
            width: 240.0,
            x: 0.0,
            y: 0.0,
        };
        let cursor = Cursor::Available(Point::new(20.0, 14.0));
        let press = Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left));

        let action = canvas::Program::update(&program, &mut (), &press, bounds, cursor)
            .unwrap_or_else(|| panic!("a press anywhere in the header must toggle its module"));

        assert_eq!(
            action.into_inner(),
            (
                Some(UiEvent::ToggleModule("app-deck".to_owned())),
                RedrawRequest::Wait,
                event::Status::Captured,
            )
        );
    }
}
