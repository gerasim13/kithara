use std::fs;

use super::{consts, lists};
use crate::similarity::chains::{ChainConfig, detect};

fn unreached(root: &str, source: &str) -> Vec<String> {
    let temp = tempfile::tempdir().expect("dependency directory");
    let path = temp.path().join("lib.rs");
    fs::write(&path, root).expect("dependency root");
    let config = ChainConfig {
        dependency_roots: [("synthetic_dep".to_owned(), path)].into_iter().collect(),
        ..ChainConfig::default()
    };
    let sources = [(format!("{}/lib.rs", consts::ROOT), source.to_owned())];
    detect(&sources, &config)
        .expect("chain report")
        .coverage
        .unreached_private
}

#[test]
fn dependency_result_alias_preserves_the_success_receiver() {
    let unreached = unreached(
        "pub type Result<T, E = Error> = core::result::Result<T, E>;",
        r"
use synthetic_dep::Result as Outcome;
pub struct Item;
impl Item { fn finish(&self) {} fn imported(&self) {} }
fn f() -> synthetic_dep::Result<Item> { loop {} }
fn g() -> Outcome<Item> { loop {} }
pub fn drive() -> Outcome<()> { f()?.finish(); g()?.imported(); loop {} }
",
    );
    assert!(!lists(&unreached, "Item::finish"), "{unreached:?}");
    assert!(!lists(&unreached, "Item::imported"), "{unreached:?}");
}

#[test]
fn dependency_alias_defaults_and_root_aliases_preserve_payloads() {
    let unreached = unreached(
        r"
pub type Result<T, E = Error> = core::result::Result<T, E>;
pub type Outcome<T, U = T> = crate::Result<U>;
",
        r"
pub struct Item;
impl Item { fn finish(&self) {} }
pub struct Other;
impl Other { fn finish(&self) {} fn explicit(&self) {} }
fn f() -> synthetic_dep::Outcome<Item> { loop {} }
fn g() -> synthetic_dep::Outcome<Item, Other> { loop {} }
pub fn drive() -> synthetic_dep::Result<()> { f()?.finish(); g()?.explicit(); loop {} }
",
    );
    assert!(!lists(&unreached, "Item::finish"), "{unreached:?}");
    assert!(lists(&unreached, "Other::finish"), "{unreached:?}");
    assert!(!lists(&unreached, "Other::explicit"), "{unreached:?}");
}

#[test]
fn dependency_structs_private_and_nested_aliases_stay_opaque() {
    for (root, ty) in [
        (
            "pub struct Result<T> { value: T } impl<T> Result<T> { pub fn into_inner(self) -> T { self.value } }",
            "Result",
        ),
        ("type Result<T> = core::result::Result<T, Error>;", "Result"),
        (
            "pub(crate) type Result<T> = core::result::Result<T, Error>;",
            "Result",
        ),
        (
            "pub mod nested { pub type Result<T> = core::result::Result<T, Error>; }",
            "nested::Result",
        ),
    ] {
        let unreached = unreached(
            root,
            &format!(
                r"
pub struct Item;
impl Item {{ fn finish(&self) {{}} }}
fn f() -> synthetic_dep::{ty}<Item> {{ loop {{}} }}
pub fn drive() {{ f()?.finish(); f().into_inner().finish(); }}
"
            ),
        );
        assert!(lists(&unreached, "Item::finish"), "{root}: {unreached:?}");
    }
}

#[test]
fn workspace_items_shadow_dependency_aliases() {
    let unreached = unreached(
        "pub type Result<T, E = Error> = core::result::Result<T, E>;",
        r"
pub struct Result;
impl Result { fn finish(&self) {} }
pub mod synthetic_dep {
    pub struct Result;
    impl Result { fn finish(&self) {} }
}
fn f() -> Result { Result }
pub fn drive() { f().finish(); synthetic_dep::Result.finish(); }
",
    );
    assert!(!lists(&unreached, "Result::finish"), "{unreached:?}");
    assert!(unreached.is_empty(), "{unreached:?}");
}

#[test]
fn unreadable_dependency_roots_do_not_fail_the_scan() {
    let temp = tempfile::tempdir().expect("dependency directory");
    let config = ChainConfig {
        dependency_roots: [("synthetic_dep".to_owned(), temp.path().join("missing.rs"))]
            .into_iter()
            .collect(),
        ..ChainConfig::default()
    };
    let sources = [(
        format!("{}/lib.rs", consts::ROOT),
        "pub fn drive() {}".to_owned(),
    )];
    assert!(detect(&sources, &config).is_ok());
}
