use std::collections::BTreeMap;

use kithara_test_utils::kithara;

use super::{control::*, layout::*, module::*, path::*};
use crate::{
    error::UiDocError,
    expand::ControlSite,
    ids::{EndpointId, SourceUri},
    layout::parse_layout,
    module::{BindingRef, ControlNode, parse_module},
    registry::{EndpointCategory, EndpointDesc, EndpointRegistry, ValueKind},
};

#[derive(Default)]
struct TestRegistry {
    endpoints: BTreeMap<(EndpointCategory, EndpointId), EndpointDesc>,
}

impl TestRegistry {
    fn insert(&mut self, category: EndpointCategory, id: &str, description: EndpointDesc) {
        self.endpoints
            .insert((category, EndpointId(id.to_owned())), description);
    }
}

impl EndpointRegistry for TestRegistry {
    fn endpoint(&self, category: EndpointCategory, id: &EndpointId) -> Option<&EndpointDesc> {
        self.endpoints.get(&(category, id.clone()))
    }
}

fn origin() -> SourceUri {
    SourceUri("dup.ron".into())
}

#[kithara::test]
fn duplicate_instance_reports_path() {
    let text = r#"(schema: "kithara.layout", version: 1, id: "dup",
        root: Split(axis: Horizontal, children: [
            (node: Module(instance: "deck-a", source: "m.ron")),
            (node: Module(instance: "deck-a", source: "m.ron")),
        ]))"#;
    let doc = parse_layout(text, &origin()).unwrap();
    let error = check_layout_instances(&doc, &origin()).unwrap_err();
    let message = error.to_string();
    assert!(message.contains("deck-a"), "{message}");
    assert!(message.contains("Split[1]"), "{message}");
}

#[kithara::test]
fn layout_instance_with_path_separator_is_rejected() {
    let text = r#"(schema: "kithara.layout", version: 1, id: "invalid",
        root: Module(instance: "deck/a", source: "m.ron"))"#;
    let doc = parse_layout(text, &origin()).unwrap();
    let error = check_layout_instances(&doc, &origin()).unwrap_err();
    assert!(matches!(
        error,
        UiDocError::InvalidId { id, reason, .. }
            if id == "deck/a" && reason.contains('/')
    ));
}

#[kithara::test]
fn negative_split_weight_is_rejected() {
    let text = r#"(schema: "kithara.layout", version: 1, id: "invalid",
        root: Split(axis: Horizontal, children: [
            (weight: -1.0, node: Module(instance: "deck-a", source: "m.ron")),
        ]))"#;
    let doc = parse_layout(text, &origin()).unwrap();
    let error = check_layout_instances(&doc, &origin()).unwrap_err();
    assert!(matches!(
        error,
        UiDocError::InvalidWeight { path, value, .. }
            if path == "root/Split[0]" && value == "-1"
    ));
}

#[kithara::test]
fn zero_split_weight_is_rejected() {
    let text = r#"(schema: "kithara.layout", version: 1, id: "invalid",
        root: Split(axis: Horizontal, children: [
            (weight: 0.0, node: Module(instance: "deck-a", source: "m.ron")),
        ]))"#;
    let doc = parse_layout(text, &origin()).unwrap();
    let error = check_layout_instances(&doc, &origin()).unwrap_err();
    assert!(matches!(
        error,
        UiDocError::InvalidWeight { path, value, .. }
            if path == "root/Split[0]" && value == "0"
    ));
}

fn layout_root(root: &str) -> Result<(), UiDocError> {
    let text = format!(r#"(schema: "kithara.layout", version: 1, id: "l", root: {root})"#);
    let doc = parse_layout(&text, &origin())?;
    check_layout_instances(&doc, &origin())
}

fn split_cell(head: &str, tail: &str) -> Result<(), UiDocError> {
    layout_root(&format!(
        r#"Split(axis: Horizontal, {head} children: [
            (node: Module(instance: "deck-a", source: "m.ron"){tail}),
        ])"#
    ))
}

fn measuring_split_cell(tail: &str) -> Result<(), UiDocError> {
    split_cell("measure: Width, size: (w: Fill, h: Fixed(42.0)),", tail)
}

#[kithara::test]
fn a_band_stands_only_among_the_cells_of_a_measuring_split() {
    for band in [", from: 350.0", ", until: Some(350.0)"] {
        let error = split_cell("", band).unwrap_err();
        assert!(
            matches!(&error, UiDocError::UnmeasuredReveal { path, .. }
                if path == "root/Split[0]"),
            "{band}: {error:?}",
        );
    }
}

#[kithara::test]
fn a_measuring_split_reveals_its_own_cells() {
    layout_root(
        r#"Split(axis: Horizontal, measure: Width, size: (w: Fill, h: Fixed(42.0)), children: [
            (node: Module(instance: "menu", source: "m.ron")),
            (node: Module(instance: "strip", source: "m.ron"), until: Some(350.0)),
            (node: Module(instance: "wave", source: "m.ron"), from: 350.0),
        ])"#,
    )
    .unwrap();
}

#[kithara::test]
fn a_measuring_split_must_declare_the_axis_it_measures() {
    for head in [
        "measure: Width,",
        "measure: Height,",
        "measure: Width, size: (w: Shrink, h: Fill),",
        "measure: Height, size: (w: Fill, h: Shrink),",
    ] {
        let error = split_cell(head, "").unwrap_err();
        assert!(
            matches!(&error, UiDocError::UnmeasuredAxis { path, .. }
                if path == "root/Split"),
            "{head}: {error:?}",
        );
    }
}

#[kithara::test]
fn a_split_band_is_finite_and_closes_above_the_room_it_opens_in() {
    for band in [", from: -1.0", ", from: inf", ", from: NaN"] {
        let error = measuring_split_cell(band).unwrap_err();
        assert!(
            matches!(&error, UiDocError::RevealThreshold { .. }),
            "{band}: {error:?}",
        );
    }
    for band in [
        ", from: 350.0, until: Some(350.0)",
        ", from: 350.0, until: Some(0.0)",
        ", until: Some(-inf)",
        ", until: Some(NaN)",
    ] {
        let error = measuring_split_cell(band).unwrap_err();
        assert!(
            matches!(&error, UiDocError::RevealBand { .. }),
            "{band}: {error:?}",
        );
    }
}

#[kithara::test]
fn empty_and_parameter_like_ids_are_rejected() {
    for id in ["", "$deck"] {
        assert!(matches!(
            check_id(id, &origin()),
            Err(UiDocError::InvalidId { id: invalid, .. }) if invalid == id
        ));
    }
}

#[kithara::test]
fn duplicate_control_id_reports_path() {
    let text = r#"(schema: "kithara.module", version: 1, id: "m",
        root: Row(children: [
            Button(id: "play", label: "PLAY"),
            Button(id: "play", label: "PLAY"),
        ]))"#;
    let doc = parse_module(text, &origin()).unwrap();
    let error = check_module_node_ids(&doc, &origin()).unwrap_err();
    assert!(error.to_string().contains("Control(play)"));
}

#[kithara::test]
fn control_id_with_path_separator_is_rejected() {
    let text = r#"(schema: "kithara.module", version: 1, id: "m",
        root: Button(id: "transport/play", label: "PLAY"))"#;
    let doc = parse_module(text, &origin()).unwrap();
    let error = check_module_node_ids(&doc, &origin()).unwrap_err();
    assert!(matches!(
        error,
        UiDocError::InvalidId { id, reason, .. }
            if id == "transport/play" && reason.contains('/')
    ));
}

#[kithara::test]
fn an_object_may_move_a_whole_row() {
    let text = r#"(schema: "kithara.module", version: 1, id: "m",
        root: Object(id: "shift", transform: (position: (8.0, 0.0)),
            child: Row(children: [Button(id: "play", label: "PLAY")])))"#;
    let doc = parse_module(text, &origin()).unwrap();

    assert!(check_module_node_ids(&doc, &origin()).is_ok());
}

#[kithara::test]
fn an_object_may_not_turn_a_row() {
    let text = r#"(schema: "kithara.module", version: 1, id: "m",
        root: Object(id: "spin", transform: (rotation: 30.0),
            child: Row(children: [Button(id: "play", label: "PLAY")])))"#;
    let doc = parse_module(text, &origin()).unwrap();

    let error = check_module_node_ids(&doc, &origin()).unwrap_err();

    assert!(matches!(
        error,
        UiDocError::ObjectGroup { child: "Row", .. }
    ));
}

#[kithara::test]
fn an_object_may_not_scale_a_row_either() {
    let text = r#"(schema: "kithara.module", version: 1, id: "m",
        root: Object(id: "grow", transform: (scale: (2.0, 2.0)),
            child: Row(children: [Button(id: "play", label: "PLAY")])))"#;
    let doc = parse_module(text, &origin()).unwrap();

    assert!(check_module_node_ids(&doc, &origin()).is_err());
}

/// A visualiser paints its own pass, so an object over it would move the
/// box and leave the picture. Refusing beats drawing the wrong answer.
#[kithara::test]
fn an_object_may_not_even_move_a_native_pass() {
    let text = r#"(schema: "kithara.module", version: 1, id: "m",
        root: Object(id: "shift", transform: (position: (8.0, 0.0)),
            child: Vis(id: "scope")))"#;
    let doc = parse_module(text, &origin()).unwrap();

    let error = check_module_node_ids(&doc, &origin()).unwrap_err();

    assert!(matches!(
        error,
        UiDocError::ObjectNative { child: "Vis", .. }
    ));
}

/// A still object is the identity, and the identity reaches everything
/// because it does nothing.
#[kithara::test]
fn a_still_object_may_wrap_anything() {
    let text = r#"(schema: "kithara.module", version: 1, id: "m",
        root: Object(id: "still", child: Vis(id: "scope")))"#;
    let doc = parse_module(text, &origin()).unwrap();

    assert!(check_module_node_ids(&doc, &origin()).is_ok());
}

/// The walk ends in a catch-all that records an id and stops, so a
/// container the walk does not name is validated as a leaf and its children
/// are never looked at. This test fails the moment `Stage` falls into it.
#[kithara::test]
fn a_stage_walks_its_children() {
    let text = r#"(schema: "kithara.module", version: 1, id: "m",
        root: Stage(id: "scene", children: [
            Button(id: "play", label: "PLAY"),
            Button(id: "play", label: "AGAIN"),
        ]))"#;
    let doc = parse_module(text, &origin()).unwrap();

    let error = check_module_node_ids(&doc, &origin()).unwrap_err();

    assert!(matches!(
        error,
        UiDocError::DuplicateId { id, .. } if id == "play"
    ));
}

/// Every child of a stage gets the whole box, so a stage is several boxes,
/// and a turn about one origin would take them apart.
#[kithara::test]
fn an_object_may_not_turn_a_stage() {
    let text = r#"(schema: "kithara.module", version: 1, id: "m",
        root: Object(id: "spin", transform: (rotation: 30.0),
            child: Stage(id: "scene", children: [Button(id: "play", label: "PLAY")])))"#;
    let doc = parse_module(text, &origin()).unwrap();

    let error = check_module_node_ids(&doc, &origin()).unwrap_err();

    assert!(matches!(
        error,
        UiDocError::ObjectGroup { child: "Stage", .. }
    ));
}

/// A move carries every box by the same vector, so it reaches a stage the
/// way it reaches a row.
#[kithara::test]
fn an_object_may_move_a_whole_stage() {
    let text = r#"(schema: "kithara.module", version: 1, id: "m",
        root: Object(id: "shift", transform: (position: (8.0, 0.0)),
            child: Stage(id: "scene", children: [Button(id: "play", label: "PLAY")])))"#;
    let doc = parse_module(text, &origin()).unwrap();

    assert!(check_module_node_ids(&doc, &origin()).is_ok());
}

/// A motion computes the phase, so an object carrying both leaves one pose
/// with two answers. There is no honest rule for ranking them, and inventing
/// one is what refusing here avoids.
#[kithara::test]
fn an_object_may_not_be_driven_twice() {
    let text = r#"(schema: "kithara.module", version: 1, id: "m",
        root: Object(id: "spin", to: (rotation: 360.0),
            phase: Model(id: "app.phase"),
            motion: (clock: Model(id: "app.time"), duration: 4.0),
            child: Button(id: "play", label: "PLAY")))"#;
    let doc = parse_module(text, &origin()).unwrap();

    let error = check_module_node_ids(&doc, &origin()).unwrap_err();

    assert!(matches!(error, UiDocError::ObjectDrivenTwice { .. }));
}

#[kithara::test]
fn an_object_may_be_driven_by_a_motion_alone() {
    let text = r#"(schema: "kithara.module", version: 1, id: "m",
        root: Object(id: "spin", to: (rotation: 360.0),
            motion: (clock: Model(id: "app.time"), duration: 4.0, repeat: Loop),
            child: Button(id: "play", label: "PLAY")))"#;
    let doc = parse_module(text, &origin()).unwrap();

    assert!(check_module_node_ids(&doc, &origin()).is_ok());
}

#[kithara::test]
fn module_id_with_an_address_separator_is_rejected() {
    let text = r#"(schema: "kithara.module", version: 1, id: "studio.strip",
        root: Button(id: "play", label: "PLAY"))"#;
    let doc = parse_module(text, &origin()).unwrap();
    let error = check_module_id(&doc, &origin()).unwrap_err();
    assert!(matches!(
        error,
        UiDocError::InvalidId { id, reason, .. }
            if id == "studio.strip" && reason.contains("'.'")
    ));
}

#[kithara::test]
fn a_container_that_writes_without_an_id_is_rejected() {
    let text = r#"(schema: "kithara.module", version: 1, id: "m",
        root: Row(write: Parameter(id: "deck.tempo.rate"), children: [
            Button(id: "play", label: "PLAY"),
        ]))"#;
    let doc = parse_module(text, &origin()).unwrap();
    let error = check_module_node_ids(&doc, &origin()).unwrap_err();
    assert!(matches!(
        error,
        UiDocError::UnaddressedSurface { path, .. } if path == "root"
    ));
}

fn adaptive(steps: &str) -> Result<(), UiDocError> {
    let text = format!(
        r#"(schema: "kithara.module", version: 1, id: "m",
            root: Adaptive(
                id: "bank",
                measure: Read(Model(id: "ui.measure")),
                base: Knob(id: "low"),
                steps: [{steps}],
            ))"#
    );
    let doc = parse_module(&text, &origin())?;
    check_module_node_ids(&doc, &origin())
}

#[kithara::test]
fn an_adaptive_node_without_steps_is_rejected() {
    let error = adaptive("").unwrap_err();

    assert!(
        matches!(&error, UiDocError::AdaptiveWithoutSteps { id, path, .. }
            if id == "bank" && path == "root/Adaptive(bank)"),
        "{error:?}"
    );
}

#[kithara::test]
fn adaptive_steps_must_climb() {
    for (steps, at) in [
        (
            r#"(from: 4.0, node: Knob(id: "a")), (from: 2.0, node: Knob(id: "b"))"#,
            1,
        ),
        (
            r#"(from: 4.0, node: Knob(id: "a")), (from: 4.0, node: Knob(id: "b"))"#,
            1,
        ),
        (r#"(from: NaN, node: Knob(id: "a"))"#, 0),
    ] {
        let error = adaptive(steps).unwrap_err();
        assert!(
            matches!(&error, UiDocError::AdaptiveStepOrder { index, .. } if *index == at),
            "{steps}: {error:?}"
        );
    }
}

fn measured(measure: &str, size: &str) -> Result<(), UiDocError> {
    let text = format!(
        r#"(schema: "kithara.module", version: 1, id: "m",
            root: Adaptive(
                id: "bank",
                measure: {measure},
                {size}
                base: Knob(id: "low"),
                steps: [(from: 4.0, node: Knob(id: "high"))],
            ))"#
    );
    let doc = parse_module(&text, &origin())?;
    check_module_node_ids(&doc, &origin())
}

#[kithara::test]
fn a_self_measured_node_must_declare_the_axis_it_measures() {
    for (measure, size) in [
        ("Width", ""),
        ("Height", ""),
        ("Width", "size: Some((w: Shrink, h: Fill)),"),
        ("Height", "size: Some((w: Fill, h: Shrink)),"),
    ] {
        let error = measured(measure, size).unwrap_err();
        assert!(
            matches!(&error, UiDocError::UnmeasuredAxis { path, .. }
                if path == "root/Adaptive(bank)"),
            "{measure} {size}: {error:?}",
        );
    }
}

#[kithara::test]
fn a_declared_axis_carries_a_self_measured_node() {
    measured("Width", "size: Some((w: Fill, h: Shrink)),").unwrap();
    measured("Height", "size: Some((w: Shrink, h: Fixed(80.0))),").unwrap();
}

#[kithara::test]
fn a_read_measure_declares_no_box() {
    let error = measured(
        r#"Read(Model(id: "ui.measure"))"#,
        "size: Some((w: Fill, h: Fill)),",
    )
    .unwrap_err();

    assert!(
        matches!(&error, UiDocError::MeasuredBoxWithoutAxis { id, .. } if id == "bank"),
        "{error:?}",
    );
}

fn module_root(root: &str) -> Result<(), UiDocError> {
    let text = format!(r#"(schema: "kithara.module", version: 1, id: "m", root: {root})"#);
    let doc = parse_module(&text, &origin())?;
    check_module_node_ids(&doc, &origin())
}

const BAR: &str = r#"id: "bar", measure: Width, size: (w: Fill, h: Fixed(42.0)),"#;

#[kithara::test]
fn a_reveal_stands_only_among_the_children_of_a_measuring_container() {
    for root in [
        r#"Reveal(from: 1.0, child: Knob(id: "low"))"#.to_owned(),
        r#"Row(children: [Reveal(from: 1.0, child: Knob(id: "low"))])"#.to_owned(),
        format!(
            r#"Row({BAR} children: [Pressable(id: "press", press: Command(id: "ui.press"),
                child: Reveal(from: 1.0, child: Knob(id: "low")))])"#
        ),
        format!(
            r#"Row({BAR} children: [Reveal(from: 1.0,
                child: Reveal(from: 2.0, child: Knob(id: "low")))])"#
        ),
    ] {
        let error = module_root(&root).unwrap_err();
        assert!(
            matches!(&error, UiDocError::UnmeasuredReveal { .. }),
            "{root}: {error:?}",
        );
    }
}

#[kithara::test]
fn a_measuring_container_reveals_its_own_children() {
    module_root(&format!(
        r#"Row({BAR} children: [
            Knob(id: "low"),
            Reveal(from: 0.0, child: Knob(id: "mid")),
            Reveal(from: 350.0, child: Knob(id: "high")),
        ])"#
    ))
    .unwrap();
}

#[kithara::test]
fn a_measuring_container_must_declare_the_axis_it_measures() {
    for (measure, size) in [
        ("Width", ""),
        ("Height", ""),
        ("Width", "size: (w: Shrink, h: Fill),"),
        ("Height", "size: (w: Fill, h: Shrink),"),
    ] {
        let root =
            format!(r#"Row(id: "bar", measure: {measure}, {size} children: [Knob(id: "a")])"#);
        let error = module_root(&root).unwrap_err();
        assert!(
            matches!(&error, UiDocError::UnmeasuredAxis { path, .. }
                if path == "root/Group(bar)"),
            "{root}: {error:?}",
        );
    }
}

#[kithara::test]
fn a_threshold_is_finite_and_not_negative() {
    for from in ["-1.0", "inf", "-inf", "NaN"] {
        let root = format!(r#"Row({BAR} children: [Reveal(from: {from}, child: Knob(id: "a"))])"#);
        let error = module_root(&root).unwrap_err();
        assert!(
            matches!(&error, UiDocError::RevealThreshold { .. }),
            "{from}: {error:?}",
        );
    }
}

#[kithara::test]
fn a_band_closes_above_the_room_it_opens_in() {
    for until in ["0.0", "350.0", "inf", "-inf", "NaN"] {
        let root = format!(
            r#"Row({BAR} children: [Reveal(from: 350.0, until: Some({until}),
                child: Knob(id: "a"))])"#
        );
        let error = module_root(&root).unwrap_err();
        assert!(
            matches!(&error, UiDocError::RevealBand { .. }),
            "{until}: {error:?}",
        );
    }
}

#[kithara::test]
fn bands_meeting_at_one_number_stand_in_one_line() {
    module_root(&format!(
        r#"Row({BAR} children: [
            Reveal(from: 0.0, until: Some(350.0), child: Knob(id: "strip")),
            Reveal(from: 350.0, child: Knob(id: "wave")),
        ])"#
    ))
    .unwrap();
}

#[kithara::test]
fn thresholds_need_not_climb() {
    module_root(&format!(
        r#"Row({BAR} children: [
            Reveal(from: 520.0, child: Knob(id: "a")),
            Reveal(from: 350.0, child: Knob(id: "b")),
        ])"#
    ))
    .unwrap();
}

#[kithara::test]
fn adaptive_branches_may_name_the_same_place() {
    adaptive(r#"(from: 4.0, node: Knob(id: "low"))"#).unwrap();
}

#[kithara::test]
fn a_duplicate_id_inside_one_adaptive_branch_is_rejected() {
    let error = adaptive(r#"(from: 4.0, node: Row(children: [Knob(id: "dup"), Knob(id: "dup")]))"#)
        .unwrap_err();

    assert!(
        matches!(&error, UiDocError::DuplicateId { id, .. } if id == "dup"),
        "{error:?}"
    );
}

#[kithara::test]
fn a_sibling_after_an_adaptive_node_sees_every_branch_id() {
    let text = r#"(schema: "kithara.module", version: 1, id: "m",
        root: Row(children: [
            Adaptive(
                id: "bank",
                measure: Read(Model(id: "ui.measure")),
                base: Knob(id: "low"),
                steps: [(from: 4.0, node: Knob(id: "low-mid"))],
            ),
            Knob(id: "low-mid"),
        ]))"#;
    let doc = parse_module(text, &origin()).unwrap();
    let error = check_module_node_ids(&doc, &origin()).unwrap_err();

    assert!(
        matches!(&error, UiDocError::DuplicateId { id, .. } if id == "low-mid"),
        "{error:?}"
    );
}

#[kithara::test]
fn unique_ids_pass() {
    let text = r#"(schema: "kithara.module", version: 1, id: "m",
        root: Row(children: [
            Button(id: "play", label: "PLAY"),
            Slot(id: "extra"),
        ]))"#;
    let doc = parse_module(text, &origin()).unwrap();
    check_module_node_ids(&doc, &origin()).unwrap();
}

fn check_control(body: &str, path: &str, write: Option<&BindingRef>) -> Result<(), UiDocError> {
    let text = format!(r#"(schema: "kithara.module", version: 1, id: "test", root: {body})"#);
    let document = parse_module(&text, &origin())?;
    let scope = match &document.root {
        ControlNode::ContextBar { scope, .. } => scope.as_ref(),
        _ => None,
    };
    let zoom = match &document.root {
        ControlNode::Wave { zoom, .. } => zoom.as_ref(),
        _ => None,
    };
    let active = match &document.root {
        ControlNode::Text { active, .. } => active.as_ref(),
        _ => None,
    };
    check_controls(
        ControlSite {
            path,
            write,
            scope,
            zoom,
            active,
            control: &document.root,
            read: None,
            columns: &[],
            columns_state: None,
            query: None,
        },
        &origin(),
        &registry(),
    )
}

fn registry() -> TestRegistry {
    let mut registry = TestRegistry::default();
    registry.insert(
        EndpointCategory::Command,
        "deck.transport.toggle_play",
        EndpointDesc::new(ValueKind::Trigger).with_scope("deck"),
    );
    registry.insert(
        EndpointCategory::Parameter,
        "player.output.volume",
        EndpointDesc::new(ValueKind::Scalar),
    );
    registry.insert(
        EndpointCategory::Model,
        "library.breadcrumb",
        EndpointDesc::new(ValueKind::Text),
    );
    registry
}

fn with_deck() -> BTreeMap<String, String> {
    std::iter::once(("deck".to_owned(), "a".to_owned())).collect()
}

#[kithara::test]
fn valid_command_binding_passes() {
    let binding = BindingRef::Command {
        id: EndpointId("deck.transport.toggle_play".into()),
        with: with_deck(),
    };
    check_control(
        r#"Button(id: "play", label: "PLAY")"#,
        "play",
        Some(&binding),
    )
    .unwrap();
}

#[kithara::test]
fn tree_query_binding_must_be_text() {
    let document = parse_module(
        r#"(schema: "kithara.module", version: 1, id: "tree",
            root: Tree(
                id: "browser",
                query: Parameter(id: "player.output.volume"),
            ))"#,
        &origin(),
    )
    .unwrap();
    let ControlNode::Tree { query, .. } = &document.root else {
        panic!("expected tree");
    };

    let error = check_controls(
        ControlSite {
            path: "tree/browser",
            control: &document.root,
            read: None,
            write: None,
            columns: &[],
            columns_state: None,
            query: query.as_ref(),
            scope: None,
            zoom: None,
            active: None,
        },
        &origin(),
        &registry(),
    )
    .unwrap_err();

    assert!(matches!(
        error,
        UiDocError::BindingType {
            expected,
            got,
            path,
            ..
        } if expected == "Text" && got == "Scalar" && path == "tree/browser"
    ));
}

#[kithara::test]
fn wave_zoom_binding_must_be_scalar() {
    let error = check_control(
        r#"Wave(id: "wave", zoom: Model(id: "library.breadcrumb"))"#,
        "deck/wave",
        None,
    )
    .unwrap_err();

    assert!(matches!(
        error,
        UiDocError::BindingType {
            expected,
            got,
            path,
            ..
        } if expected == "Scalar" && got == "Text" && path == "deck/wave"
    ));
}

#[kithara::test]
fn context_scope_items_require_scope_binding() {
    let error = check_control(
        r#"ContextBar(id: "context", scope_items: ["LOCAL"])"#,
        "library/context",
        None,
    )
    .unwrap_err();

    assert!(matches!(
        error,
        UiDocError::InvalidContextScope { path, .. } if path == "library/context"
    ));
}

#[kithara::test]
fn context_scope_binding_must_be_scalar() {
    let write = BindingRef::Parameter {
        id: EndpointId("player.output.volume".into()),
        with: BTreeMap::new(),
    };
    let error = check_control(
        r#"ContextBar(
            id: "context",
            scope_items: ["LOCAL"],
            scope: Model(id: "library.breadcrumb"),
        )"#,
        "library/context",
        Some(&write),
    )
    .unwrap_err();

    assert!(matches!(
        error,
        UiDocError::BindingType {
            expected,
            got,
            path,
            ..
        } if expected == "Scalar" && got == "Text" && path == "library/context"
    ));
}

#[kithara::test]
fn missing_scope_is_reported() {
    let binding = BindingRef::Command {
        id: EndpointId("deck.transport.toggle_play".into()),
        with: BTreeMap::new(),
    };
    let error = check_control(
        r#"Button(id: "play", label: "PLAY")"#,
        "play",
        Some(&binding),
    )
    .unwrap_err();
    assert!(matches!(
        error,
        UiDocError::MissingScope { scope, .. } if scope == "deck"
    ));
}

#[kithara::test]
fn undeclared_command_scope_is_reported() {
    let mut with = with_deck();
    with.insert("sidechain".to_owned(), "1".to_owned());
    let binding = BindingRef::Command {
        with,
        id: EndpointId("deck.transport.toggle_play".into()),
    };
    let error = check_control(
        r#"Button(id: "play", label: "PLAY")"#,
        "play",
        Some(&binding),
    )
    .unwrap_err();
    assert!(matches!(
        error,
        UiDocError::UnknownScope {
            id,
            scope,
            path,
            ..
        } if id == "deck.transport.toggle_play" && scope == "sidechain" && path == "play"
    ));
}

#[kithara::test]
fn scope_on_unscoped_parameter_is_reported() {
    let binding = BindingRef::Parameter {
        id: EndpointId("player.output.volume".into()),
        with: with_deck(),
    };
    let error = check_control(r#"Fader(id: "volume")"#, "volume", Some(&binding)).unwrap_err();
    assert!(matches!(
        error,
        UiDocError::UnknownScope {
            id,
            scope,
            path,
            ..
        } if id == "player.output.volume" && scope == "deck" && path == "volume"
    ));
}

#[kithara::test]
fn crossfader_requires_scalar_read_and_write_endpoints() {
    let document = parse_module(
        r#"(schema: "kithara.module", version: 1, id: "mixer",
            root: Crossfader(
                id: "xfade",
                read: Model(id: "library.breadcrumb"),
                write: Parameter(id: "player.output.volume"),
            ))"#,
        &origin(),
    )
    .unwrap();
    let ControlNode::Crossfader { read, write, .. } = &document.root else {
        panic!("expected crossfader");
    };

    let error = check_controls(
        ControlSite {
            path: "mixer/xfade",
            control: &document.root,
            read: read.as_ref(),
            write: write.as_ref(),
            columns: &[],
            columns_state: None,
            query: None,
            scope: None,
            zoom: None,
            active: None,
        },
        &origin(),
        &registry(),
    )
    .unwrap_err();

    assert!(matches!(
        error,
        UiDocError::BindingType {
            expected,
            got,
            path,
            ..
        } if expected == "Scalar" && got == "Text" && path == "mixer/xfade"
    ));
}

#[kithara::test]
fn model_binding_on_write_side_is_direction_error() {
    let binding = BindingRef::Model {
        id: EndpointId("library.visible_tracks".into()),
        with: BTreeMap::new(),
    };
    let error = check_control(
        r#"Button(id: "play", label: "PLAY")"#,
        "play",
        Some(&binding),
    )
    .unwrap_err();
    assert!(matches!(error, UiDocError::BindingDirection { .. }));
}
