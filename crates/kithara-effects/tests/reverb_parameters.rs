#![forbid(unsafe_code)]

use std::fmt::Debug;

use kithara_effects::reverb::{Damping, Decay, Width};
use kithara_test_utils::kithara;

fn assert_domain<T: Copy + Debug + PartialEq + Into<f64>>(
    minimum: T,
    maximum: T,
    default: T,
    checked: fn(f64) -> Option<T>,
) {
    let min = minimum.into();
    let max = maximum.into();
    for value in [
        min.next_down(),
        max.next_up(),
        f64::NAN,
        f64::INFINITY,
        f64::NEG_INFINITY,
    ] {
        assert!(checked(value).is_none(), "refuse {value}");
    }
    for value in [min, max, default.into(), min + (max - min) * 0.5] {
        assert_eq!(
            checked(value).expect("admitted value").into().to_bits(),
            value.to_bits()
        );
    }
    assert_eq!(checked(default.into()), Some(default));
}

#[kithara::test]
fn reverb_controls_admit_their_bounds_and_refuse_nonfinite_or_out_of_range_values() {
    assert_domain(Decay::MIN, Decay::MAX, Decay::DEFAULT, Decay::checked);
    assert_domain(
        Damping::MIN,
        Damping::MAX,
        Damping::DEFAULT,
        Damping::checked,
    );
    assert_domain(Width::MIN, Width::MAX, Width::DEFAULT, Width::checked);
    assert_eq!(Decay::default(), Decay::DEFAULT);
    assert_eq!(Damping::default(), Damping::DEFAULT);
    assert_eq!(Width::default(), Width::DEFAULT);
    assert!(f64::from(Decay::MIN) > 0.0);
}
