use kithara_derive::Ranged;

#[derive(Clone, Copy, Debug, PartialEq, Ranged)]
#[ranged(min = 0.015, max = 0.5, default = 0.12, clamp)]
pub struct Zoom(f32);

pub const DEFAULT_ZOOM: f32 = Zoom::DEFAULT.0;

mod consts {
    pub(super) const BUTTON_FACTOR: f32 = 0.7;
}

/// Narrows the visible window by one button press.
#[must_use]
pub fn zoom_in(zoom: Zoom) -> Zoom {
    Zoom::from(f32::from(zoom) * consts::BUTTON_FACTOR)
}

/// Widens the visible window by one button press.
#[must_use]
pub fn zoom_out(zoom: Zoom) -> Zoom {
    Zoom::from(f32::from(zoom) / consts::BUTTON_FACTOR)
}

#[cfg(test)]
mod tests {
    use kithara_test_utils::kithara;

    use super::*;

    mod consts {
        pub(super) const EPSILON: f32 = 0.000_1;
    }

    fn assert_near(actual: impl Into<f32>, expected: f32) {
        let actual = actual.into();
        assert!(
            (actual - expected).abs() < consts::EPSILON,
            "expected {expected}, got {actual}"
        );
    }

    #[kithara::test]
    fn buttons_step_wider_than_a_detent_and_clamp() {
        assert_near(zoom_in(DEFAULT_ZOOM.into()), 0.084);
        assert_near(zoom_out(DEFAULT_ZOOM.into()), 0.171_428_57);
        assert_near(zoom_in(f32::from(Zoom::MIN).into()), f32::from(Zoom::MIN));
        assert_near(zoom_out(f32::from(Zoom::MAX).into()), f32::from(Zoom::MAX));
    }

    #[kithara::test]
    fn zoom_rejects_non_finite_documents_and_clamps_knob_input() {
        assert!(Zoom::checked(f32::NAN).is_none());
        assert!(Zoom::checked(f32::INFINITY).is_none());
        assert_eq!(Zoom::from(0.0), Zoom::MIN);
        assert_eq!(Zoom::from(1.0), Zoom::MAX);
        assert_eq!(Zoom::from(f32::NAN), Zoom::DEFAULT);
    }
}
