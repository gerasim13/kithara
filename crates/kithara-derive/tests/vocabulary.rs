use kithara_derive::{EnumStr, Variants};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Variants)]
enum Unit {
    First,
    Second,
}

#[derive(EnumStr)]
#[enum_str(all = KINDS, method = kind)]
enum Value {
    Unit,
    Tuple(u8),
    Named { value: u8 },
}

#[test]
fn variants_preserves_declaration_order() {
    assert_eq!(Unit::ALL, &[Unit::First, Unit::Second]);
}

#[test]
fn enum_str_names_every_shape_exhaustively() {
    assert_eq!(Value::KINDS, &["Unit", "Tuple", "Named"]);
    assert_eq!(Value::Unit.kind(), "Unit");
    let tuple = Value::Tuple(1);
    assert_eq!(tuple.kind(), "Tuple");
    assert!(matches!(tuple, Value::Tuple(1)));
    let named = Value::Named { value: 2 };
    assert_eq!(named.kind(), "Named");
    assert!(matches!(named, Value::Named { value: 2 }));
}
