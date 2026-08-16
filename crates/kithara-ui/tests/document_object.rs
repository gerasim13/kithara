//! What an object does to the document, measured where a host would see it.
//!
//! The pose is resolved in the neutral facade, so the one place worth testing
//! is the argument every host receives: a control's transform. Neither host is
//! involved here, which is the point — if this is right, both draw it right.

mod common;

use kithara_test_utils::kithara;
use kithara_ui::{
    builtin,
    compile::{CompiledUi, compile},
    expand::{Binding, ControlSpec},
    geom::{Pt, Transform},
    ids::InternId,
    layout::Axis,
    registry::{EndpointCategory, EndpointDesc, ValueKind},
    render::{
        InputOwner, ReadValue, Reads,
        document::{Group, Host, Module, Popover, render},
    },
    size::SizeSpec,
    source::UiConfig,
};

/// One control, and where the document put what it draws.
#[derive(Debug, PartialEq)]
struct Placed {
    path: String,
    transform: Transform,
}

struct Spy<'a> {
    ui: &'a CompiledUi,
}

impl Spy<'_> {
    fn flatten<T>(groups: impl IntoIterator<Item = Vec<T>>) -> Vec<T> {
        groups.into_iter().flatten().collect()
    }
}

impl Host for Spy<'_> {
    type Output = Vec<Placed>;

    fn split(&mut self, _axis: Axis, children: Vec<(f32, SizeSpec, Self::Output)>) -> Self::Output {
        Self::flatten(children.into_iter().map(|(_, _, output)| output))
    }

    fn module(&mut self, _module: Module<'_>, content: Option<Self::Output>) -> Self::Output {
        content.unwrap_or_default()
    }

    fn group(
        &mut self,
        _group: Group<'_>,
        children: Vec<(Option<f32>, Self::Output)>,
    ) -> Self::Output {
        Self::flatten(children.into_iter().map(|(_, output)| output))
    }

    fn popover(
        &mut self,
        _popover: Popover,
        mut anchor: Self::Output,
        content: Option<Self::Output>,
    ) -> Self::Output {
        anchor.extend(content.unwrap_or_default());
        anchor
    }

    fn pressable(
        &mut self,
        _path: InternId,
        child: Self::Output,
        _size: Option<SizeSpec>,
    ) -> Self::Output {
        child
    }

    fn scroll(
        &mut self,
        _id: InternId,
        child: Self::Output,
        _size: Option<SizeSpec>,
    ) -> Self::Output {
        child
    }

    fn slot(&mut self, children: Vec<Self::Output>, _size: Option<SizeSpec>) -> Self::Output {
        Self::flatten(children)
    }

    fn stage(&mut self, children: Vec<Self::Output>, _size: Option<SizeSpec>) -> Self::Output {
        Self::flatten(children)
    }

    fn control(
        &mut self,
        path: InternId,
        _spec: &ControlSpec,
        _read: Option<&Binding>,
        _owner: InputOwner,
        _size: Option<SizeSpec>,
        transform: Transform,
    ) -> Self::Output {
        vec![Placed {
            path: self.ui.resolve(path).to_owned(),
            transform,
        }]
    }

    fn hosted(
        &mut self,
        _node: &kithara_ui::expand::ExpandedNode,
        child: Self::Output,
    ) -> Self::Output {
        child
    }

    fn window(
        &mut self,
        content: Self::Output,
        _dragged: Option<String>,
        _resize_edges: bool,
    ) -> Self::Output {
        content
    }
}

/// One scalar the document can be driven by, standing in for whatever the app
/// advances between frames.
struct Phase(Option<f64>);

impl Reads for Phase {
    fn get(&self, endpoint: &str) -> Option<ReadValue<'_>> {
        match endpoint {
            "gallery.phase" => self.0.map(ReadValue::Scalar),
            _ => None,
        }
    }
}

fn registry() -> common::TestRegistry {
    let mut registry = common::player_registry();
    registry.insert(
        EndpointCategory::Model,
        "gallery.phase",
        EndpointDesc::new(ValueKind::Scalar),
    );
    registry
}

fn placed(root: &str, reads: &Phase) -> Vec<Placed> {
    let mut resolver = builtin::resolver();
    resolver.insert(
        "object.klayout.ron",
        r#"(schema: "kithara.layout", version: 1, id: "object-document",
            root: Module(instance: "page", source: "modules/object.kmodule.ron"))"#,
    );
    resolver.insert(
        "modules/object.kmodule.ron",
        &format!(r#"(schema: "kithara.module", version: 1, id: "object", root: {root})"#),
    );
    let ui = compile(
        "object.klayout.ron",
        &resolver,
        &registry(),
        builtin::skin_doc(),
        &UiConfig::default(),
    )
    .unwrap_or_else(|error| panic!("the fixture must compile: {error}"));

    render(&ui.root, &ui, reads, builtin::skin_doc(), Spy { ui: &ui })
}

fn only(root: &str, reads: &Phase) -> Transform {
    let placed = placed(root, reads);
    let [one] = placed.as_slice() else {
        panic!("the fixture mounts one control, not {placed:?}");
    };
    one.transform
}

const STILL: Phase = Phase(None);

#[kithara::test]
fn a_control_no_object_wraps_is_left_where_it_was() {
    let alone = only(r#"Text(id: "leaf")"#, &STILL);

    assert!(alone.is_identity());
}

#[kithara::test]
fn an_object_offsets_the_control_it_wraps() {
    let moved = only(
        r#"Object(id: "shift", transform: (position: (10.0, 4.0)), child: Text(id: "leaf"))"#,
        &STILL,
    );

    assert_eq!(moved, Transform::translate(Pt { x: 10.0, y: 4.0 }));
}

#[kithara::test]
fn nested_objects_compose_into_one_offset() {
    let moved = only(
        r#"Object(id: "outer", transform: (position: (10.0, 0.0)),
            child: Object(id: "inner", transform: (position: (0.0, 4.0)),
                child: Text(id: "leaf")))"#,
        &STILL,
    );

    assert_eq!(moved, Transform::translate(Pt { x: 10.0, y: 4.0 }));
}

/// A move applies to a whole subtree because every box in it shifts by the same
/// vector, so two siblings under one object carry the same offset rather than a
/// share of it.
#[kithara::test]
fn two_controls_under_one_moved_object_carry_the_same_offset() {
    let placed = placed(
        r#"Object(id: "shift", transform: (position: (10.0, 0.0)),
            child: Row(children: [Text(id: "one"), Text(id: "two")]))"#,
        &STILL,
    );

    let [first, second] = placed.as_slice() else {
        panic!("the fixture mounts two controls, not {placed:?}");
    };
    assert_eq!(first.transform, second.transform);
}

const TRACK: &str = r#"Object(id: "travel",
    to: (position: (100.0, 0.0)),
    phase: Model(id: "gallery.phase"),
    child: Text(id: "leaf"))"#;

#[kithara::test]
fn the_start_of_a_track_leaves_the_control_alone() {
    assert!(only(TRACK, &Phase(Some(0.0))).is_identity());
}

#[kithara::test]
fn the_end_of_a_track_puts_the_control_at_the_far_pose() {
    assert_eq!(
        only(TRACK, &Phase(Some(1.0))),
        Transform::translate(Pt { x: 100.0, y: 0.0 })
    );
}

/// The whole point of resolving the pose per frame: the same compiled document
/// draws in a different place when the endpoint behind it moves.
#[kithara::test]
fn moving_the_endpoint_moves_the_control() {
    assert_ne!(
        only(TRACK, &Phase(Some(0.25))),
        only(TRACK, &Phase(Some(0.75)))
    );
}

/// A track nobody drives is not half-applied: the object sits at the pose the
/// document wrote down, which is where it would sit with no track at all.
#[kithara::test]
fn a_track_with_no_answer_sits_at_its_written_pose() {
    assert!(only(TRACK, &STILL).is_identity());
}
