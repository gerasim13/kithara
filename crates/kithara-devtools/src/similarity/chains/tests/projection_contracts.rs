use super::{lists, unreached};

#[test]
fn range_get_and_index_do_not_promote_the_slice_to_its_element() {
    let unreached = unreached(
        r"
pub struct Item;
impl Item { fn from_get(&self) {} fn from_index(&self) {} fn scalar(&self) {} }
trait SliceRun { fn from_get(&self) {} fn from_index(&self) {} }
impl SliceRun for [Item] {}
pub fn drive(items: &Vec<Item>, index: usize) {
    items.get(0..1).unwrap().from_get();
    items[0..1].from_index();
    items[index].scalar();
}
",
    );
    assert!(lists(&unreached, "Item::from_get"), "{unreached:?}");
    assert!(lists(&unreached, "Item::from_index"), "{unreached:?}");
    assert!(!lists(&unreached, "Item::scalar"), "{unreached:?}");
}

#[test]
fn local_std_binding_shadows_the_extern_root_but_absolute_std_does_not() {
    let unreached = unreached(
        r"
use crate::custom as std;
pub struct Item;
impl Item { fn run(&self) {} fn builtin(&self) {} }
pub struct Returned;
impl Returned { fn run(&self) {} }
pub mod custom { pub mod vec {
    pub struct Vec<T> { value: T }
    impl<T> Vec<T> { pub(crate) fn get(&self, _: usize) -> crate::Returned { crate::Returned } }
} }
pub fn custom(value: &std::vec::Vec<Item>) { value.get(0).run(); }
pub fn builtin(value: &::std::vec::Vec<Item>) { value.get(0).unwrap().builtin(); }
",
    );
    assert!(!lists(&unreached, "Returned::run"), "{unreached:?}");
    assert!(!lists(&unreached, "Item::builtin"), "{unreached:?}");
    assert!(lists(&unreached, "Item::run"), "{unreached:?}");
}

#[test]
fn default_return_substitutes_the_impl_trait_argument() {
    let unreached = unreached(
        r"
pub struct Left;
pub struct Right;
impl Left { fn finish(&self) {} }
impl Right { fn finish(&self) {} }
pub struct Holder<T> { value: T }
pub trait Read<T> { fn make(&self) -> T; fn read(&self) -> T { self.make() } }
impl Read<Right> for Holder<Left> { fn make(&self) -> Right { Right } }
pub fn drive(holder: &Holder<Left>) { holder.read().finish(); }
",
    );
    assert!(lists(&unreached, "Left::finish"), "{unreached:?}");
    assert!(!lists(&unreached, "Right::finish"), "{unreached:?}");
}

#[test]
fn a_non_generic_receiver_keeps_its_default_trait_argument() {
    let unreached = unreached(
        r"
pub struct Item;
impl Item { fn finish(&self) {} }
pub struct Reader;
pub trait Read<T> { fn make(&self) -> T; fn read(&self) -> T { self.make() } }
impl Read<Item> for Reader { fn make(&self) -> Item { Item } }
pub fn drive(reader: &Reader) { reader.read().finish(); }
",
    );
    assert!(!lists(&unreached, "Item::finish"), "{unreached:?}");
}

#[test]
fn qualified_trait_arguments_select_only_the_matching_override() {
    let unreached = unreached(
        r"
pub struct Left;
pub struct Right;
impl Left { fn finish(&self) {} }
impl Right { fn finish(&self) {} }
pub struct Reader;
pub trait Read<T> { fn read(&self) -> T; }
impl Read<Left> for Reader { fn read(&self) -> Left { Left } }
impl Read<Right> for Reader { fn read(&self) -> Right { Right } }
pub fn drive(reader: &Reader) { <Reader as Read<Left>>::read(reader).finish(); }
",
    );
    assert!(!lists(&unreached, "Left::finish"), "{unreached:?}");
    assert!(lists(&unreached, "Right::finish"), "{unreached:?}");
}
