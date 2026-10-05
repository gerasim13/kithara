use kithara_test_utils::kithara;

use crate::{
    builtin,
    compile::{CompiledUi, compile},
    draw::Pt,
    error::UiDocError,
    mock::TestRegistry,
    module::ViewSet,
    registry::{EndpointCategory, EndpointDesc, ValueKind},
    render::{ControlAction, Published, ReadValue, Reads, ScalarRange, UiEvent, WriteValue},
    source::{MemResolver, UiConfig},
    view::{self, ViewState},
};

const DOCUMENT: &str = "slots.klayout.ron";

struct Interval;

impl Reads for Interval {
    fn get(&self, endpoint: &str) -> Option<ReadValue<'_>> {
        (endpoint == "fixture.span@deck=b").then_some(ReadValue::Range(ScalarRange {
            min: 0.25,
            max: 0.75,
        }))
    }
}

fn registry() -> TestRegistry {
    let mut registry = TestRegistry::default();
    for (category, id, kind) in [
        (
            EndpointCategory::Command,
            "fixture.fire",
            ValueKind::Trigger,
        ),
        (
            EndpointCategory::Command,
            "fixture.menu",
            ValueKind::Trigger,
        ),
        (
            EndpointCategory::Command,
            "fixture.reset",
            ValueKind::Trigger,
        ),
        (
            EndpointCategory::Command,
            "fixture.fold",
            ValueKind::Trigger,
        ),
        (
            EndpointCategory::Command,
            "fixture.settings",
            ValueKind::Trigger,
        ),
        (
            EndpointCategory::Parameter,
            "fixture.rate",
            ValueKind::Scalar,
        ),
        (
            EndpointCategory::Parameter,
            "fixture.seek",
            ValueKind::Scalar,
        ),
        (
            EndpointCategory::Parameter,
            "fixture.loop_start",
            ValueKind::Scalar,
        ),
        (
            EndpointCategory::Parameter,
            "fixture.loop_end",
            ValueKind::Scalar,
        ),
        (
            EndpointCategory::Parameter,
            "fixture.span",
            ValueKind::Range,
        ),
        (
            EndpointCategory::Parameter,
            "fixture.preset",
            ValueKind::Text,
        ),
        (EndpointCategory::Parameter, "fixture.at", ValueKind::Point),
        (
            EndpointCategory::Telemetry,
            "fixture.wave",
            ValueKind::Waveform,
        ),
        (EndpointCategory::Model, "fixture.at", ValueKind::Point),
        (EndpointCategory::Model, "fixture.zoom", ValueKind::Scalar),
        (EndpointCategory::Model, "fixture.span", ValueKind::Range),
        (EndpointCategory::Model, "fixture.tree", ValueKind::Tree),
        (EndpointCategory::Model, "fixture.query", ValueKind::Text),
        (EndpointCategory::Model, "fixture.open", ValueKind::Bool),
    ] {
        registry.insert(category, id, EndpointDesc::new(kind).with_scope("deck"));
    }
    registry.insert(
        EndpointCategory::Model,
        "fixture.rows",
        EndpointDesc::new(ValueKind::Table).with_scope("deck"),
    );
    registry.insert(
        EndpointCategory::Parameter,
        "fixture.width",
        EndpointDesc::new(ValueKind::Scalar)
            .with_scope("deck")
            .with_scope("column"),
    );
    registry
}

fn compiled(controls: &str) -> Result<CompiledUi, UiDocError> {
    let mut resolver = MemResolver::default();
    resolver.insert(
        DOCUMENT,
        r#"(schema: "kithara.layout", version: 1, id: "slots",
            root: Module(instance: "deck-b", source: "slots.kmodule.ron", with: { "deck": "b" },
                size: (w: Fill, h: Fill)))"#,
    );
    resolver.insert(
        "slots.kmodule.ron",
        &format!(
            r#"(schema: "kithara.module", version: 1, id: "slots", chrome: Plain,
                parameters: ["deck"],
                collapse: Some(Command(id: "fixture.fold", with: {{ "deck": "$deck" }})),
                root: Column(size: (w: Fill, h: Fill), gap: 0.0, pad: 0.0, children: [{controls}]))"#
        ),
    );
    compile(
        DOCUMENT,
        &resolver,
        &registry(),
        builtin::skin_doc(),
        builtin::text_doc(),
        &UiConfig::default(),
        &view::EMPTY,
    )
}

fn delivers(controls: &str, path: &str, action: &ControlAction) -> Option<UiEvent> {
    let ui = compiled(controls).unwrap_or_else(|error| panic!("the slots must compile: {error}"));
    settle(&ui, path, action.clone(), &mut ViewState::new())
}

fn settle(
    ui: &CompiledUi,
    path: &str,
    action: ControlAction,
    view: &mut ViewState,
) -> Option<UiEvent> {
    let published = Published::Gesture {
        action,
        path: path.to_owned(),
    };
    ui.views().settle(published, &Interval, view)
}

fn write(key: &str, value: WriteValue) -> UiEvent {
    UiEvent::Write {
        key: key.to_owned(),
        value,
    }
}

const PRESSABLE: &str = r#"Pressable(id: "anchor",
    press: Command(id: "fixture.fire", with: { "deck": "$deck" }),
    secondary: Command(id: "fixture.menu", with: { "deck": "$deck" }),
    child: Spacer(id: "face", size: Some((w: Fixed(20.0), h: Fixed(20.0)))))"#;

#[kithara::test]
fn a_secondary_press_delivers_its_own_slot_and_a_primary_press_the_other() {
    assert_eq!(
        delivers(
            PRESSABLE,
            "deck-b/anchor",
            &ControlAction::SecondaryActivate
        ),
        Some(write("fixture.menu@deck=b", WriteValue::Trigger))
    );
    assert_eq!(
        delivers(PRESSABLE, "deck-b/anchor", &ControlAction::Activate),
        Some(write("fixture.fire@deck=b", WriteValue::Trigger))
    );
}

#[kithara::test]
fn a_stepper_resets_through_its_reset_slot() {
    const TEMPO: &str = r#"Row(id: "tempo", size: (w: Fixed(80.0), h: Fixed(20.0)),
        write: Parameter(id: "fixture.rate", with: { "deck": "$deck" }),
        reset: Command(id: "fixture.reset", with: { "deck": "$deck" }),
        children: [Text(id: "label", style: MicroLabel, label: "TEMPO")])"#;

    assert_eq!(
        delivers(TEMPO, "deck-b/tempo", &ControlAction::Activate),
        Some(write("fixture.reset@deck=b", WriteValue::Trigger))
    );
    assert_eq!(
        delivers(TEMPO, "deck-b/tempo", &ControlAction::StepScalar(-1.0)),
        Some(write("fixture.rate@deck=b", WriteValue::Step(-1.0)))
    );
}

#[kithara::test]
fn a_wave_delivers_seek_zoom_and_loop_edges_each_to_its_slot() {
    const WAVE: &str = r#"Wave(id: "wave", size: (w: Fixed(200.0), h: Fixed(40.0)),
        read: Telemetry(id: "fixture.wave", with: { "deck": "$deck" }),
        write: Parameter(id: "fixture.seek", with: { "deck": "$deck" }),
        zoom: Model(id: "fixture.zoom", with: { "deck": "$deck" }),
        write_zoom: Model(id: "fixture.zoom", with: { "deck": "$deck" }),
        write_loop_start: Parameter(id: "fixture.loop_start", with: { "deck": "$deck" }),
        write_loop_end: Parameter(id: "fixture.loop_end", with: { "deck": "$deck" }))"#;

    for (path, key) in [
        ("deck-b/wave", "fixture.seek@deck=b"),
        ("deck-b/wave/zoom", "fixture.zoom@deck=b"),
        ("deck-b/wave/loop_start", "fixture.loop_start@deck=b"),
        ("deck-b/wave/loop_end", "fixture.loop_end@deck=b"),
    ] {
        assert_eq!(
            delivers(WAVE, path, &ControlAction::SetScalar(0.5)),
            Some(write(key, WriteValue::Scalar(0.5))),
            "{path}"
        );
    }
}

const TABLE: &str = r#"Table(id: "tracks", size: (w: Fixed(300.0), h: Fixed(120.0)),
    read: Model(id: "fixture.rows", with: { "deck": "$deck" }),
    write_width: Parameter(id: "fixture.width", with: { "deck": "$deck" }),
    columns: [
        (id: "title", label: "TITLE", style: Primary, width: 180.0),
        (id: "artist", label: "ARTIST", style: Secondary, width: 120.0),
    ])"#;

#[kithara::test]
fn a_table_delivers_each_column_width_scoped_by_its_column() {
    for column in ["title", "artist"] {
        assert_eq!(
            delivers(
                TABLE,
                &format!("deck-b/tracks/width/{column}"),
                &ControlAction::SetScalar(0.4)
            ),
            Some(write(
                &format!("fixture.width@column={column},deck=b"),
                WriteValue::Scalar(0.4)
            )),
            "{column}"
        );
    }
}

#[kithara::test]
fn a_width_slot_bound_to_a_trigger_fails_to_compile_naming_the_table() {
    let table = TABLE.replace(
        r#"write_width: Parameter(id: "fixture.width", with: { "deck": "$deck" })"#,
        r#"write_width: Command(id: "fixture.fire", with: { "deck": "$deck" })"#,
    );

    let error = compiled(&table).expect_err("a width written as a trigger must be refused");

    assert!(
        matches!(&error, UiDocError::BindingType { path, .. } if path.ends_with("tracks")),
        "the refusal must name the table, not {error}"
    );
}

#[kithara::test]
fn a_wave_without_a_loop_slot_delivers_nothing_for_a_loop_edge() {
    const WAVE: &str = r#"Wave(id: "wave", size: (w: Fixed(200.0), h: Fixed(40.0)),
        read: Telemetry(id: "fixture.wave", with: { "deck": "$deck" }))"#;

    assert_eq!(
        delivers(
            WAVE,
            "deck-b/wave/loop_start",
            &ControlAction::SetScalar(0.5)
        ),
        None
    );
}

#[kithara::test]
fn slots_that_only_read_deliver_nothing() {
    const READS: &str = r#"Column(size: (w: Fill, h: Fill), gap: 0.0, pad: 0.0, children: [
        Wave(id: "wave", size: (w: Fixed(200.0), h: Fixed(40.0)),
            read: Telemetry(id: "fixture.wave", with: { "deck": "$deck" }),
            zoom: Model(id: "fixture.zoom", with: { "deck": "$deck" })),
        Tree(id: "browser", size: (w: Fixed(200.0), h: Fixed(200.0)),
            read: Model(id: "fixture.tree", with: { "deck": "$deck" }),
            query: Model(id: "fixture.query", with: { "deck": "$deck" })),
    ])"#;

    assert_eq!(
        delivers(READS, "deck-b/wave/zoom", &ControlAction::SetScalar(0.5)),
        None
    );
    assert_eq!(
        delivers(
            READS,
            "deck-b/browser/search",
            &ControlAction::Text("loc".to_owned())
        ),
        None
    );
}

#[kithara::test]
fn a_text_input_delivers_what_was_typed_to_its_query() {
    const TREE: &str = r#"Tree(id: "browser", size: (w: Fixed(200.0), h: Fixed(200.0)),
        read: Model(id: "fixture.tree", with: { "deck": "$deck" }),
        query: Model(id: "fixture.query", with: { "deck": "$deck" }),
        write_query: Model(id: "fixture.query", with: { "deck": "$deck" }))"#;

    assert_eq!(
        delivers(
            TREE,
            "deck-b/browser/search",
            &ControlAction::Text("loc".to_owned())
        ),
        Some(write(
            "fixture.query@deck=b",
            WriteValue::Text("loc".to_owned())
        ))
    );
}

#[kithara::test]
fn pressing_a_module_header_delivers_its_collapse() {
    assert_eq!(
        delivers(PRESSABLE, "deck-b/header", &ControlAction::Activate),
        Some(write("fixture.fold@deck=b", WriteValue::Trigger))
    );
}

#[kithara::test]
fn a_preset_selector_delivers_the_preset_it_picked() {
    const PRESETS: &str = r#"PresetSelector(id: "presets",
        write: Parameter(id: "fixture.preset", with: { "deck": "$deck" }))"#;

    assert_eq!(
        delivers(
            PRESETS,
            "deck-b/presets",
            &ControlAction::Text("player.klayout.ron".to_owned())
        ),
        Some(write(
            "fixture.preset@deck=b",
            WriteValue::Text("player.klayout.ron".to_owned())
        ))
    );
}

#[kithara::test]
fn a_settings_button_delivers_its_trigger() {
    const SETTINGS: &str = r#"SettingsButton(id: "settings",
        size: Some((w: Fixed(40.0), h: Fixed(40.0))),
        write: Command(id: "fixture.settings", with: { "deck": "$deck" }))"#;

    assert_eq!(
        delivers(SETTINGS, "deck-b/settings", &ControlAction::Activate),
        Some(write("fixture.settings@deck=b", WriteValue::Trigger))
    );
}

#[kithara::test]
fn moving_one_end_of_a_range_delivers_the_whole_interval() {
    const RANGE: &str = r#"Range(id: "span", size: (w: Fixed(200.0), h: Fixed(16.0)),
        read: Model(id: "fixture.span", with: { "deck": "$deck" }),
        write: Parameter(id: "fixture.span", with: { "deck": "$deck" }))"#;

    assert_eq!(
        delivers(RANGE, "deck-b/span/min", &ControlAction::SetScalar(0.125)),
        Some(write("fixture.span@deck=b", WriteValue::Range(0.125, 0.75)))
    );
    assert_eq!(
        delivers(RANGE, "deck-b/span/max", &ControlAction::SetScalar(0.875)),
        Some(write("fixture.span@deck=b", WriteValue::Range(0.25, 0.875)))
    );
}

#[kithara::test]
fn a_placement_delivers_the_point_it_came_to_rest_at() {
    const PLACED: &str = r#"Stage(id: "stage", size: Some((w: Fixed(200.0), h: Fixed(100.0))), children: [
        Placed(id: "puck", at: (10.0, 10.0),
            read: Model(id: "fixture.at", with: { "deck": "$deck" }),
            write: Parameter(id: "fixture.at", with: { "deck": "$deck" }),
            child: Spacer(id: "dot", size: Some((w: Fixed(10.0), h: Fixed(10.0))))),
    ])"#;
    let at = Pt { x: 40.0, y: 30.0 };

    assert_eq!(
        delivers(PLACED, "deck-b/puck", &ControlAction::Place(at)),
        Some(write("fixture.at@deck=b", WriteValue::Point(at)))
    );
}

/// A popover on a view flag, declaring `dismiss` the way the caller spells it.
fn menu(dismiss: &str) -> String {
    format!(
        r#"Popover(id: "menu", open: View(id: "menu"), {dismiss}
    anchor: Pressable(id: "burger", press: View(id: "menu"),
        child: Spacer(id: "icon", size: Some((w: Fixed(20.0), h: Fixed(20.0))))),
    content: Column(size: (w: Fixed(100.0), h: Fixed(60.0)), gap: 0.0, pad: 0.0, children: [
        Pressable(id: "fire", press: Command(id: "fixture.fire", with: {{ "deck": "$deck" }}),
            child: Spacer(id: "fire-face", size: Some((w: Fixed(100.0), h: Fixed(20.0))))),
        Pressable(id: "group", press: View(id: "group"),
            child: Spacer(id: "group-face", size: Some((w: Fixed(100.0), h: Fixed(20.0))))),
        Optional(id: "block", hidden: View(id: "group"),
            child: Spacer(id: "block-face", size: Some((w: Fixed(100.0), h: Fixed(20.0))))),
    ]))"#
    )
}

#[kithara::test]
fn an_action_inside_a_popover_shut_on_any_action_shuts_it() {
    let ui = compiled(&menu("dismiss: OnAnyAction,"))
        .unwrap_or_else(|error| panic!("the menu must compile: {error}"));

    let mut view = ViewState::new();
    view.set("deck-b/menu", ViewSet::On);

    let host = settle(&ui, "deck-b/fire", ControlAction::Activate, &mut view);

    assert!(!view.flag("deck-b/menu"));
    assert_eq!(
        host,
        Some(write("fixture.fire@deck=b", WriteValue::Trigger))
    );
}

#[kithara::test]
fn an_action_inside_a_popover_shut_on_a_tap_outside_leaves_it_open() {
    for popover in [menu("dismiss: OnTapOutside,"), menu("")] {
        let ui =
            compiled(&popover).unwrap_or_else(|error| panic!("the menu must compile: {error}"));

        let mut view = ViewState::new();
        view.set("deck-b/menu", ViewSet::On);

        let host = settle(&ui, "deck-b/fire", ControlAction::Activate, &mut view);

        assert!(view.flag("deck-b/menu"));
        assert_eq!(
            host,
            Some(write("fixture.fire@deck=b", WriteValue::Trigger))
        );

        let host = settle(&ui, "deck-b/menu", ControlAction::Activate, &mut view);

        assert!(!view.flag("deck-b/menu"), "a tap outside shuts it");
        assert_eq!(host, None);
    }
}

#[kithara::test]
fn a_view_flag_press_inside_a_popover_leaves_it_open() {
    let ui = compiled(&menu("dismiss: OnAnyAction,"))
        .unwrap_or_else(|error| panic!("the menu must compile: {error}"));

    let mut view = ViewState::new();
    view.set("deck-b/menu", ViewSet::On);

    let host = settle(&ui, "deck-b/group", ControlAction::Activate, &mut view);

    assert!(view.flag("deck-b/menu"));
    assert!(view.flag("deck-b/group"));
    assert_eq!(host, None);
}

#[kithara::test]
fn a_popover_any_action_shuts_must_open_on_a_view_flag() {
    let popover = r#"Popover(id: "menu", open: Model(id: "fixture.open", with: { "deck": "$deck" }),
        dismiss: OnAnyAction,
        anchor: Spacer(id: "icon", size: Some((w: Fixed(20.0), h: Fixed(20.0)))),
        content: Pressable(id: "fire", press: Command(id: "fixture.fire", with: { "deck": "$deck" }),
            child: Spacer(id: "fire-face", size: Some((w: Fixed(100.0), h: Fixed(20.0))))))"#;

    let Err(error) = compiled(popover) else {
        panic!("an action cannot shut a popover the host holds open")
    };

    assert!(
        matches!(&error, UiDocError::InvalidId { id, .. } if id == "deck-b/menu"),
        "the refusal must name the popover, got {error}"
    );
}

#[kithara::test]
fn a_new_slot_bound_to_the_wrong_kind_fails_to_compile_naming_the_control() {
    for (control, id) in [
        (
            r#"Pressable(id: "anchor", press: Command(id: "fixture.fire", with: { "deck": "$deck" }),
                secondary: Parameter(id: "fixture.rate", with: { "deck": "$deck" }),
                child: Spacer(id: "face", size: Some((w: Fixed(20.0), h: Fixed(20.0)))))"#,
            "anchor",
        ),
        (
            r#"Wave(id: "wave", size: (w: Fixed(200.0), h: Fixed(40.0)),
                read: Telemetry(id: "fixture.wave", with: { "deck": "$deck" }),
                write_loop_start: Command(id: "fixture.fire", with: { "deck": "$deck" }))"#,
            "wave",
        ),
        (
            r#"Row(id: "tempo", size: (w: Fixed(80.0), h: Fixed(20.0)),
                write: Parameter(id: "fixture.rate", with: { "deck": "$deck" }),
                reset: Parameter(id: "fixture.seek", with: { "deck": "$deck" }),
                children: [Text(id: "label", style: MicroLabel, label: "TEMPO")])"#,
            "tempo",
        ),
        (
            r#"PresetSelector(id: "presets",
                write: Parameter(id: "fixture.seek", with: { "deck": "$deck" }))"#,
            "presets",
        ),
    ] {
        let Err(error) = compiled(control) else {
            panic!("{id} must refuse an endpoint of the wrong kind")
        };
        assert!(
            matches!(&error, UiDocError::BindingType { path, .. } if path.ends_with(id)),
            "the refusal must name {id}, got {error}"
        );
    }
}

fn body_hidden(ui: &CompiledUi, view: &ViewState) -> bool {
    let crate::compile::CompiledNode::Module { root, .. } = &ui.root else {
        panic!("the slots are one module");
    };
    let crate::expand::ExpandedNode::Column { children, .. } = &**root else {
        panic!("the slots stack in a column");
    };
    let Some(crate::expand::ExpandedNode::Optional { block, .. }) = children.get(1) else {
        panic!("the body follows the head");
    };
    crate::render::Ctx::new(
        ui,
        &Interval,
        view,
        builtin::skin_doc(),
        crate::render::Clock::default(),
    )
    .flag(Some(&block.hidden))
}

#[kithara::test]
fn an_inverted_view_read_hides_a_block_until_its_flag_is_set() {
    let ui = compiled(
        r#"Pressable(id: "head", press: View(id: "group"),
                child: Spacer(id: "face", size: Some((w: Fixed(20.0), h: Fixed(20.0))))),
           Optional(id: "body", hidden: View(id: "group", invert: true),
                child: Spacer(id: "inside", size: Some((w: Fixed(20.0), h: Fixed(20.0)))))"#,
    )
    .unwrap_or_else(|error| panic!("an inverted view read must compile: {error}"));
    let mut view = ViewState::default();

    let closed = body_hidden(&ui, &view);
    let host = ui.views().settle(
        Published::Gesture {
            action: ControlAction::Activate,
            path: "deck-b/head".to_owned(),
        },
        &Interval,
        &mut view,
    );
    assert_eq!(host, None, "the head turns only the group's flag");
    let opened = body_hidden(&ui, &view);

    assert_eq!([closed, opened], [true, false]);
}

#[kithara::test]
fn search_delivers_text_by_binding_when_its_id_changes() {
    for id in ["query", "renamed"] {
        let node = format!(
            r#"Search(id: "{id}", read: Model(id: "fixture.query", with: {{ "deck": "$deck" }}), write: Model(id: "fixture.query", with: {{ "deck": "$deck" }}))"#
        );
        assert_eq!(
            delivers(
                &node,
                &format!("deck-b/{id}"),
                &ControlAction::Text("needle".to_owned())
            ),
            Some(write(
                "fixture.query@deck=b",
                WriteValue::Text("needle".to_owned())
            ))
        );
    }
}
