use iced::{
    Size,
    advanced::{
        layout::{Layout, Limits},
        widget::Tree,
    },
};
use kithara_test_utils::kithara;
use num_traits::cast::AsPrimitive;

use super::shared::{Endpoints, collect_rows, renderer, snapped};
use crate::{
    app::{App, Config, Ui},
    builtin,
    compile::{CompiledUi, compile},
    draw::{DrawListBuilder, Rect},
    render::{
        Clock, ReadValue, Reads, Skin, UiEvent,
        custom::{CustomKinds, CustomWidget, Size2, SizeLimits, TextMeasurer},
        tree,
    },
    shaping::TextContext,
    source::{MemResolver, UiConfig},
};

/// A document naming content the toolkit does not own, so both hosts have to
/// say what box the application's own widget was measured into.
struct Extension;

impl Extension {
    /// What the extension shapes to say how big it is, so the box it is given
    /// carries the host's own text metrics rather than a number it invented.
    const CAPTION: &'static str = "MEASURED BY THE HOST";
    /// The room the window leaves, wider and taller than the caption needs.
    const CASE: (u32, u32) = (200, 60);
    /// The name the document knows this extension by.
    const KIND: &'static str = "parity-extension";
    /// Padding the extension adds around what it shaped.
    const PAD: f32 = 4.0;

    fn kinds() -> CustomKinds {
        CustomKinds::default().with(Self::KIND, || Caption, |()| UiEvent::OpenSettings)
    }

    fn document() -> MemResolver {
        let mut resolver = MemResolver::default();
        resolver.insert(
            "extension.klayout.ron",
            r#"(schema: "kithara.layout", version: 1, id: "extension",
                root: Module(instance: "page", source: "extension.kmodule.ron",
                    size: (w: Fill, h: Fill)))"#,
        );
        resolver.insert(
            "extension.kmodule.ron",
            r#"(schema: "kithara.module", version: 1, id: "extension", chrome: Plain,
                root: Row(size: (w: Fill, h: Fill), gap: 0.0, pad: 0.0, children: [
                    Custom(id: "drawn", kind: "parity-extension",
                        size: Some((w: Shrink, h: Shrink))),
                ]))"#,
        );
        resolver
    }

    fn compiled() -> CompiledUi {
        compile(
            "extension.klayout.ron",
            &Self::document(),
            &Endpoints::default(),
            builtin::skin_doc(),
            builtin::text_doc(),
            &UiConfig::builder()
                .custom_kinds([Self::KIND.to_owned()].into_iter().collect())
                .build(),
        )
        .unwrap_or_else(|error| panic!("the extension fixture must compile: {error}"))
    }

    /// The size the extension asks for, shaped outside either host so a box
    /// both hosts got wrong the same way cannot pass for agreement.
    fn asked() -> Size2 {
        let (width, height) = Self::CASE;
        let mut context = TextContext::from(builtin::skin().text_resources());
        Caption.measure(
            &mut TextMeasurer::new(&mut context),
            SizeLimits::new(Size2::default(), Size2::new(width.as_(), height.as_())),
        )
    }

    /// The box the retained host measured the extension into.
    fn retained() -> Rect {
        let (width, height) = Self::CASE;
        let endpoints = Endpoints::default();
        let resolver = Self::document();
        let kinds = Self::kinds();
        let mut ui = Ui::new(
            Page,
            Config::builder()
                .endpoints(&endpoints)
                .kinds(&kinds)
                .resolver(&resolver)
                .text(builtin::text_doc())
                .build(),
            (width, height),
            1.0,
        )
        .unwrap_or_else(|error| panic!("the extension fixture must mount: {error}"));
        ui.scene()
            .unwrap_or_else(|error| panic!("the retained host must draw the extension: {error}"));
        ui.rect_of("page/drawn")
            .unwrap_or_else(|| panic!("the extension must be laid out"))
    }

    /// The box the immediate host measured the extension into.
    fn neutral() -> Rect {
        let (width, height) = Self::CASE;
        let ui = Self::compiled();
        let kinds = Self::kinds();
        let renderer = renderer();
        let viewport = Size::new(width.as_(), height.as_());
        let mut element = tree::render(
            &ui.root,
            &ui,
            &Page,
            builtin::skin(),
            Clock::default(),
            Some(&kinds),
        );
        let mut state = Tree::new(element.as_widget());
        let node = element.as_widget_mut().layout(
            &mut state,
            &renderer,
            &Limits::new(Size::ZERO, viewport),
        );
        let mut rows = Vec::new();
        collect_rows(Layout::new(&node), &mut rows);
        let [drawn] = rows[..] else {
            panic!(
                "the extension page holds one leaf, and the immediate host laid out {}",
                rows.len()
            )
        };
        drawn
    }
}

/// An application whose whole document is the page holding one extension.
struct Page;

impl Reads for Page {
    fn get(&self, _endpoint: &str) -> Option<ReadValue<'_>> {
        None
    }
}

impl App for Page {
    fn skin(&self) -> &Skin {
        builtin::skin()
    }

    fn document(&self) -> &str {
        "extension.klayout.ron"
    }

    fn reads<R>(&self, with: impl FnOnce(&dyn Reads) -> R) -> R {
        with(self)
    }

    fn update(&mut self, _event: UiEvent) {}
}

/// An extension that says how big it is by shaping a caption through the host's
/// own measurer, which is the one thing both hosts must answer alike.
struct Caption;

impl CustomWidget for Caption {
    type Action = ();

    fn measure(&mut self, text: &mut TextMeasurer<'_>, _limits: SizeLimits) -> Size2 {
        let shaped = text.measure(Extension::CAPTION, builtin::skin_doc().text.section, None);
        Size2::new(
            shaped.w + Extension::PAD * 2.0,
            shaped.h + Extension::PAD * 2.0,
        )
    }

    fn paint(&mut self, _list: &mut DrawListBuilder, _text: &mut TextMeasurer<'_>, _bounds: Rect) {}
}

/// An extension asks both hosts for the same box.
///
/// What it draws is the application's, but what it is measured into is the
/// host's: the size it asks for is shaped through the toolkit's own text
/// measurer, so a host that hands it a different shaper - or resolves a
/// `Shrink` axis its own way - puts the same widget in a different box, and
/// everything beside it moves.
#[kithara::test]
fn both_hosts_give_a_registered_extension_the_same_box() {
    assert_eq!(
        snapped(Extension::retained()),
        snapped(Extension::neutral())
    );
}

/// A `Shrink` axis is the extension's own answer, not the room around it.
#[kithara::test]
fn a_shrunk_extension_is_given_the_size_it_asked_for() {
    let asked = Extension::asked();
    let [_, _, w, h] = snapped(Extension::retained());

    assert_eq!([w, h], [asked.w.round(), asked.h.round()]);
}
