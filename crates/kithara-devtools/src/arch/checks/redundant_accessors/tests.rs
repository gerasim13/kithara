use std::fs;

use cargo_metadata::MetadataCommand;

use super::RedundantAccessors;
use crate::{
    arch::{
        checks::{Check, Context},
        config::{ArchConfig, RedundantAccessorsThreshold},
    },
    common::{
        scope::Scope,
        violation::{Severity, Violation},
    },
};

fn run_sources(sources: &[(&str, &str)], threshold: RedundantAccessorsThreshold) -> Vec<Violation> {
    let dir = tempfile::tempdir().expect("temporary workspace");
    let mut members = std::collections::BTreeSet::new();
    for &(rel, source) in sources {
        let member = rel.split('/').nth(1).expect("fixture package directory");
        members.insert(member);
        let path = dir.path().join(rel);
        fs::create_dir_all(path.parent().expect("source directory"))
            .expect("create fixture source directory");
        fs::write(path, source).expect("write source fixture");
    }
    let member_paths: Vec<_> = members
        .iter()
        .map(|member| format!("\"crates/{member}\""))
        .collect();
    fs::write(
        dir.path().join("Cargo.toml"),
        format!(
            "[workspace]\nmembers = [{}]\nresolver = \"2\"\n",
            member_paths.join(", ")
        ),
    )
    .expect("write workspace manifest");
    for member in members {
        fs::write(
            dir.path().join("crates").join(member).join("Cargo.toml"),
            format!("[package]\nname = \"{member}\"\nversion = \"0.0.0\"\nedition = \"2024\"\n"),
        )
        .expect("write package manifest");
    }
    let metadata = MetadataCommand::new()
        .manifest_path(dir.path().join("Cargo.toml"))
        .no_deps()
        .exec()
        .expect("fixture cargo metadata");
    let mut config = ArchConfig::default();
    config.thresholds.redundant_accessors = threshold;
    let scope = Scope::default();
    let ctx = Context::new(&config, &metadata, dir.path(), &scope);
    RedundantAccessors.run(&ctx).expect("run accessor check")
}

fn only_pattern(pattern: u8) -> RedundantAccessorsThreshold {
    RedundantAccessorsThreshold {
        detect_field_passthrough: pattern == 1,
        detect_nested_shorthand: pattern == 2,
        detect_mutation_handle: pattern == 3,
        detect_delegate_passthrough: pattern == 4,
        ..RedundantAccessorsThreshold::default()
    }
}

fn run_p4(sources: &[(&str, &str)]) -> Vec<Violation> {
    run_sources(sources, only_pattern(4))
}

#[test]
fn p4_flags_accessor_paired_with_delegate_same_file() {
    let v = run_p4(&[(
        "crates/x/src/lib.rs",
        r#"
        use std::sync::Arc;
        pub struct Q { player: Arc<P> }
        impl Q {
            pub fn player(&self) -> &Arc<P> { &self.player }
            delegate::delegate! {
                to self.player {
                    pub fn play(&self);
                    pub fn pause(&self);
                }
            }
        }
    "#,
    )]);
    assert_eq!(v.len(), 1, "expected exactly one P4 violation, got {v:?}");
    assert_eq!(v[0].severity, Severity::Warn);
    assert!(
        v[0].message.contains("P4:") && v[0].message.contains("`player`"),
        "unexpected message: {}",
        v[0].message
    );
}

#[test]
fn p4_flags_accessor_paired_with_delegate_cross_file() {
    let v = run_p4(&[
        ("crates/x/src/lib.rs", "mod access; mod passthrough;"),
        (
            "crates/x/src/access.rs",
            r#"
            use std::sync::Arc;
            pub struct P;
            impl P { pub fn play(&self) {} }
            pub struct Q { pub(super) player: Arc<P> }
            impl Q {
                pub fn player(&self) -> &Arc<P> { &self.player }
            }
        "#,
        ),
        (
            "crates/x/src/passthrough.rs",
            r#"
            use crate::access::Q;
            impl Q {
                delegate::delegate! {
                    to self.player {
                        pub fn play(&self);
                    }
                }
            }
        "#,
        ),
    ]);
    assert_eq!(v.len(), 1, "expected one cross-file P4, got {v:?}");
    assert!(
        v[0].message
            .contains("delegate lives in `crates/x/src/passthrough.rs`"),
        "diagnostic should pinpoint the delegate file: {}",
        v[0].message
    );
    assert!(
        v[0].key.contains("crates/x/src/access.rs"),
        "violation key should anchor to the accessor's file: {}",
        v[0].key
    );
}

#[test]
fn p4_silent_when_only_delegate() {
    assert!(
        run_p4(&[(
            "crates/x/src/lib.rs",
            r#"
                pub struct Q { player: u32 }
                impl Q {
                    delegate::delegate! {
                        to self.player {
                            pub fn play(&self);
                        }
                    }
                }
            "#,
        )])
        .is_empty()
    );
}

#[test]
fn p4_silent_when_only_accessor() {
    assert!(
        run_p4(&[(
            "crates/x/src/lib.rs",
            r#"
                use std::sync::Arc;
                pub struct Q { player: Arc<P> }
                impl Q {
                    pub fn player(&self) -> &Arc<P> { &self.player }
                }
            "#,
        )])
        .is_empty()
    );
}

#[test]
fn p4_flags_only_matching_target_in_multi_target_delegate() {
    let v = run_p4(&[(
        "crates/x/src/lib.rs",
        r#"
            use std::sync::Arc;
            pub struct Q { a: Arc<A>, b: Arc<B> }
            impl Q {
                pub fn a(&self) -> &Arc<A> { &self.a }
                delegate::delegate! {
                    to self.a {
                        pub fn alpha(&self);
                    }
                    to self.b {
                        pub fn beta(&self);
                    }
                }
            }
        "#,
    )]);
    assert_eq!(v.len(), 1, "expected one P4 (for `a` only), got {v:?}");
    assert!(v[0].message.contains("`a`"));
}

#[test]
fn p4_silent_for_chained_delegate_target() {
    let v = run_p4(&[(
        "crates/x/src/lib.rs",
        r#"
            use std::sync::Arc;
            pub struct Q { inner: Arc<I> }
            impl Q {
                pub fn inner(&self) -> &Arc<I> { &self.inner }
                delegate::delegate! {
                    to self.inner.player {
                        pub fn play(&self);
                    }
                }
            }
        "#,
    )]);
    assert!(
        v.is_empty(),
        "chained `to self.inner.player` should not pair with `inner` accessor: {v:?}"
    );
}

#[test]
fn p4_silent_for_non_delegate_macro_with_to_self_tokens() {
    let v = run_p4(&[(
        "crates/x/src/lib.rs",
        r#"
            pub struct Q { x: u32 }
            impl Q {
                pub fn x(&self) -> &u32 { &self.x }
                some_other_macro! { to self.x { } }
            }
        "#,
    )]);
    assert!(
        v.is_empty(),
        "non-delegate macro must not produce P4: {v:?}"
    );
}
fn assert_separate_owners(pattern: u8, external: bool, left: &str, right: &str) {
    let inline = format!("mod left {{ {left} }} mod right {{ {right} }}");
    let sources = if external {
        vec![
            ("crates/x/src/lib.rs", "mod left; mod right;"),
            ("crates/x/src/left.rs", left),
            ("crates/x/src/right.rs", right),
        ]
    } else {
        vec![("crates/x/src/lib.rs", inline.as_str())]
    };
    let violations = run_sources(&sources, only_pattern(pattern));
    assert!(
        violations.is_empty(),
        "distinct owners were paired: {violations:?}"
    );
}

const P1_LEFT: &str = "pub struct Meter { pub value: u32 }";
const P1_RIGHT: &str =
    "pub struct Meter { value: u32 } impl Meter { pub fn value(&self) -> u32 { self.value } }";
const P2_LEFT: &str = "struct Inner { value: u32 } struct Meter { inner: Inner } impl Meter { pub fn inner(&self) -> &Inner { &self.inner } }";
const P2_RIGHT: &str = "struct Inner { value: u32 } struct Meter { inner: Inner } impl Meter { pub fn value(&self) -> u32 { self.inner.value } }";
const P3_LEFT: &str = "use std::sync::atomic::AtomicU32; struct Meter { value: AtomicU32 } impl Meter { pub fn handle(&self) -> &AtomicU32 { &self.value } }";
const P3_RIGHT: &str = "use std::sync::atomic::{AtomicU32, Ordering}; struct Meter { value: AtomicU32 } impl Meter { pub fn set(&self, value: u32) { self.value.store(value, Ordering::Relaxed); } }";
const P4_LEFT: &str = "struct Player; struct Meter { player: Player } impl Meter { pub fn player(&self) -> &Player { &self.player } }";
const P4_RIGHT: &str = "struct Player; struct Meter { player: Player } impl Meter { delegate::delegate! { to self.player { pub fn play(&self); } } }";

#[test]
fn p1_keeps_same_named_workspace_packages_separate() {
    let violations = run_sources(
        &[
            ("crates/public/src/lib.rs", P1_LEFT),
            ("crates/private/src/lib.rs", P1_RIGHT),
        ],
        only_pattern(1),
    );
    assert!(
        violations.is_empty(),
        "cross-crate fields leaked: {violations:?}"
    );
}

#[test]
fn p1_keeps_same_named_inline_modules_separate() {
    assert_separate_owners(1, false, P1_LEFT, P1_RIGHT);
}

#[test]
fn p1_keeps_same_named_external_modules_separate() {
    assert_separate_owners(1, true, P1_LEFT, P1_RIGHT);
}

#[test]
fn p2_keeps_same_named_inline_modules_separate() {
    assert_separate_owners(2, false, P2_LEFT, P2_RIGHT);
}

#[test]
fn p2_keeps_same_named_external_modules_separate() {
    assert_separate_owners(2, true, P2_LEFT, P2_RIGHT);
}

#[test]
fn p3_keeps_same_named_inline_modules_separate() {
    assert_separate_owners(3, false, P3_LEFT, P3_RIGHT);
}

#[test]
fn p3_keeps_same_named_external_modules_separate() {
    assert_separate_owners(3, true, P3_LEFT, P3_RIGHT);
}

#[test]
fn p4_keeps_same_named_inline_modules_separate() {
    assert_separate_owners(4, false, P4_LEFT, P4_RIGHT);
}

#[test]
fn p4_keeps_same_named_external_modules_separate() {
    assert_separate_owners(4, true, P4_LEFT, P4_RIGHT);
}

#[test]
fn p1_retains_true_public_field_accessor() {
    let violations = run_sources(
        &[(
            "crates/x/src/lib.rs",
            "pub struct Meter { pub value: u32 } impl Meter { pub fn value(&self) -> u32 { self.value } }",
        )],
        only_pattern(1),
    );
    assert_eq!(
        violations.len(),
        1,
        "true owner pairing lost: {violations:?}"
    );
    assert_eq!(violations[0].key, "crates/x/src/lib.rs::Meter::value");
    assert_eq!(violations[0].severity, Severity::Warn);
}

#[test]
fn p1_resolves_qualified_impl_owner() {
    let violations = run_sources(
        &[
            ("crates/x/src/lib.rs", "mod model; mod access;"),
            (
                "crates/x/src/model.rs",
                "pub struct Meter { pub value: u32 }",
            ),
            (
                "crates/x/src/access.rs",
                "impl crate::model::Meter { pub fn value(&self) -> u32 { self.value } }",
            ),
        ],
        only_pattern(1),
    );
    assert_eq!(
        violations.len(),
        1,
        "qualified owner pairing lost: {violations:?}"
    );
    assert_eq!(violations[0].key, "crates/x/src/access.rs::Meter::value");
}

#[test]
fn p4_resolves_explicit_import_alias_across_impl_files() {
    let violations = run_p4(&[
        (
            "crates/x/src/lib.rs",
            "mod model; mod access; mod forwarding;",
        ),
        (
            "crates/x/src/model.rs",
            "pub struct Player; pub struct Meter { pub(crate) player: Player }",
        ),
        (
            "crates/x/src/access.rs",
            "use crate::model::{Meter as LocalMeter, Player}; impl LocalMeter { pub fn player(&self) -> &Player { &self.player } }",
        ),
        (
            "crates/x/src/forwarding.rs",
            "impl crate::model::Meter { delegate::delegate! { to self.player { pub fn play(&self); } } }",
        ),
    ]);
    assert_eq!(
        violations.len(),
        1,
        "aliased owner pairing lost: {violations:?}"
    );
    assert_eq!(violations[0].key, "crates/x/src/access.rs::Meter::player");
    assert!(
        violations[0]
            .message
            .contains("delegate lives in `crates/x/src/forwarding.rs`")
    );
}

#[test]
fn p1_refuses_unknown_impl_owner() {
    let violations = run_sources(
        &[(
            "crates/x/src/lib.rs",
            "pub struct Meter { pub value: u32 } impl external::Meter { pub fn value(&self) -> u32 { self.value } }",
        )],
        only_pattern(1),
    );
    assert!(
        violations.is_empty(),
        "unknown owner was guessed: {violations:?}"
    );
}

#[test]
fn p1_refuses_conflicting_glob_imports() {
    let violations = run_sources(
        &[(
            "crates/x/src/lib.rs",
            "mod left { pub struct Meter { pub value: u32 } } mod right { pub struct Meter { pub value: u32 } } use left::*; use right::*; impl Meter { pub fn value(&self) -> u32 { self.value } }",
        )],
        only_pattern(1),
    );
    assert!(
        violations.is_empty(),
        "ambiguous glob owner was guessed: {violations:?}"
    );
}

#[test]
fn p1_refuses_duplicate_conditional_declarations() {
    let violations = run_sources(
        &[(
            "crates/x/src/lib.rs",
            "#[cfg(feature = \"alternate\")] pub struct Meter { pub value: u32 } #[cfg(not(feature = \"alternate\"))] pub struct Meter { value: u32 } impl Meter { pub fn value(&self) -> u32 { self.value } }",
        )],
        only_pattern(1),
    );
    assert!(
        violations.is_empty(),
        "conditional owner was guessed: {violations:?}"
    );
}

#[test]
fn p1_refuses_generic_parameter_shadowing_named_owner() {
    let violations = run_sources(
        &[(
            "crates/x/src/lib.rs",
            "pub struct Meter { pub value: u32 } impl<Meter> Meter { pub fn value(&self) -> u32 { self.value } }",
        )],
        only_pattern(1),
    );
    assert!(
        violations.is_empty(),
        "generic parameter was treated as declared owner: {violations:?}"
    );
}

#[test]
fn p1_refuses_impl_with_unknown_attribute() {
    let violations = run_sources(
        &[(
            "crates/x/src/lib.rs",
            "pub struct Meter { pub value: u32 } #[rewrite] impl Meter { pub fn value(&self) -> u32 { self.value } }",
        )],
        only_pattern(1),
    );
    assert!(
        violations.is_empty(),
        "unknown impl attribute was treated as a proven owner: {violations:?}"
    );
}
