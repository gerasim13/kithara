use std::cell::RefCell;

use iced::{
    Element, Length, Rectangle, Renderer, Size, Theme, Vector,
    advanced::{
        Renderer as _, Widget as IcedWidget,
        graphics::geometry::Renderer as _,
        layout::{self, Layout},
        mouse, renderer,
        widget::{self, Tree},
    },
    mouse::Interaction,
    widget::{Space, canvas::Frame, mouse_area},
};

use crate::{
    atoms::tab::TabLarge,
    backends::IcedBackend,
    draw::{DrawList, DrawListBuilder, Rect, replay},
    render::{ControlAction, InputOwner, ReadValue, Skin, UiEvent, control_event},
    text::TextContext,
};

pub(crate) fn tab_large<'a>(
    path: &'a str,
    label: &'a str,
    value: Option<&ReadValue<'_>>,
    skin: &'a Skin,
    owner: InputOwner,
) -> Element<'a, UiEvent> {
    let Some(ReadValue::Bool(active)) = value else {
        return Space::new().into();
    };
    let painted: Element<'a, UiEvent> = Painted {
        active: *active,
        label,
        tab: TabLarge::new(skin),
        skin,
    }
    .into();

    match owner {
        InputOwner::Leaf => mouse_area(painted)
            .on_press(control_event(path, ControlAction::Activate))
            .interaction(Interaction::Pointer)
            .into(),
        InputOwner::Engine => painted,
    }
}

struct Painted<'data, 'skin> {
    active: bool,
    label: &'data str,
    tab: TabLarge,
    skin: &'skin Skin,
}

impl Painted<'_, '_> {
    fn draw_list(&self, text: &mut TextContext, bounds: Rect) -> DrawList {
        let mut builder = DrawListBuilder::default();
        self.tab
            .paint(&mut builder, text, self.label, self.active, bounds);
        builder.finish()
    }
}

#[derive(Default)]
struct State {
    text: RefCell<Option<TextContext>>,
}

impl IcedWidget<UiEvent, Theme, Renderer> for Painted<'_, '_> {
    fn size(&self) -> Size<Length> {
        Size::new(Length::Shrink, Length::Fixed(self.skin.tab_large.height))
    }

    fn tag(&self) -> widget::tree::Tag {
        widget::tree::Tag::of::<State>()
    }

    fn state(&self) -> widget::tree::State {
        widget::tree::State::new(State::default())
    }

    fn layout(
        &mut self,
        tree: &mut Tree,
        _renderer: &Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        let state = tree.state.downcast_mut::<State>();
        let mut text = state.text.borrow_mut();
        let text = text.get_or_insert_with(|| self.skin.text_resources().into());
        let (width, height) = self.tab.measure(text, self.label);
        layout::Node::new(limits.resolve(
            Length::Shrink,
            Length::Fixed(height),
            Size::new(width, height),
        ))
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut Renderer,
        _theme: &Theme,
        _style: &renderer::Style,
        layout: Layout<'_>,
        _cursor: mouse::Cursor,
        _viewport: &Rectangle,
    ) {
        let bounds = layout.bounds();
        if bounds.width < 1.0 || bounds.height < 1.0 {
            return;
        }

        let state = tree.state.downcast_ref::<State>();
        let mut text = state.text.borrow_mut();
        let text = text.get_or_insert_with(|| self.skin.text_resources().into());
        renderer.with_translation(Vector::new(bounds.x, bounds.y), |renderer| {
            let mut frame = Frame::new(renderer, bounds.size());
            let list = self.draw_list(
                text,
                Rect {
                    h: bounds.height,
                    w: bounds.width,
                    x: 0.0,
                    y: 0.0,
                },
            );
            replay(
                &list,
                &mut IcedBackend::new(&mut frame, self.skin.text_resources()),
            );
            renderer.draw_geometry(frame.into_geometry());
        });
    }
}

impl<'a> From<Painted<'a, 'a>> for Element<'a, UiEvent> {
    fn from(painted: Painted<'a, 'a>) -> Self {
        Self::new(painted)
    }
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;

    use iced::{
        Pixels,
        advanced::{graphics::text::font_system, layout::Limits, widget::Tree},
    };
    use iced_renderer::fallback::Renderer as FallbackRenderer;
    use iced_tiny_skia::Renderer as TinySkiaRenderer;
    use kithara_test_utils::kithara;

    use super::*;
    use crate::{
        builtin,
        render::fonts::{FONT_BYTES, SANS},
    };

    fn headless_renderer() -> Renderer {
        let mut fonts = font_system()
            .write()
            .unwrap_or_else(|error| panic!("iced font system must be available: {error}"));
        for bytes in FONT_BYTES {
            fonts.load_font(Cow::Borrowed(bytes));
        }
        drop(fonts);

        FallbackRenderer::Secondary(TinySkiaRenderer::new(SANS, Pixels(14.0)))
    }

    #[kithara::test]
    fn hosted_and_leaf_tabs_keep_the_iced_intrinsic_width() {
        let skin = builtin::skin();
        let value = ReadValue::Bool(true);
        let renderer = headless_renderer();

        for (owner, mode) in [(InputOwner::Leaf, "leaf"), (InputOwner::Engine, "hosted")] {
            let mut element = tab_large(
                "modules-tabs/deck-micro",
                "DECK MICRO",
                Some(&value),
                skin,
                owner,
            );
            let mut tree = Tree::new(element.as_widget());
            let node = element.as_widget_mut().layout(
                &mut tree,
                &renderer,
                &Limits::new(Size::ZERO, Size::new(320.0, 80.0)),
            );
            let size = node.size();

            assert!(
                (size.width - 94.0).abs() < 0.001,
                "iced gave this label and skin 94 px; a {} px {mode} tab would move its \
                 siblings when hosted",
                size.width
            );
            assert_eq!(size.height, 28.0);
        }
    }

    #[cfg(feature = "masonry-host")]
    #[kithara::test]
    fn iced_and_masonry_record_the_same_tab() {
        let skin = builtin::skin();
        let bounds = Rect {
            h: skin.tab_large.height,
            w: 94.0,
            x: 0.0,
            y: 0.0,
        };
        for active in [false, true] {
            let mut text = TextContext::from(skin.text_resources());
            let iced = Painted {
                active,
                label: "DECK MICRO",
                tab: TabLarge::new(skin),
                skin,
            }
            .draw_list(&mut text, bounds);
            let mut masonry = crate::render::masonry::Painted::new(
                TabLarge::new(skin),
                crate::atoms::painter::Labelled {
                    active,
                    label: "DECK MICRO".to_owned(),
                },
                skin,
            );

            assert_eq!(
                iced,
                crate::render::masonry::MasonryControl::draw_list(&mut masonry, bounds)
            );
        }
    }
}
