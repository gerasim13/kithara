use iced::{
    Size,
    advanced::{
        layout::{Layout, Limits},
        widget::Tree,
    },
};
use kithara_test_utils::kithara;
use num_traits::cast::AsPrimitive;

use super::shared::{Endpoints, collect_rows, renderer};
use crate::{
    app::{App, Config, Ui},
    builtin,
    compile::compile,
    draw::{Pt, Rect},
    interact::{Input, MOUSE, PointerInput, PointerPhase},
    render::{Clock, ControlAction, ReadValue, Reads, Skin, UiEvent, tree},
    source::{MemResolver, UiConfig},
    view,
};

/// The shape of the document below, and the window both hosts are given.
struct Consts;

impl Consts {
    /// The room the window leaves, wide and tall enough for every block.
    const CASE: (u32, u32) = (300, 240);
    /// Every leaf the document lays out, in the order it holds them: a row of
    /// the flow, the block that flow hides, the row after it, and the cell the
    /// split hides beside them.
    const LEAVES: [&'static str; 4] = ["head", "body", "tail", "aside"];
}

/// A document that hides a child of a flow and a cell of a split behind the
/// same flag, the way the shipped menu hides a group and the shipped layout
/// hides a module.
fn documents() -> MemResolver {
    let mut resolver = MemResolver::default();
    resolver.insert(
        "blocks.klayout.ron",
        r#"(schema: "kithara.layout", version: 1, id: "blocks",
            root: Split(axis: Horizontal, size: (w: Fill, h: Fill), children: [
                (weight: 1.0, node: Module(instance: "flow", source: "flow.kmodule.ron",
                    size: (w: Fill, h: Fill))),
                (node: Optional(id: "aside-block", hidden: Model(id: "fixture.hidden"),
                    node: Module(instance: "aside", source: "aside.kmodule.ron",
                        size: (w: Fixed(80.0), h: Fill)))),
            ]))"#,
    );
    resolver.insert(
        "flow.kmodule.ron",
        r#"(schema: "kithara.module", version: 1, id: "flow", chrome: Plain,
            root: Column(size: (w: Fill, h: Fill), gap: 0.0, pad: 0.0, children: [
                Pressable(id: "open", press: Command(id: "fixture.toggle"),
                    child: Spacer(id: "head", size: Some((w: Fill, h: Fixed(26.0))))),
                Optional(id: "body-block", hidden: Model(id: "fixture.hidden"),
                    child: Spacer(id: "body", size: Some((w: Fill, h: Fixed(40.0))))),
                Spacer(id: "tail", size: Some((w: Fill, h: Fixed(26.0)))),
            ]))"#,
    );
    resolver.insert(
        "aside.kmodule.ron",
        r#"(schema: "kithara.module", version: 1, id: "aside", chrome: Plain,
            root: Row(size: (w: Fill, h: Fill), gap: 0.0, pad: 0.0, children: [
                Spacer(id: "aside", size: Some((w: Fill, h: Fill))),
            ]))"#,
    );
    resolver
}

/// An application that hides every block until something presses the row that
/// shows them.
#[derive(Default)]
struct Blocks {
    shown: bool,
}

impl Reads for Blocks {
    fn get(&self, endpoint: &str) -> Option<ReadValue<'_>> {
        let id = endpoint.split_once('@').map_or(endpoint, |(id, _)| id);
        (id == "fixture.hidden").then_some(ReadValue::Bool(!self.shown))
    }
}

impl App for Blocks {
    fn skin(&self) -> &Skin {
        builtin::skin()
    }

    fn document(&self) -> &str {
        "blocks.klayout.ron"
    }

    fn reads<R>(&self, with: impl FnOnce(&dyn Reads) -> R) -> R {
        with(self)
    }

    fn update(&mut self, event: UiEvent) {
        if let UiEvent::Control { action, .. } = event
            && action == ControlAction::Activate
        {
            self.shown = !self.shown;
        }
    }
}

/// The boxes the retained host laid the document's leaves into once a press
/// has shown the blocks.
///
/// The blocks are shown by pressing the row the document names for it, so the
/// boxes below are the ones a person sees after using the document rather than
/// a state the test declared.
fn retained() -> Vec<Rect> {
    let endpoints = Endpoints::default();
    let resolver = documents();
    let (width, height) = Consts::CASE;
    let mut ui = Ui::new(
        Blocks::default(),
        Config::builder()
            .endpoints(&endpoints)
            .resolver(&resolver)
            .text(builtin::text_doc())
            .build(),
        (width, height),
        1.0,
    )
    .unwrap_or_else(|error| panic!("the block fixture must mount: {error}"));
    let head = ui
        .rect_of("flow/head")
        .unwrap_or_else(|| panic!("the row that shows the blocks must be laid out"));
    let at = Pt {
        x: head.x + head.w / 2.0,
        y: head.y + head.h / 2.0,
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
        ui.app().shown,
        "the press on the row must show the blocks the document hides"
    );
    ui.scene()
        .unwrap_or_else(|error| panic!("the retained host must draw the shown blocks: {error}"));
    Consts::LEAVES
        .iter()
        .filter_map(|leaf| ui.rect_of(&format!("{}/{leaf}", instance(leaf))))
        .filter(|rect| rect.w > 0.0 && rect.h > 0.0)
        .collect()
}

/// The boxes the immediate host laid the same leaves into, reading the flag the
/// press on the other host set.
///
/// This host has no pointer of its own here: it builds its whole tree from that
/// reading, which is exactly what makes a block it never mounts invisible.
fn neutral() -> Vec<Rect> {
    let ui = compile(
        "blocks.klayout.ron",
        &documents(),
        &Endpoints::default(),
        builtin::skin_doc(),
        builtin::text_doc(),
        &UiConfig::default(),
        &view::EMPTY,
    )
    .unwrap_or_else(|error| panic!("the block fixture must compile: {error}"));
    let (width, height) = Consts::CASE;
    let renderer = renderer();
    let viewport = Size::new(width.as_(), height.as_());
    let mut element = tree::render(
        &ui.root,
        &ui,
        &Blocks { shown: true },
        &view::EMPTY,
        builtin::skin(),
        Clock::default(),
        None,
    );
    let mut state = Tree::new(element.as_widget());
    let node =
        element
            .as_widget_mut()
            .layout(&mut state, &renderer, &Limits::new(Size::ZERO, viewport));
    let mut rows = Vec::new();
    collect_rows(Layout::new(&node), &mut rows);
    rows
}

/// Which module holds one leaf of the document.
fn instance(leaf: &str) -> &'static str {
    if leaf == "aside" { "aside" } else { "flow" }
}

/// A block shown by a press stands in the same box on both hosts.
///
/// The host that throws its tree away every frame builds the block the moment
/// the document stops hiding it. The host that keeps its tree has to mount the
/// block while it is hidden and show it in place, because a block missing from
/// a mounted tree could never come back.
#[kithara::test]
fn both_hosts_lay_a_shown_block_out_the_same_way() {
    assert_eq!(
        retained(),
        neutral(),
        "the two hosts disagree on where the blocks a press showed stand"
    );
}
