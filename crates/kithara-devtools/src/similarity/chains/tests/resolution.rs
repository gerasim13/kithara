use super::{consts, lists, report, unreached};
use crate::similarity::chains::{
    facts,
    graph::{Graph, Node},
    resolve::Resolver,
};

fn sources(files: &[(&str, &str)]) -> Vec<(String, String)> {
    files
        .iter()
        .map(|(file, source)| (format!("{}/{file}.rs", consts::ROOT), (*source).to_owned()))
        .collect()
}

#[test]
fn repeated_method_lookups_keep_specialized_receiver_arguments() {
    let sources = sources(&[(
        "lib",
        "pub struct Left; pub struct Right; pub struct Holder<T> { value: T }
         pub trait Read { fn read(&self) {} }
         impl Read for Holder<Left> {}
         pub fn left(holder: &Holder<Left>) { holder.read(); holder.read(); }
         pub fn right(holder: &Holder<Right>) { holder.read(); holder.read(); }",
    )]);
    let facts = facts::collect(&sources).expect("facts");
    let mut resolver = Resolver::new(&facts, 8);
    let graph = Graph::build(&facts, &mut resolver, 8, 8);
    let left = facts
        .fns
        .iter()
        .position(|fact| fact.name == "left")
        .expect("left");
    let right = facts
        .fns
        .iter()
        .position(|fact| fact.name == "right")
        .expect("right");
    let read = facts
        .fns
        .iter()
        .position(|fact| fact.name == "read")
        .expect("read");

    assert_eq!(graph.successors(Node::Fn(left)), &[Node::Fn(read)]);
    assert!(graph.successors(Node::Fn(right)).is_empty());
}

#[test]
fn a_depth_limited_reexport_does_not_poison_a_shallower_lookup() {
    let sources = sources(&[
        (
            "lib",
            "pub mod a; pub mod bridge; pub mod long; pub mod hop1; pub mod hop2; pub mod hop3;
             pub fn deep(reader: &long::Handle) { reader.read(); }
             pub fn shallow(reader: &bridge::Handle) { reader.read(); }",
        ),
        (
            "a",
            "pub struct Reader; impl Reader { pub(crate) fn read(&self) {} }",
        ),
        ("bridge", "pub use crate::a::Reader as Handle;"),
        ("long", "pub use crate::hop1::Handle;"),
        ("hop1", "pub use crate::hop2::Handle;"),
        ("hop2", "pub use crate::hop3::Handle;"),
        ("hop3", "pub use crate::bridge::Handle;"),
    ]);
    let facts = facts::collect(&sources).expect("facts");
    let mut resolver = Resolver::new(&facts, 8);
    let graph = Graph::build(&facts, &mut resolver, 8, 8);
    let deep = facts
        .fns
        .iter()
        .position(|fact| fact.name == "deep")
        .expect("deep");
    let shallow = facts
        .fns
        .iter()
        .position(|fact| fact.name == "shallow")
        .expect("shallow");
    let read = facts
        .fns
        .iter()
        .position(|fact| fact.name == "read")
        .expect("read");

    assert!(graph.successors(Node::Fn(deep)).is_empty());
    assert_eq!(graph.successors(Node::Fn(shallow)), &[Node::Fn(read)]);
}

#[test]
fn a_qualified_receiver_reaches_only_its_module_method() {
    let sources = sources(&[
        (
            "lib",
            "pub mod a; pub mod b; pub fn read_a(reader: &a::Reader) -> u64 { reader.read() }",
        ),
        (
            "a",
            "pub struct Reader; impl Reader { pub(crate) fn read(&self) -> u64 { 1 } }",
        ),
        (
            "b",
            "pub struct Reader; impl Reader { pub(crate) fn read(&self) -> u64 { 2 } }",
        ),
    ]);
    let unreached = report(&sources, 0).coverage.unreached_private;

    assert!(
        !unreached
            .iter()
            .any(|entry| entry.contains("/a.rs:") && entry.ends_with("Reader::read")),
        "{unreached:?}"
    );
    assert!(
        unreached
            .iter()
            .any(|entry| entry.contains("/b.rs:") && entry.ends_with("Reader::read")),
        "{unreached:?}"
    );
}

#[test]
fn same_named_aliases_keep_their_declared_targets() {
    let sources = sources(&[
        (
            "lib",
            "pub mod a; pub mod b; pub fn drive(reader: &a::Handle) -> u64 { reader.tick() }",
        ),
        (
            "a",
            "pub type Handle = Reader; pub struct Reader; impl Reader { pub(crate) fn tick(&self) -> u64 { 1 } }",
        ),
        (
            "b",
            "pub type Handle = Writer; pub struct Writer; impl Writer { pub(crate) fn tick(&self) -> u64 { 2 } }",
        ),
    ]);
    let unreached = report(&sources, 0).coverage.unreached_private;

    assert!(!lists(&unreached, "Reader::tick"), "{unreached:?}");
    assert!(lists(&unreached, "Writer::tick"), "{unreached:?}");
}

#[test]
fn same_named_traits_do_not_share_default_dispatch() {
    let sources = sources(&[
        (
            "lib",
            "pub mod a; pub mod b; use a::Read; pub fn drive(reader: &a::Left) -> u64 { reader.read() }",
        ),
        (
            "a",
            "pub trait Read { fn read(&self) -> u64 { 1 } } pub struct Left; impl Read for Left {}",
        ),
        (
            "b",
            "pub trait Read { fn read(&self) -> u64 { 2 } } pub struct Right; impl Read for Right {}",
        ),
    ]);
    let facts = facts::collect(&sources).expect("facts");
    let mut resolver = Resolver::new(&facts, 8);
    let graph = Graph::build(&facts, &mut resolver, 8, 8);
    let drive = facts
        .fns
        .iter()
        .position(|fact| fact.name == "drive")
        .expect("drive");
    let modules: Vec<_> = graph
        .successors(Node::Fn(drive))
        .iter()
        .filter_map(|node| node.as_fn())
        .filter_map(|fid| facts.fns.get(fid))
        .filter(|fact| fact.name == "read")
        .map(|fact| fact.place.module.clone())
        .collect();

    assert_eq!(modules, vec![vec!["a".to_owned()]]);
}

#[test]
fn same_named_enum_variants_reach_only_their_own_handlers() {
    let sources = sources(&[
        (
            "lib",
            "pub mod a; pub mod b; pub fn make_a() -> a::State { a::State::Ready }",
        ),
        (
            "a",
            "pub enum State { Ready } pub fn handle(state: State) -> u64 { match state { State::Ready => apply() } } fn apply() -> u64 { 1 }",
        ),
        (
            "b",
            "pub enum State { Ready } pub fn handle(state: State) -> u64 { match state { State::Ready => apply() } } fn apply() -> u64 { 2 }",
        ),
    ]);
    let facts = facts::collect(&sources).expect("facts");
    let mut resolver = Resolver::new(&facts, 8);
    let graph = Graph::build(&facts, &mut resolver, 8, 8);
    let make_a = facts
        .fns
        .iter()
        .position(|fact| fact.name == "make_a")
        .expect("make_a");
    let modules: Vec<_> = facts.fns[make_a]
        .body
        .sites
        .iter()
        .enumerate()
        .flat_map(|(site, _)| graph.site_nodes(make_a, site))
        .filter_map(|node| match node {
            Node::Arm(fid, _) => facts.fns.get(fid),
            Node::Fn(_) => None,
        })
        .map(|fact| fact.place.module.clone())
        .collect();

    assert_eq!(modules, vec![vec!["a".to_owned()]]);
}

#[test]
fn field_types_use_the_declaration_import_scope() {
    let sources = sources(&[
        (
            "lib",
            "pub mod a; pub mod b; pub mod owner; use b::Foreign as Item; pub fn drive(store: &owner::Store, _: &Item) -> u64 { store.item.read() }",
        ),
        (
            "a",
            "pub struct Stored; impl Stored { pub(crate) fn read(&self) -> u64 { 1 } }",
        ),
        (
            "b",
            "pub struct Foreign; impl Foreign { pub(crate) fn read(&self) -> u64 { 2 } }",
        ),
        (
            "owner",
            "use crate::a::Stored as Item; pub struct Store { pub(crate) item: Item }",
        ),
    ]);
    let unreached = report(&sources, 0).coverage.unreached_private;

    assert!(!lists(&unreached, "Stored::read"), "{unreached:?}");
    assert!(lists(&unreached, "Foreign::read"), "{unreached:?}");
}

#[test]
fn vec_and_slice_projections_reach_the_element() {
    let unreached = unreached(
        r"
pub struct Segment;
impl Segment {
    fn vec_get(&self) {}
    fn vec_first(&self) {}
    fn slice_get(&self) {}
    fn slice_first(&self) {}
}
pub fn vec_get(items: &Vec<Segment>, index: usize) -> Option<()> {
    items.get(index)?.vec_get(); Some(())
}
pub fn vec_first(items: &Vec<Segment>) -> Option<()> {
    items.first()?.vec_first(); Some(())
}
pub fn slice_get(items: &[Segment], index: usize) -> Option<()> {
    items.get(index)?.slice_get(); Some(())
}
pub fn slice_first(items: &[Segment]) -> Option<()> {
    items.first()?.slice_first(); Some(())
}
",
    );

    for label in [
        "Segment::vec_get",
        "Segment::vec_first",
        "Segment::slice_get",
        "Segment::slice_first",
    ] {
        assert!(!lists(&unreached, label), "{label}: {unreached:?}");
    }
}

#[test]
fn option_projections_reach_the_inner_value() {
    let unreached = unreached(
        r"
pub struct State;
impl State {
    fn peek(&self) {}
    fn apply(&mut self) {}
}
pub fn peek(state: &Option<State>) -> Option<()> {
    state.as_ref()?.peek(); Some(())
}
pub fn apply(state: &mut Option<State>) -> Option<()> {
    state.as_mut()?.apply(); Some(())
}
",
    );

    for label in ["State::peek", "State::apply"] {
        assert!(!lists(&unreached, label), "{label}: {unreached:?}");
    }
}

#[test]
fn mutex_guard_projects_through_a_nested_option() {
    let unreached = unreached(
        r#"
use std::sync::Mutex;
pub struct State;
impl State { fn apply(&mut self) {} }
pub fn apply(state: &Mutex<Option<State>>) -> Option<()> {
    state.lock().expect("lock").as_mut()?.apply(); Some(())
}
"#,
    );

    assert!(!lists(&unreached, "State::apply"), "{unreached:?}");
}

#[test]
fn iterator_closure_receives_the_element() {
    let unreached = unreached(
        r"
pub struct Item;
impl Item { fn run(&self) {} }
pub fn drive(items: &[Item]) { items.iter().for_each(|item| item.run()); }
",
    );

    assert!(!lists(&unreached, "Item::run"), "{unreached:?}");
}

#[test]
fn occupied_map_entry_projects_the_value_without_the_key() {
    let unreached = unreached(
        r"
use std::collections::{HashMap, hash_map::Entry};
#[derive(Hash, PartialEq, Eq)]
pub struct Key;
impl Key { fn flush(&self) {} }
pub struct Value;
impl Value { fn flush(&mut self) {} }
pub fn drive(map: &mut HashMap<Key, Value>, key: Key) {
    if let Entry::Occupied(mut entry) = map.entry(key) { entry.get_mut().flush(); }
}
",
    );

    assert!(!lists(&unreached, "Value::flush"), "{unreached:?}");
    assert!(lists(&unreached, "Key::flush"), "{unreached:?}");
}

#[test]
fn vacant_map_entry_insert_returns_the_value_without_the_key() {
    let unreached = unreached(
        r"
use std::collections::{HashMap, hash_map::Entry};
#[derive(Hash, PartialEq, Eq)]
pub struct Key;
impl Key { fn flush(&self) {} }
pub struct Value;
impl Value { fn flush(&mut self) {} }
pub fn drive(map: &mut HashMap<Key, Value>, key: Key, value: Value) {
    if let Entry::Vacant(entry) = map.entry(key) { entry.insert(value).flush(); }
}
",
    );

    assert!(!lists(&unreached, "Value::flush"), "{unreached:?}");
    assert!(lists(&unreached, "Key::flush"), "{unreached:?}");
}

#[test]
fn custom_get_lock_as_mut_and_iter_use_declared_return_types() {
    let unreached = unreached(
        r"
pub struct ReturnedGet;
pub struct ReturnedLock;
pub struct ReturnedMut;
pub struct ReturnedIter;
impl ReturnedGet { fn run(&self) {} }
impl ReturnedLock { fn run(&self) {} }
impl ReturnedMut { fn run(&self) {} }
impl ReturnedIter { fn run(&self) {} }
pub struct Receiver;
impl Receiver {
    fn get(&self) -> ReturnedGet { ReturnedGet }
    fn lock(&self) -> ReturnedLock { ReturnedLock }
    fn as_mut(&mut self) -> ReturnedMut { ReturnedMut }
    fn iter(&self) -> std::iter::Once<ReturnedIter> { std::iter::once(ReturnedIter) }
    fn run(&self) {}
}
pub fn drive(receiver: &mut Receiver) {
    receiver.get().run();
    receiver.lock().run();
    receiver.as_mut().run();
    receiver.iter().for_each(|item| item.run());
}
",
    );

    for label in [
        "ReturnedGet::run",
        "ReturnedLock::run",
        "ReturnedMut::run",
        "ReturnedIter::run",
    ] {
        assert!(!lists(&unreached, label), "{label}: {unreached:?}");
    }
    assert!(lists(&unreached, "Receiver::run"), "{unreached:?}");
}

#[test]
fn custom_vec_and_option_do_not_project_their_generic_parameter() {
    let unreached = unreached(
        r"
pub struct Inner;
impl Inner { fn run(&self) {} }
pub struct ReturnedGet;
impl ReturnedGet { fn run(&self) {} }
pub struct ReturnedRef;
impl ReturnedRef { fn run(&self) {} }
pub struct Vec<T> { value: T }
impl<T> Vec<T> { fn get(&self, _: usize) -> ReturnedGet { ReturnedGet } }
pub struct Option<T> { value: T }
impl<T> Option<T> { fn as_ref(&self) -> ReturnedRef { ReturnedRef } }
pub fn drive(values: &Vec<Inner>, value: &Option<Inner>) {
    values.get(0).run(); value.as_ref().run();
}
",
    );

    assert!(!lists(&unreached, "ReturnedGet::run"), "{unreached:?}");
    assert!(!lists(&unreached, "ReturnedRef::run"), "{unreached:?}");
    assert!(lists(&unreached, "Inner::run"), "{unreached:?}");
}

#[test]
fn a_module_alias_reaches_only_the_aliased_function() {
    let sources = sources(&[
        (
            "lib",
            "pub mod portable; pub mod other; use portable as platform; pub fn drive() { platform::run(); }",
        ),
        ("portable", "pub(crate) fn run() {}"),
        ("other", "pub(crate) fn run() {}"),
    ]);
    let unreached = report(&sources, 0).coverage.unreached_private;

    assert!(
        !unreached
            .iter()
            .any(|entry| entry.contains("/portable.rs:")),
        "{unreached:?}"
    );
    assert!(
        unreached.iter().any(|entry| entry.contains("/other.rs:")),
        "{unreached:?}"
    );
}
