use std::collections::BTreeMap;

use super::{
    check::{fix_items, scan_items},
    safety,
};
use crate::{
    common::{fix::SourceRewriter, violation::Violation},
    style::config::StructFieldOrderConfig,
};

fn default_cfg() -> StructFieldOrderConfig {
    StructFieldOrderConfig {
        visibility_order: vec![
            "pub".to_string(),
            "pub(crate)".to_string(),
            "pub(super)".to_string(),
            "pub(in)".to_string(),
            "private".to_string(),
        ],
        exempt_attrs: vec!["repr".to_string()],
        exempt_derives: vec!["uniffi::Record".to_string()],
    }
}

pub(super) fn run_fix(src: &str) -> (String, Vec<String>) {
    let cfg = default_cfg();
    let file = syn::parse_file(src).unwrap_or_else(|e| panic!("parse failed: {e}\n---\n{src}"));
    let mut rw = SourceRewriter::new(src);
    let mut skipped = Vec::new();
    fix_items(
        &cfg,
        "fixture.rs",
        src,
        &file.items,
        safety::OrderSafety::new(safety::closed_file(&file), &file.attrs),
        &mut rw,
        &mut skipped,
    );
    let out = if rw.is_empty() {
        src.to_string()
    } else {
        rw.finish().expect("rewriter finish")
    };
    (out, skipped)
}

fn comment_multiset(src: &str) -> BTreeMap<String, usize> {
    let mut counts = BTreeMap::new();
    for line in src.lines() {
        let t = line.trim_start();
        if t.starts_with("//") {
            *counts.entry(t.trim_end().to_string()).or_insert(0) += 1;
        }
    }
    counts
}

#[test]
fn pub_before_private() {
    let src = "\
struct S {
private_field: u32,
pub public_field: u32,
}
";
    let (out, skipped) = run_fix(src);
    assert!(skipped.is_empty(), "skipped: {skipped:?}");
    let pub_pos = out.find("pub public_field").unwrap();
    let priv_pos = out.find("private_field").unwrap();
    assert!(pub_pos < priv_pos, "pub must precede private:\n{out}");
}

#[test]
fn already_ordered_is_no_op() {
    let src = "\
struct S {
pub a: u32,
private: u32,
}
";
    let (out, _) = run_fix(src);
    assert_eq!(out, src);
}

#[test]
fn repr_struct_is_skipped() {
    let src = "\
#[repr(C)]
struct Layout {
z: u32,
a: u32,
}
";
    let (out, _) = run_fix(src);
    assert_eq!(out, src, "repr layout must not change");
}

#[test]
fn a_derived_foreign_record_is_skipped() {
    let src = "\
#[derive(Clone, uniffi::Record)]
struct Direct {
z: u32,
a: u32,
}

#[cfg_attr(feature = \"uniffi\", derive(uniffi::Record))]
struct Gated {
z: u32,
a: u32,
}
";
    let (out, _) = run_fix(src);
    assert_eq!(out, src, "a foreign binding's field order is its contract");
}

#[test]
fn opaque_builder_role_order_is_not_rewritten() {
    let src = "\
struct S {
#[builder(default)]
a: u32,
#[builder(finish_fn)]
z: u32,
#[builder(field)]
y: u32,
#[builder(start_fn)]
x: u32,
}
";
    let (out, skipped) = run_fix(src);
    assert_eq!(out, src);
    assert!(skipped.iter().any(|reason| reason.contains("attributes")));
}

/// `#[builder(start_fn)]` fields are the starting function's parameters, so
/// their declaration order is the call order every caller already wrote.
#[test]
fn positional_builder_fields_keep_their_declared_order() {
    let src = "\
struct S {
#[builder(start_fn)]
worker: Worker,
#[builder(start_fn)]
pools: PoolRegion,
#[builder(finish_fn)]
zone: Zone,
#[builder(finish_fn)]
area: Area,
}
";
    let (out, skipped) = run_fix(src);
    assert!(skipped.is_empty(), "skipped: {skipped:?}");
    assert_eq!(out, src, "a positional signature must survive the fix");
}

#[test]
fn doc_comments_travel_with_field() {
    let src = "\
struct S {
/// docs for z
z: u32,
/// docs for a
a: u32,
}
";
    let (out, skipped) = run_fix(src);
    assert!(skipped.is_empty(), "skipped: {skipped:?}");
    assert_eq!(comment_multiset(src), comment_multiset(&out));
    let a_doc = out.find("/// docs for a").unwrap();
    let a_field = out.find("a: u32,").unwrap();
    let z_doc = out.find("/// docs for z").unwrap();
    let z_field = out.find("z: u32,").unwrap();
    assert!(
        a_doc < a_field && a_field < z_doc && z_doc < z_field,
        "docs must precede their fields:\n{out}"
    );
}

#[test]
fn heterogeneous_cfg_is_skipped() {
    let src = "\
struct S {
z: u32,
#[cfg(feature = \"x\")]
a: u32,
}
";
    let (out, skipped) = run_fix(src);
    assert_eq!(out, src);
    assert!(
        skipped.iter().any(|s| s.contains("cfg")),
        "skipped: {skipped:?}"
    );
}

#[test]
fn missing_trailing_comma_is_skipped() {
    let src = "\
struct S {
z: u32,
a: u32
}
";
    let (out, skipped) = run_fix(src);
    assert_eq!(out, src);
    assert!(
        skipped.iter().any(|s| s.contains("trailing comma")),
        "skipped: {skipped:?}"
    );
}

#[test]
fn idempotent_run() {
    let src = "\
struct S {
z: u32,
a: u32,
pub p: u32,
}
";
    let (after_first, _) = run_fix(src);
    let (after_second, _) = run_fix(&after_first);
    assert_eq!(after_first, after_second, "I2: idempotency violated");
}

#[test]
fn floating_comment_skipped() {
    let src = "\
struct S {
z: u32,

// floating

a: u32,
}
";
    let (out, skipped) = run_fix(src);
    assert_eq!(out, src, "must not modify");
    assert!(
        skipped.iter().any(|s| s.contains("floating")),
        "skipped: {skipped:?}"
    );
}

/// Run the detection scan over a snippet and return the violation keys.
pub(super) fn detect(src: &str) -> Vec<String> {
    violations(src)
        .into_iter()
        .map(|violation| violation.key)
        .collect()
}

pub(super) fn detect_messages(src: &str) -> Vec<String> {
    violations(src)
        .into_iter()
        .map(|violation| violation.message)
        .collect()
}

fn violations(src: &str) -> Vec<Violation> {
    let cfg = default_cfg();
    let file = syn::parse_file(src).unwrap_or_else(|e| panic!("parse failed: {e}\n---\n{src}"));
    let mut out = Vec::new();
    scan_items(
        &cfg,
        "fixture.rs",
        &file.items,
        safety::OrderSafety::new(safety::closed_file(&file), &file.attrs),
        &mut Vec::new(),
        &mut out,
    );
    out
}

#[test]
fn out_of_order_visibility_is_flagged() {
    let src = "\
struct S {
private_field: u32,
pub public_field: u32,
}
";
    assert_eq!(detect(src).len(), 1, "homogeneous struct must still fire");
}

#[test]
fn heterogeneous_cfg_is_not_flagged() {
    let src = "\
struct S {
z: u32,
#[cfg(feature = \"x\")]
a: u32,
}
";
    assert!(
        detect(src).is_empty(),
        "reordering across a `#[cfg]` boundary is unsafe — must not flag"
    );
}

#[test]
fn homogeneous_cfg_still_flagged() {
    let src = "\
struct S {
#[cfg(test)]
z: u32,
#[cfg(test)]
a: u32,
}
";
    assert_eq!(
        detect(src).len(),
        1,
        "uniform `#[cfg(test)]` on every field is safe to reorder — must still flag"
    );
}

/// The starting and finishing functions take their parameters in
/// declaration order, so a caller reads that order, not a sorted one.
#[test]
fn a_positional_builder_signature_is_accepted_in_its_declared_order() {
    let src = "\
struct S {
#[builder(start_fn)]
worker: Worker,
#[builder(start_fn)]
pools: PoolRegion,
#[builder(finish_fn)]
zone: Zone,
#[builder(finish_fn)]
area: Area,
}
";
    assert!(detect(src).is_empty(), "{:?}", detect(src));
}

#[test]
fn bon_role_order_is_accepted() {
    let src = "\
struct S {
#[builder(start_fn)]
z: u32,
#[builder(default)]
a: u32,
}
";
    assert!(detect(src).is_empty());
}
