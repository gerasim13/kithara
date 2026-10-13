#![cfg(feature = "masonry")]

//! Embedded host contracts that do not require iced, a window or a GPU.

use std::sync::LazyLock;

use kithara_test_utils::kithara;
use kithara_ui::{
    app::{App, Config, Handled, RenderRootSignal, Ui},
    builtin,
    draw::Pt,
    ids::SourceUri,
    interact::{Input, PointerPhase, mouse},
    mock::TestRegistry,
    registry::{EndpointCategory, EndpointDesc, ValueKind},
    render::{ReadValue, Reads, Skin, UiEvent, WriteValue},
    shaping::FontPolicy,
    source::MemResolver,
};

#[derive(Default)]
struct Editor {
    active: bool,
    query: String,
}

fn skin() -> &'static Skin {
    static SKIN: LazyLock<Skin> = LazyLock::new(|| {
        Skin::resolve_with_font_policy(
            builtin::skin_doc().clone(),
            builtin::text_doc(),
            &SourceUri("fixture:embedded-host".into()),
            &builtin::resolver(),
            FontPolicy::Embedded,
        )
        .unwrap_or_else(|error| panic!("fixture skin must resolve: {error}"))
    });
    &SKIN
}

impl Reads for Editor {
    fn get(&self, endpoint: &str) -> Option<ReadValue<'_>> {
        match endpoint {
            "fixture.flag" => Some(ReadValue::Bool(self.active)),
            "fixture.query" => Some(ReadValue::Text(&self.query)),
            "fixture.tree" => Some(ReadValue::Tree(&[])),
            _ => None,
        }
    }
}

impl App for Editor {
    fn document(&self) -> &str {
        "one.klayout.ron"
    }
    fn reads<R>(&self, with: impl FnOnce(&dyn Reads) -> R) -> R {
        with(self)
    }
    fn skin(&self) -> &Skin {
        skin()
    }
    fn update(&mut self, event: UiEvent) {
        if let UiEvent::Write { key, value } = event {
            match (key.as_str(), value) {
                ("fixture.fire", WriteValue::Trigger) => self.active = true,
                ("fixture.query", WriteValue::Text(query)) => self.query = query,
                _ => {}
            }
        }
    }
}

fn documents() -> (TestRegistry, MemResolver) {
    let mut registry = TestRegistry::default();
    for (category, id, kind) in [
        (EndpointCategory::Model, "fixture.flag", ValueKind::Bool),
        (
            EndpointCategory::Command,
            "fixture.fire",
            ValueKind::Trigger,
        ),
        (EndpointCategory::Model, "fixture.tree", ValueKind::Tree),
        (EndpointCategory::Model, "fixture.query", ValueKind::Text),
        (EndpointCategory::Command, "fixture.query", ValueKind::Text),
    ] {
        registry.insert(category, id, EndpointDesc::new(kind));
    }
    let mut resolver = MemResolver::default();
    resolver.insert(
        "one.klayout.ron",
        r#"(schema: "kithara.layout", version: 1, id: "one",
        root: Module(instance: "demo", source: "one.kmodule.ron", size: (w: Fill, h: Fill)))"#,
    );
    resolver.insert(
        "one.kmodule.ron",
        r#"(schema: "kithara.module", version: 1,
        id: "gallery-knobs", chrome: Plain,
        root: Column(size: (w: Fill, h: Fill), gap: 0.0, pad: 0.0, children: [
            Chip(id: "fire", label: "PLAY", size: Some((w: Fixed(80.0), h: Fixed(40.0))),
                read: Model(id: "fixture.flag"), write: Command(id: "fixture.fire")),
            Spacer(id: "room", size: Some((w: Fill, h: Fixed(40.0)))),
            Tree(id: "tree", size: (w: Fill, h: Fill), read: Model(id: "fixture.tree"),
                query: Model(id: "fixture.query"), write_query: Command(id: "fixture.query"))
        ]))"#,
    );
    (registry, resolver)
}

fn mounted<'a>(registry: &'a TestRegistry, resolver: &'a MemResolver) -> Ui<'a, Editor> {
    let mut ui = Ui::new(
        Editor::default(),
        Config::builder()
            .endpoints(registry)
            .resolver(resolver)
            .text(builtin::text_doc())
            .build(),
        (240, 160),
        1.0,
    )
    .unwrap_or_else(|error| panic!("fixture must mount: {error}"));
    ui.frame(std::time::Duration::from_millis(16));
    ui.render()
        .unwrap_or_else(|error| panic!("fixture must lay out: {error}"));
    ui.complete_frame();
    ui
}

#[kithara::test]
fn embedded_input_reports_consumption_and_updates_synchronously() {
    let (registry, resolver) = documents();
    let mut ui = mounted(&registry, &resolver);
    let rect = ui
        .rect_of("demo/fire")
        .unwrap_or_else(|| panic!("embedded hosts need the logical action rectangle"));
    let at = Pt {
        x: rect.x + rect.w / 2.0,
        y: rect.y + rect.h / 2.0,
    };
    ui.input(Input::Pointer(mouse(PointerPhase::Move, Some(at))));
    assert_eq!(
        ui.input(Input::Pointer(mouse(PointerPhase::Down, Some(at)))),
        Handled::Yes
    );
    assert!(
        ui.app().active,
        "browser actions dispatch before the next frame"
    );
    ui.input(Input::Pointer(mouse(PointerPhase::Cancel, None)));
    let room = ui
        .rect_of("demo/room")
        .unwrap_or_else(|| panic!("room viewport must be laid out"));
    let room_point = Pt {
        x: room.x + room.w / 2.0,
        y: room.y + room.h / 2.0,
    };
    assert_eq!(
        ui.input(Input::Pointer(mouse(PointerPhase::Down, Some(room_point)))),
        Handled::No,
        "uncaptured room input remains the external host's"
    );
}

#[kithara::test]
fn embedded_host_exports_input_method_lifecycle_signals() {
    let (registry, resolver) = documents();
    let mut ui = mounted(&registry, &resolver);
    let rect = ui
        .rect_of("demo/tree")
        .unwrap_or_else(|| panic!("search must be laid out"));
    let at = Pt {
        x: rect.x + skin().tree.search_icon_width + 10.0,
        y: rect.y + skin().tree.search_height / 2.0,
    };
    for phase in [PointerPhase::Move, PointerPhase::Down, PointerPhase::Up] {
        ui.input(Input::Pointer(mouse(phase, Some(at))));
    }
    assert!(ui.editing_text(), "search must own keyboard focus");
    let signals = ui.take_platform_signals();
    assert!(
        signals
            .iter()
            .any(|signal| matches!(signal, RenderRootSignal::StartIme))
    );
    assert!(
        ui.take_platform_signals().is_empty(),
        "host signals drain once"
    );
}
