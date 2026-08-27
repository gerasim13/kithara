//! What the two hosts owe each other about the boxes a document is laid out
//! into, on the documents the committed rect corpus cannot reach.
//!
//! The corpus compares the documents this crate ships, standing still. Neither
//! of the two below is in it: a menu has no box at all until a press opens it,
//! and no shipped document puts a run in a box too small for it. So each one is
//! mounted on both hosts, used the way a person would use it, and the boxes the
//! two placed are compared.

use iced::{
    Point as IcedPoint, Rectangle, Size, Vector,
    advanced::{
        layout::{Layout, Limits},
        widget::Tree,
    },
};
use iced_renderer::fallback::Renderer as FallbackRenderer;
use iced_tiny_skia::Renderer as TinySkiaRenderer;
use kithara_test_utils::kithara;
use num_traits::cast::AsPrimitive;

use crate::{
    app::{App, Config, Ui},
    builtin,
    compile::{CompiledUi, compile},
    draw::{Pt, Rect},
    ids::EndpointId,
    interact::{Input, MOUSE, PointerInput, PointerPhase},
    registry::{EndpointCategory, EndpointDesc, EndpointRegistry, ValueKind},
    render::{Clock, ControlAction, ReadValue, Reads, UiEvent, tree},
    source::{MemResolver, UiConfig},
};

/// The shape of the document below: the bars it hangs a menu on, the rows each
/// menu is made of, and the windows the two hosts are compared at.
struct Fixture;

impl Fixture {
    /// Every bar the document hangs a menu on.
    const BARS: [&'static str; 2] = ["narrow", "wide"];
    /// A window short enough that only the narrow bar's band is reached and one
    /// tall enough that only the wide bar's is, each carrying the bar the room
    /// reaches there.
    const CASES: [(u32, u32, &'static str); 2] = [(400, 240, "narrow"), (400, 480, "wide")];
    /// The rows the menu is made of, in the order the document holds them.
    const ROWS: [&'static str; 2] = ["head", "item"];
}

/// A document that hangs one menu on two bars and reaches whichever the window
/// is tall enough for, the way the shipped app bar carries a wide strip and a
/// narrow one.
fn documents() -> MemResolver {
    let mut resolver = MemResolver::default();
    resolver.insert(
        "bars.klayout.ron",
        r#"(schema: "kithara.layout", version: 1, id: "bars",
            root: Split(axis: Vertical, measure: Height, size: (w: Fill, h: Fill), children: [
                (until: Some(300.0), node: Module(instance: "narrow", source: "bar.kmodule.ron",
                    size: (w: Fill, h: Fixed(42.0)))),
                (from: 300.0, node: Module(instance: "wide", source: "bar.kmodule.ron",
                    size: (w: Fill, h: Fixed(42.0)))),
                (weight: 1.0, node: Module(instance: "body", source: "body.kmodule.ron",
                    size: (w: Fill, h: Fill))),
            ]))"#,
    );
    resolver.insert(
        "bar.kmodule.ron",
        r#"(schema: "kithara.module", version: 1, id: "bar", chrome: Plain,
            root: Row(size: (w: Fill, h: Fill), gap: 0.0, pad: 0.0, children: [
                Popover(id: "pop", open: Model(id: "fixture.menu"), align: Start,
                    anchor: Pressable(id: "burger", press: Command(id: "fixture.toggle"),
                        child: Spacer(id: "anchor", size: Some((w: Fixed(40.0), h: Fixed(42.0))))),
                    content: Column(id: "surface", size: (w: Fixed(180.0), h: Shrink), gap: 0.0,
                        children: [
                            Spacer(id: "head", size: Some((w: Fill, h: Fixed(26.0)))),
                            Spacer(id: "item", size: Some((w: Fill, h: Fixed(24.0)))),
                        ])),
            ]))"#,
    );
    resolver.insert(
        "body.kmodule.ron",
        r#"(schema: "kithara.module", version: 1, id: "body", chrome: Plain,
            root: Row(size: (w: Fill, h: Fill), gap: 0.0, pad: 0.0, children: []))"#,
    );
    resolver
}

/// An application whose one flag every bar's menu reads, so a press on the
/// burger that stands opens as many menus as the document holds.
#[derive(Default)]
struct Bars {
    open: bool,
}

impl Reads for Bars {
    fn get(&self, endpoint: &str) -> Option<ReadValue<'_>> {
        let id = endpoint.split_once('@').map_or(endpoint, |(id, _)| id);
        (id == "fixture.menu").then_some(ReadValue::Bool(self.open))
    }
}

impl App for Bars {
    fn document(&self) -> &str {
        "bars.klayout.ron"
    }

    fn reads<R>(&self, with: impl FnOnce(&dyn Reads) -> R) -> R {
        with(self)
    }

    fn update(&mut self, event: UiEvent) {
        if let UiEvent::Control { action, .. } = event
            && action == ControlAction::Activate
        {
            self.open = !self.open;
        }
    }
}

struct Endpoints {
    open: EndpointDesc,
    press: EndpointDesc,
}

impl Default for Endpoints {
    fn default() -> Self {
        Self {
            open: EndpointDesc::new(ValueKind::Bool),
            press: EndpointDesc::new(ValueKind::Trigger),
        }
    }
}

impl EndpointRegistry for Endpoints {
    fn endpoint(&self, category: EndpointCategory, id: &EndpointId) -> Option<&EndpointDesc> {
        match (category, id.0.as_str()) {
            (EndpointCategory::Model, "fixture.menu") => Some(&self.open),
            (EndpointCategory::Command, "fixture.toggle") => Some(&self.press),
            _ => None,
        }
    }
}

fn compiled() -> CompiledUi {
    compile(
        "bars.klayout.ron",
        &documents(),
        &Endpoints::default(),
        builtin::skin_doc(),
        builtin::text_doc(),
        &UiConfig::default(),
    )
    .unwrap_or_else(|error| panic!("the bar fixture must compile: {error}"))
}

fn renderer() -> iced::Renderer {
    FallbackRenderer::Secondary(TinySkiaRenderer::new(
        crate::render::fonts::SANS,
        iced::Pixels(14.0),
    ))
}

/// Every menu the retained host has standing, each one holding the boxes its
/// rows were laid out into.
///
/// The menu is opened by pressing the burger the room reached, so the boxes
/// below are the boxes a person sees after using the bar rather than a state
/// the test declared.
fn retained_menus(standing: &str, width: u32, height: u32) -> Vec<Vec<Rect>> {
    let endpoints = Endpoints::default();
    let resolver = documents();
    let skin = builtin::skin().clone();
    let mut ui = Ui::new(
        Bars::default(),
        Config::builder()
            .endpoints(&endpoints)
            .resolver(&resolver)
            .skin(&skin)
            .skin_doc(builtin::skin_doc())
            .text(builtin::text_doc())
            .build(),
        (width, height),
        1.0,
    )
    .unwrap_or_else(|error| panic!("the bar fixture must mount: {error}"));
    let anchor = ui
        .rect_of(&format!("{standing}/anchor"))
        .unwrap_or_else(|| panic!("the bar the room reached must be laid out"));
    let at = Pt {
        x: anchor.x + anchor.w / 2.0,
        y: anchor.y + anchor.h / 2.0,
    };
    for phase in [PointerPhase::Move, PointerPhase::Down, PointerPhase::Up] {
        ui.input(Input::Pointer(PointerInput::new(
            MOUSE,
            None,
            phase,
            Some(at),
            1,
        )));
    }
    assert!(
        ui.app().open,
        "the press on the burger that stands must open the menu"
    );
    ui.scene()
        .unwrap_or_else(|error| panic!("the retained host must draw the open menu: {error}"));
    Fixture::BARS
        .iter()
        .map(|bar| {
            Fixture::ROWS
                .iter()
                .filter_map(|row| ui.rect_of(&format!("{bar}/{row}")))
                .filter(|rect| rect.w > 0.0 && rect.h > 0.0)
                .collect::<Vec<_>>()
        })
        .filter(|menu| !menu.is_empty())
        .collect()
}

/// Every menu the immediate host has standing, read from the overlay it hands
/// its renderer, each one holding the boxes its rows were laid out into.
///
/// This host has no pointer of its own here: it reads the same flag the press
/// on the other host set, and builds its whole tree from that reading.
fn neutral_menus(width: u32, height: u32) -> Vec<Vec<Rect>> {
    let ui = compiled();
    let reads = Bars { open: true };
    let renderer = renderer();
    let viewport = Size::new(width.as_(), height.as_());
    let mut element = tree::render(
        &ui.root,
        &ui,
        &reads,
        builtin::skin(),
        Clock::default(),
        None,
    );
    let mut state = Tree::new(element.as_widget());
    let node =
        element
            .as_widget_mut()
            .layout(&mut state, &renderer, &Limits::new(Size::ZERO, viewport));
    let bounds = Rectangle::new(IcedPoint::ORIGIN, viewport);
    let Some(mut overlay) = element.as_widget_mut().overlay(
        &mut state,
        Layout::new(&node),
        &renderer,
        &bounds,
        Vector::ZERO,
    ) else {
        return Vec::new();
    };
    let node = overlay.as_overlay_mut().layout(&renderer, viewport);
    let mut menus = Vec::new();
    collect_menus(Layout::new(&node), viewport, &mut menus);
    menus
}

/// Gathers each surface the overlay put on top of the document.
///
/// The overlay a document hands over is a nest of groups, one for every level
/// that forwarded it, and each of those takes the whole window. A surface is
/// the first node under them that asked for a box of its own.
fn collect_menus(layout: Layout<'_>, viewport: Size, menus: &mut Vec<Vec<Rect>>) {
    for child in layout.children() {
        if fills(child.bounds(), viewport) {
            collect_menus(child, viewport, menus);
            continue;
        }
        let mut rows = Vec::new();
        collect_rows(child, &mut rows);
        menus.push(rows);
    }
}

/// The boxes the leaves of one surface were laid out into, in document order.
fn collect_rows(layout: Layout<'_>, rows: &mut Vec<Rect>) {
    let mut children = layout.children().peekable();
    if children.peek().is_none() {
        let bounds = layout.bounds();
        rows.push(Rect {
            x: bounds.x,
            y: bounds.y,
            w: bounds.width,
            h: bounds.height,
        });
        return;
    }
    for child in children {
        collect_rows(child, rows);
    }
}

fn fills(bounds: Rectangle, viewport: Size) -> bool {
    bounds.width == viewport.width && bounds.height == viewport.height
}

/// A menu that stands on one host has to stand on the other.
///
/// One document can hang the same menu on two bars and reach whichever the
/// window is tall enough for. The host that throws its tree away every frame
/// only ever builds the bar it reached; the host that keeps its tree mounts
/// both and stands one aside, and a surface hanging on the bar that stands
/// aside would open on the one flag they share.
#[kithara::test]
fn both_hosts_stand_the_same_number_of_menus() {
    for (width, height, standing) in Fixture::CASES {
        assert_eq!(
            retained_menus(standing, width, height).len(),
            neutral_menus(width, height).len(),
            "at {width}x{height} the two hosts disagree on how many menus one press opens"
        );
    }
}

/// An opened menu lands in the same boxes on both hosts.
#[kithara::test]
fn both_hosts_lay_an_opened_menu_out_the_same_way() {
    for (width, height, standing) in Fixture::CASES {
        assert_eq!(
            retained_menus(standing, width, height),
            neutral_menus(width, height),
            "at {width}x{height} the menu opened from the `{standing}` bar was laid out \
             differently by the two hosts"
        );
    }
}

/// A strip carrying one run wider than the room the window leaves it, so each
/// host has to say what a squeezed run asks its parent for.
struct Squeeze;

impl Squeeze {
    /// The room down the window, which the run never competes for.
    const HEIGHT: u32 = 60;
    /// A window narrower across than the run wants to be, one about as wide,
    /// and one with room to spare.
    const WIDTHS: [u32; 3] = [40, 60, 200];

    /// A document whose whole content is one run of words in a strip across the
    /// window, so the only thing either host has to decide is how wide that run
    /// is.
    fn document() -> MemResolver {
        let mut resolver = MemResolver::default();
        resolver.insert(
            "strip.klayout.ron",
            r#"(schema: "kithara.layout", version: 1, id: "strip",
                root: Split(axis: Vertical, measure: Height, size: (w: Fill, h: Fill), children: [
                    (weight: 1.0, node: Module(instance: "strip", source: "strip.kmodule.ron",
                        size: (w: Fill, h: Fill))),
                ]))"#,
        );
        resolver.insert(
            "strip.kmodule.ron",
            r#"(schema: "kithara.module", version: 1, id: "strip", chrome: Plain,
                root: Row(size: (w: Fill, h: Fill), gap: 0.0, pad: 0.0, children: [
                    Text(id: "label", style: MicroLabel, label: "1 / WINDOW"),
                ]))"#,
        );
        resolver
    }

    /// The box the retained host laid the strip's run into.
    fn retained(width: u32) -> Rect {
        let endpoints = Endpoints::default();
        let resolver = Self::document();
        let skin = builtin::skin().clone();
        let mut ui = Ui::new(
            Strip,
            Config::builder()
                .endpoints(&endpoints)
                .resolver(&resolver)
                .skin(&skin)
                .skin_doc(builtin::skin_doc())
                .text(builtin::text_doc())
                .build(),
            (width, Self::HEIGHT),
            1.0,
        )
        .unwrap_or_else(|error| panic!("the strip fixture must mount: {error}"));
        ui.scene()
            .unwrap_or_else(|error| panic!("the retained host must draw the strip: {error}"));
        ui.rect_of("strip/label")
            .unwrap_or_else(|| panic!("the run must be laid out at {width} across"))
    }

    /// The box the immediate host laid the strip's run into.
    fn neutral(width: u32) -> Rect {
        let ui = compile(
            "strip.klayout.ron",
            &Self::document(),
            &Endpoints::default(),
            builtin::skin_doc(),
            builtin::text_doc(),
            &UiConfig::default(),
        )
        .unwrap_or_else(|error| panic!("the strip fixture must compile: {error}"));
        let renderer = renderer();
        let viewport = Size::new(width.as_(), Self::HEIGHT.as_());
        let mut element = tree::render(
            &ui.root,
            &ui,
            &Strip,
            builtin::skin(),
            Clock::default(),
            None,
        );
        let mut state = Tree::new(element.as_widget());
        let node = element.as_widget_mut().layout(
            &mut state,
            &renderer,
            &Limits::new(Size::ZERO, viewport),
        );
        let mut rows = Vec::new();
        collect_rows(Layout::new(&node), &mut rows);
        let [run] = rows[..] else {
            panic!(
                "the strip holds one run, and the immediate host laid out {}",
                rows.len()
            )
        };
        run
    }
}

/// An application that reads nothing, because the strip asks nothing of it.
struct Strip;

impl Reads for Strip {
    fn get(&self, _endpoint: &str) -> Option<ReadValue<'_>> {
        None
    }
}

impl App for Strip {
    fn document(&self) -> &str {
        "strip.klayout.ron"
    }

    fn reads<R>(&self, with: impl FnOnce(&dyn Reads) -> R) -> R {
        with(self)
    }

    fn update(&mut self, _event: UiEvent) {}
}

/// How the rect corpus compares the two hosts: one lays out in whole pixels and
/// the other in fractions of one, so both edges are snapped before they meet.
fn snapped(rect: Rect) -> [f32; 4] {
    let x = rect.x.round();
    let y = rect.y.round();
    [
        x,
        y,
        (rect.x + rect.w).round() - x,
        (rect.y + rect.h).round() - y,
    ]
}

/// A run asks both hosts for the same box, whatever room it is offered.
///
/// A run says how wide it wants to be before it can know how much room there
/// is. Shaped against the room instead, it asks for the width its broken lines
/// happen to need, which is narrower than the room it was already offered: the
/// same words then land on a different number of lines on the two hosts, and
/// everything beside them moves.
#[kithara::test]
fn both_hosts_give_a_squeezed_run_the_same_box() {
    for width in Squeeze::WIDTHS {
        assert_eq!(
            snapped(Squeeze::retained(width)),
            snapped(Squeeze::neutral(width)),
            "at {width} across the two hosts disagree on the box one run of words asks for"
        );
    }
}
