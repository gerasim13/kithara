//! Behaviour of the code `#[derive(Ranged)]` emits.

use kithara_derive::Ranged;

/// Asymmetric on purpose: the two ends have to be read separately.
#[derive(Clone, Copy, Debug, PartialEq, PartialOrd, Ranged)]
#[ranged(min = -24.0, max = 6.0, default = 0.0, clamp)]
struct Probe(f32);

/// A document value: no `clamp`, so every door refuses.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Ranged)]
#[ranged(min = 0, max = 100, default = 100)]
struct Share(u8);

#[derive(Debug, serde::Deserialize)]
struct Doc {
    max_share: Share,
}

#[derive(Debug, serde::Deserialize)]
struct GainDoc {
    gain: Probe,
}

#[test]
fn a_value_above_the_range_lands_on_the_ceiling() {
    assert_eq!(Probe::from(100.0), Probe::MAX);
}

#[test]
fn a_value_below_the_range_lands_on_the_floor() {
    assert_eq!(Probe::from(-100.0), Probe::MIN);
}

#[test]
fn a_value_inside_the_range_is_kept_exactly() {
    assert_eq!(f32::from(Probe::from(-3.5)), -3.5);
}

#[test]
fn a_nan_becomes_the_default() {
    assert_eq!(Probe::from(f32::NAN), Probe::DEFAULT);
}

#[test]
fn the_default_value_is_the_declared_one() {
    assert_eq!(Probe::default(), Probe::DEFAULT);
}

#[test]
fn checked_construction_keeps_an_in_range_value() {
    assert_eq!(Probe::checked(-3.5), Some(Probe::from(-3.5)));
}

#[test]
fn checked_construction_rejects_a_value_below_the_floor() {
    assert_eq!(Probe::checked(-24.1), None);
}

#[test]
fn checked_construction_rejects_a_value_above_the_ceiling() {
    assert_eq!(Probe::checked(6.1), None);
}

#[test]
fn checked_construction_rejects_a_nan() {
    assert_eq!(Probe::checked(f32::NAN), None);
}

#[test]
fn checked_construction_rejects_a_positive_infinity() {
    assert_eq!(Probe::checked(f32::INFINITY), None);
}

#[test]
fn checked_construction_rejects_a_negative_infinity() {
    assert_eq!(Probe::checked(f32::NEG_INFINITY), None);
}

#[test]
fn an_integer_at_the_ceiling_is_accepted() {
    assert_eq!(Share::checked(100), Some(Share::MAX));
}

#[test]
fn an_integer_above_the_ceiling_is_refused() {
    assert_eq!(Share::checked(101), None);
}

#[test]
fn an_integer_unwraps_to_its_primitive() {
    assert_eq!(u8::from(Share::MAX), 100);
}

#[test]
fn an_integer_default_is_the_declared_one() {
    assert_eq!(Share::default(), Share::MAX);
}

#[test]
fn a_document_value_inside_the_range_parses() {
    let doc: Doc = serde_yaml_ng::from_str("max_share: 100\n").expect("100 is inside the range");

    assert_eq!(doc.max_share, Share::MAX);
}

#[test]
fn a_document_value_outside_the_range_is_refused_by_name_and_by_bounds() {
    let error =
        serde_yaml_ng::from_str::<Doc>("max_share: 140\n").expect_err("140 is outside the range");
    let message = error.to_string();

    assert!(
        message.contains("max_share"),
        "the field is named: {message}"
    );
    assert!(
        message.contains("140"),
        "the offending value is named: {message}"
    );
    assert!(message.contains('0'), "the floor is named: {message}");
    assert!(message.contains("100"), "the ceiling is named: {message}");
}

/// A knob clamps in Rust. A document never does — this is the one place the
/// declarative macro would have let a `NaN` through silently.
#[test]
fn a_document_never_clamps_even_for_a_clamping_type() {
    let doc: GainDoc = serde_yaml_ng::from_str("gain: 0.0\n").expect("unity is inside the range");
    assert_eq!(doc.gain, Probe::DEFAULT);
    let error =
        serde_yaml_ng::from_str::<GainDoc>("gain: .nan\n").expect_err("a document refuses a NaN");

    assert!(
        error.to_string().contains("Probe"),
        "the type is named: {error}"
    );
}
