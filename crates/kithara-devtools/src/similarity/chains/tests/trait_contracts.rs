use super::{lists, unreached};

#[test]
fn imported_dependency_traits_keep_their_qualified_identity() {
    let source = r"
use dependency::visit::Visit as First;
use dependency::visit::Visit as Second;
use other::visit::Visit as Other;
use unrelated::*;
pub struct Left;
pub struct Right;
pub struct Different;
impl First for Left { fn visit(&self) {} }
impl Second for Right { fn visit(&self) {} }
impl Other for Different { fn visit(&self) {} }
";
    let sources = [(format!("{}/lib.rs", super::consts::ROOT), source.to_owned())];
    let facts = crate::similarity::chains::facts::collect(&sources).expect("facts");
    let resolver = crate::similarity::chains::resolve::Resolver::new(&facts, 8);
    let traits: Vec<_> = facts
        .fns
        .iter()
        .enumerate()
        .filter(|(_, fact)| fact.name == "visit")
        .map(|(fid, _)| resolver.trait_keys(fid))
        .collect();

    assert_eq!(traits.len(), 3);
    assert_eq!(traits[0], &[vec!["dependency", "visit", "Visit"]]);
    assert_eq!(traits[0], traits[1]);
    assert_ne!(traits[0], traits[2]);
}

#[test]
fn typed_trait_objects_do_not_dispatch_to_other_trait_arguments() {
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
pub fn drive(reader: &dyn Read<Left>) { reader.read().finish(); }
",
    );
    assert!(!lists(&unreached, "Left::finish"), "{unreached:?}");
    assert!(lists(&unreached, "Right::finish"), "{unreached:?}");
}

#[test]
fn an_override_of_one_trait_instance_does_not_hide_another_default() {
    let unreached = unreached(
        r"
pub struct Left;
pub struct Right;
impl Left { fn finish(&self) {} }
impl Right { fn finish(&self) {} }
pub struct Reader;
pub trait Read<T> { fn make(&self) -> T; fn read(&self) -> T { self.make() } }
impl Read<Left> for Reader {
    fn make(&self) -> Left { Left }
    fn read(&self) -> Left { Left }
}
impl Read<Right> for Reader { fn make(&self) -> Right { Right } }
pub fn drive(reader: &dyn Read<Right>) { reader.read().finish(); }
",
    );
    assert!(lists(&unreached, "Left::finish"), "{unreached:?}");
    assert!(!lists(&unreached, "Right::finish"), "{unreached:?}");
}

#[test]
fn a_qualified_dynamic_call_keeps_its_instantiated_trait_default() {
    let unreached = unreached(
        r"
pub struct Left;
pub struct Right;
impl Left { fn finish(&self) {} }
impl Right { fn finish(&self) {} }
pub struct Reader;
pub trait Read<T> { fn make(&self) -> T; fn read(&self) -> T { self.make() } }
impl Read<Left> for Reader {
    fn make(&self) -> Left { Left }
    fn read(&self) -> Left { Left }
}
impl Read<Right> for Reader { fn make(&self) -> Right { Right } }
pub fn drive(reader: &dyn Read<Right>) {
    <dyn Read<Right> as Read<Right>>::read(reader).finish();
}
",
    );
    assert!(lists(&unreached, "Left::finish"), "{unreached:?}");
    assert!(!lists(&unreached, "Right::finish"), "{unreached:?}");
}

#[test]
fn trait_return_parameters_match_the_trait_position() {
    let unreached = unreached(
        r"
pub struct Left;
pub struct Right;
impl Left { fn finish(&self) {} }
impl Right { fn finish(&self) {} }
pub struct Holder<A, B> { left: A, right: B }
pub trait Read<A, B> { fn read(&self) -> B; }
impl<A, B> Read<B, A> for Holder<A, B> { fn read(&self) -> A { panic!() } }
pub fn drive(reader: &dyn Read<Left, Right>) { reader.read().finish(); }
",
    );
    assert!(lists(&unreached, "Left::finish"), "{unreached:?}");
    assert!(!lists(&unreached, "Right::finish"), "{unreached:?}");
}
