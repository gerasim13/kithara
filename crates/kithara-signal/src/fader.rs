use kithara_derive::Ranged;
use serde as _;

/// A normalized fader coordinate, distinct from linear signal amplitude.
#[derive(Clone, Copy, Debug, PartialEq, Ranged)]
#[ranged(min = 0.0, max = 1.0, default = 1.0, clamp)]
pub struct FaderValue(f32);

#[cfg(test)]
mod tests {
    use kithara_test_utils::kithara;

    use super::FaderValue;

    #[kithara::test]
    fn clamps_finite_values_and_maps_nan_to_default() {
        assert_eq!(f32::from(FaderValue::from(-1.0)), 0.0);
        assert_eq!(f32::from(FaderValue::from(2.0)), 1.0);
        assert_eq!(FaderValue::from(f32::NAN), FaderValue::DEFAULT);
        assert!(FaderValue::checked(f32::INFINITY).is_none());
    }
}
