use super::{consts, lists, unreached};
use crate::similarity::chains::{
    facts,
    graph::{Graph, Node},
    resolve::Resolver,
};

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
