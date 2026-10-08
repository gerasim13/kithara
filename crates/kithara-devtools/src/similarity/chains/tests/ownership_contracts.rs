use super::{consts, lists, unreached};
use crate::similarity::chains::{
    facts,
    graph::{Graph, Node},
    resolve::Resolver,
};

#[test]
fn reordered_impl_alias_arguments_bind_the_nominal_receiver() {
    let unreached = unreached(
        r"
pub struct Left;
pub struct Right;
impl Left { fn finish(&self) {} }
impl Right { fn finish(&self) {} }
pub struct Holder<A, B> { first: A, second: B }
pub type Flip<T, U> = Holder<U, T>;
impl<T, U> Flip<T, U> { fn read(&self) -> &T { &self.second } }
pub fn drive(value: &Holder<Left, Right>) { value.read().finish(); }
",
    );
    assert!(lists(&unreached, "Left::finish"), "{unreached:?}");
    assert!(!lists(&unreached, "Right::finish"), "{unreached:?}");
}

#[test]
fn a_closed_impl_alias_does_not_override_other_receiver_arguments() {
    let unreached = unreached(
        r"
pub struct Left;
pub struct Right;
impl Left { fn finish(&self) {} }
impl Right { fn finish(&self) {} }
pub struct Holder<T> { value: T }
pub type LeftHolder = Holder<Left>;
impl LeftHolder { fn read(&self) -> &Left { &self.value } }
trait Read { fn read(&self) -> &Right; }
impl Read for Holder<Right> { fn read(&self) -> &Right { &self.value } }
pub fn drive(value: &Holder<Right>) { value.read().finish(); }
",
    );
    assert!(lists(&unreached, "Left::finish"), "{unreached:?}");
    assert!(lists(&unreached, "LeftHolder::read"), "{unreached:?}");
    assert!(!lists(&unreached, "Right::finish"), "{unreached:?}");
}

#[test]
fn an_unresolved_closed_alias_does_not_override_a_generic_trait() {
    let unreached = unreached(
        r"
pub struct Left;
pub struct Right;
impl Left { fn finish(&self) {} }
impl Right { fn finish(&self) {} }
pub struct Holder<T> { value: T }
pub type Closed = Holder<dep::A>;
impl Closed { fn read(&self) -> Right { Right } }
pub trait Read { fn read(&self) -> Left; }
impl<T> Read for Holder<T> { fn read(&self) -> Left { Left } }
pub fn drive(value: &Holder<dep::B>) { value.read().finish(); }
",
    );
    assert!(lists(&unreached, "Closed::read"), "{unreached:?}");
    assert!(lists(&unreached, "Right::finish"), "{unreached:?}");
    assert!(!lists(&unreached, "Holder::read"), "{unreached:?}");
    assert!(!lists(&unreached, "Left::finish"), "{unreached:?}");
}

#[test]
fn owner_alias_cfg_alternatives_keep_both_parameter_mappings() {
    let unreached = unreached(
        r#"
pub struct Left;
pub struct Right;
impl Left { fn finish(&self) {} }
impl Right { fn finish(&self) {} }
pub struct Holder<A, B> { first: A, second: B }
#[cfg(feature = "flipped")]
pub type View<T, U> = Holder<U, T>;
#[cfg(not(feature = "flipped"))]
pub type View<T, U> = Holder<T, U>;
impl<T, U> View<T, U> {
    fn read(&self) -> &T {
        #[cfg(feature = "flipped")]
        { &self.second }
        #[cfg(not(feature = "flipped"))]
        { &self.first }
    }
}
pub fn drive(value: &Holder<Left, Right>) { value.read().finish(); }
"#,
    );
    assert!(!lists(&unreached, "Left::finish"), "{unreached:?}");
    assert!(!lists(&unreached, "Right::finish"), "{unreached:?}");
}

#[test]
fn an_aliased_deref_substitutes_the_reordered_target() {
    let unreached = unreached(
        r"
pub struct Left;
pub struct Right;
impl Left { fn finish(&self) {} }
impl Right { fn finish(&self) {} }
pub struct Holder<A, B> { first: A, second: B }
pub type Flip<T, U> = Holder<U, T>;
impl<T, U> std::ops::Deref for Flip<T, U> {
    type Target = T;
    fn deref(&self) -> &T { &self.second }
}
pub fn drive(value: &Holder<Left, Right>) { value.finish(); }
",
    );
    assert!(lists(&unreached, "Left::finish"), "{unreached:?}");
    assert!(!lists(&unreached, "Right::finish"), "{unreached:?}");
}

#[test]
fn an_aliased_trait_impl_substitutes_its_default_return() {
    let unreached = unreached(
        r"
pub struct Left;
pub struct Right;
impl Left { fn finish(&self) {} }
impl Right { fn finish(&self) {} }
pub struct Holder<A, B> { first: A, second: B }
pub type Flip<T, U> = Holder<U, T>;
pub trait Read<T> { fn make(&self) -> &T; fn read(&self) -> &T { self.make() } }
impl<T, U> Read<T> for Flip<T, U> { fn make(&self) -> &T { &self.second } }
pub fn drive(value: &Holder<Left, Right>) { value.read().finish(); }
",
    );
    assert!(lists(&unreached, "Left::finish"), "{unreached:?}");
    assert!(!lists(&unreached, "Right::finish"), "{unreached:?}");
}

#[test]
fn every_call_form_honors_the_same_dynamic_target_bound() {
    let source = r"
pub trait Run { fn run(&self); }
pub struct A;
pub struct B;
pub struct C;
impl Run for A { fn run(&self) {} }
impl Run for B { fn run(&self) {} }
impl Run for C { fn run(&self) {} }
pub fn dot(value: &dyn Run) { value.run(); }
pub fn associated(value: &dyn Run) { Run::run(value); }
pub fn reference(value: &dyn Run) { let invoke: fn(&dyn Run) = Run::run; invoke(value); }
pub fn qself(value: &dyn Run) { <dyn Run>::run(value); }
pub fn qualified(value: &dyn Run) { <dyn Run as Run>::run(value); }
";
    let facts =
        facts::collect(&[(format!("{}/lib.rs", consts::ROOT), source.to_owned())]).expect("facts");
    for (limit, expected) in [(3, 3), (2, 0)] {
        let mut resolver = Resolver::new(&facts, limit);
        let graph = Graph::build(&facts, &mut resolver, 8, 8);
        for name in ["dot", "associated", "reference", "qself", "qualified"] {
            let fid = facts
                .fns
                .iter()
                .position(|f| f.name == name)
                .expect("caller");
            let targets: Vec<_> = graph
                .successors(Node::Fn(fid))
                .iter()
                .filter_map(|node| node.as_fn())
                .filter_map(|fid| facts.fns.get(fid))
                .filter(|f| f.name == "run")
                .collect();
            assert_eq!(targets.len(), expected, "{name}: limit={limit}");
        }
    }
}

#[test]
fn a_workspace_vec_callback_does_not_inherit_an_iterator_input() {
    let unreached = unreached(
        r"
pub struct Element;
pub struct CallbackInput;
impl Element { fn finish(&self) {} fn standard(&self) {} }
impl CallbackInput { fn finish(&self) {} }
pub trait Run { fn for_each<F: FnMut(CallbackInput)>(&self, callback: F); }
impl Run for Vec<Element> {
    fn for_each<F: FnMut(CallbackInput)>(&self, mut callback: F) { callback(CallbackInput); }
}
pub fn custom(value: &Vec<Element>) { value.for_each(|input| input.finish()); }
pub fn standard(value: &[Element]) { value.iter().for_each(|input| input.standard()); }
",
    );
    assert!(lists(&unreached, "Element::finish"), "{unreached:?}");
    assert!(!lists(&unreached, "CallbackInput::finish"), "{unreached:?}");
    assert!(!lists(&unreached, "Vec::for_each"), "{unreached:?}");
    assert!(!lists(&unreached, "Element::standard"), "{unreached:?}");
}
