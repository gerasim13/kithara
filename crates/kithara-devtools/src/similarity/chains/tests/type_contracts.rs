use super::{consts, lists, unreached};
use crate::similarity::chains::{
    facts,
    graph::{Graph, Node},
    resolve::Resolver,
};

#[test]
fn derived_clone_preserves_the_owner_without_overriding_workspace_methods() {
    let unreached = unreached(
        r"
#[derive(Clone)]
pub struct Handle;
impl Handle { fn store(&self) {} }
pub struct Other;
impl Other { fn store(&self) {} }
pub struct Custom;
impl Custom { fn clone(&self) -> Other { Other } fn store(&self) {} }
#[derive(other::Clone)]
pub struct Foreign;
impl Foreign { fn store(&self) {} }
pub fn drive(handle: Handle, custom: Custom, foreign: Foreign) {
    handle.clone().store(); custom.clone().store(); foreign.clone().store();
}
",
    );
    assert!(!lists(&unreached, "Handle::store"), "{unreached:?}");
    assert!(!lists(&unreached, "Other::store"), "{unreached:?}");
    assert!(lists(&unreached, "Custom::store"), "{unreached:?}");
    assert!(lists(&unreached, "Foreign::store"), "{unreached:?}");
}

#[test]
fn declared_callback_inputs_preserve_the_receiver_and_argument_types() {
    let unreached = unreached(
        r"
pub struct Scanner;
pub struct Header;
pub struct Other;
impl Header { fn inspect(&self) {} }
impl Other { fn inspect(&self) {} }
impl Scanner {
    fn walk<F>(&mut self, end: usize, visit: F)
    where F: FnMut(&mut Self, Header) {}
    fn finish(&self) {}
    pub fn drive(&mut self) {
        self.walk(0, |this, header| { this.finish(); header.inspect(); });
    }
}
",
    );
    assert!(!lists(&unreached, "Scanner::finish"), "{unreached:?}");
    assert!(!lists(&unreached, "Header::inspect"), "{unreached:?}");
    assert!(lists(&unreached, "Other::inspect"), "{unreached:?}");
}

#[test]
fn inferred_constructor_parameters_do_not_hide_generic_methods() {
    let unreached = unreached(
        r"
pub struct Holder<T> { value: T }
pub struct Item;
impl<T> Holder<T> {
    fn open(value: T) -> Self { Self { value } }
    fn finish(&self) {}
}
impl Holder<Item> { fn specialized(&self) {} }
pub fn drive(holder: &Holder<dep::Unknown>) { Holder::open(1).finish(); holder.specialized(); }
",
    );
    assert!(!lists(&unreached, "Holder::open"), "{unreached:?}");
    assert!(!lists(&unreached, "Holder::finish"), "{unreached:?}");
    assert!(lists(&unreached, "Holder::specialized"), "{unreached:?}");
}

#[test]
fn option_chaining_binds_the_declared_payload() {
    let unreached = unreached(
        r"
pub struct Segment;
impl Segment { fn position(&self) -> Option<usize> { Some(0) } }
pub fn drive(segment: Option<&Segment>) {
    segment.and_then(|segment| segment.position());
}
",
    );
    assert!(!lists(&unreached, "Segment::position"), "{unreached:?}");
}

#[test]
fn map_lookup_preserves_the_value_type() {
    let unreached = unreached(
        r"
use std::collections::HashMap;
pub struct Decoder;
impl Decoder { fn configure(&self) {} }
pub fn drive(decoders: &mut HashMap<usize, Decoder>) {
    if let Some(decoder) = decoders.get_mut(&0) { decoder.configure(); }
}
",
    );
    assert!(!lists(&unreached, "Decoder::configure"), "{unreached:?}");
}

#[test]
fn cell_access_preserves_the_borrowed_or_stored_type() {
    let unreached = unreached(
        r"
use std::cell::{RefCell, OnceCell};
pub struct Picture;
pub struct Source;
impl Picture { fn hovered(&self) {} }
impl Source { fn picture(&self) {} }
pub fn drive(picture: &RefCell<Picture>, source: &OnceCell<Source>) {
    picture.borrow().hovered();
    if let Some(source) = source.get() { source.picture(); }
}
",
    );
    assert!(!lists(&unreached, "Picture::hovered"), "{unreached:?}");
    assert!(!lists(&unreached, "Source::picture"), "{unreached:?}");
}
#[test]
fn generic_fields_returns_and_aliased_deref_keep_the_actual_argument() {
    let unreached = unreached(
        r"
use std::ops::Deref as View;
pub struct Item;
impl Item { fn field(&self) {} fn returned(&self) {} fn dereferenced(&self) {} }
pub struct Holder<T> { item: T }
impl<T> Holder<T> { fn item(&self) -> &T { &self.item } }
impl<T> View for Holder<T> { type Target = T; fn deref(&self) -> &T { &self.item } }
pub type Alias<T> = Holder<T>;
pub fn drive(holder: &Alias<Item>) {
    holder.item.field(); holder.item().returned(); holder.dereferenced();
}
",
    );
    for label in ["Item::field", "Item::returned", "Item::dereferenced"] {
        assert!(!lists(&unreached, label), "{label}: {unreached:?}");
    }
}

#[test]
fn arc_clone_does_not_call_the_inner_clone_method() {
    let unreached = unreached(
        r"
use std::sync::Arc;
pub struct Item;
impl Item { fn clone(&self) -> Self { Item } fn read(&self) {} }
pub fn drive(item: &Arc<Item>) { item.clone().read(); }
",
    );
    assert!(!lists(&unreached, "Item::read"), "{unreached:?}");
    assert!(lists(&unreached, "Item::clone"), "{unreached:?}");
}

#[test]
fn qualified_trait_call_selects_the_trait_and_receiver_identity() {
    let source = r"
pub mod a {
    pub trait Read { fn read(&self) -> Item { Item } }
    pub struct Reader;
    impl Read for Reader {}
    impl Reader { fn read(&self) -> Item { Item } }
    pub struct Item;
    impl Item { pub(crate) fn finish(&self) {} }
}
pub mod b {
    pub trait Read { fn read(&self) -> Item { Item } }
    pub struct Reader;
    impl Read for Reader {}
    pub struct Item;
    impl Item { fn finish(&self) {} }
}
pub fn drive(reader: &a::Reader) { <a::Reader as a::Read>::read(reader).finish(); }
";
    let unreached = unreached(source);
    assert!(
        unreached
            .iter()
            .any(|entry| entry.ends_with("Reader::read")),
        "{unreached:?}"
    );
    let facts =
        facts::collect(&[(format!("{}/lib.rs", consts::ROOT), source.to_owned())]).expect("facts");
    let mut resolver = Resolver::new(&facts, 8);
    let graph = Graph::build(&facts, &mut resolver, 8, 8);
    let drive = facts
        .fns
        .iter()
        .position(|f| f.name == "drive")
        .expect("drive");
    let modules: Vec<_> = graph
        .successors(Node::Fn(drive))
        .iter()
        .filter_map(|node| node.as_fn())
        .filter_map(|fid| facts.fns.get(fid))
        .filter(|f| f.name == "finish")
        .map(|f| f.place.module.clone())
        .collect();
    assert_eq!(modules, vec![vec!["a".to_owned()]]);
}

#[test]
fn a_function_reference_is_not_a_value_of_its_owner() {
    let unreached = unreached(
        r"
pub struct Item;
impl Item { fn create() -> Self { Item } fn read(&self) {} }
pub fn drive() { let constructor = Item::create; constructor.read(); }
",
    );
    assert!(lists(&unreached, "Item::read"), "{unreached:?}");
}

#[test]
fn a_specialized_default_does_not_apply_to_other_type_arguments() {
    let unreached = unreached(
        r"
pub struct Left;
pub struct Right;
pub struct Item;
impl Item { fn finish(&self) {} }
pub struct Holder<T> { value: T }
pub trait Read { fn read(&self) -> Item { Item } }
impl Read for Holder<Left> {}
pub fn drive(holder: &Holder<Right>) { holder.read().finish(); }
",
    );
    assert!(lists(&unreached, "Item::finish"), "{unreached:?}");
}
