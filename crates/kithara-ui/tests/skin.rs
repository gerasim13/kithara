use kithara_test_utils::kithara;
use kithara_ui::{
    builtin,
    envelope::{DocKind, probe},
    error::UiDocError,
    ids::{DocId, SourceUri},
    skin::parse_skin,
};

fn origin() -> SourceUri {
    SourceUri("kithara-dark.kskin.ron".to_owned())
}

/// Every section and every field a skin document declares is required, and an
/// unknown one is refused, so a document that parses at all is a complete one.
/// A role added to the palette with no value in the asset fails here rather
/// than reaching a renderer.
#[kithara::test]
fn builtin_skin_parses_every_required_section() {
    let document = parse_skin(builtin::DARK_SKIN, &origin()).unwrap();

    assert_eq!(document.id, DocId("kithara-dark".to_owned()));
}

#[kithara::test]
fn skin_envelope_is_probed_as_skin() {
    let envelope = probe(builtin::DARK_SKIN, &origin()).unwrap();

    assert_eq!(envelope.kind, DocKind::Skin);
}

#[kithara::test]
fn unknown_skin_field_is_rejected() {
    let text = builtin::DARK_SKIN.replacen(
        "id: \"kithara-dark\",",
        "id: \"kithara-dark\", unknown: 1,",
        1,
    );
    let error = parse_skin(&text, &origin()).unwrap_err();

    assert!(matches!(error, UiDocError::Syntax { .. }));
}

/// A field left out of any section is refused rather than defaulted, sampled
/// once per kind of section the document is made of.
#[kithara::test]
fn a_required_skin_field_is_rejected_when_missing() {
    for line in [
        "        body_fill: BgSelect,\n",
        "        rail_height: 6.0,\n",
        "        header_height: 26.0,\n",
    ] {
        let text = builtin::DARK_SKIN.replacen(line, "", 1);
        let error = parse_skin(&text, &origin()).unwrap_err();

        assert!(
            matches!(error, UiDocError::Syntax { .. }),
            "a skin without `{}` must be refused",
            line.trim()
        );
    }
}

#[kithara::test]
fn malformed_skin_hex_has_typed_error() {
    let text = builtin::DARK_SKIN.replacen("#12121f", "#12xx1f", 1);
    let error = parse_skin(&text, &origin()).unwrap_err();

    assert!(matches!(
        error,
        UiDocError::BadColor { origin, value }
            if origin == self::origin() && value == "#12xx1f"
    ));
}
