#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Outcome {
    value: Option<f32>,
    capture: bool,
}

impl Outcome {
    pub(crate) const IGNORED: Self = Self {
        value: None,
        capture: false,
    };

    pub(crate) const fn captured() -> Self {
        Self {
            value: None,
            capture: true,
        }
    }

    pub(crate) const fn set(value: f32) -> Self {
        Self {
            value: Some(value),
            capture: true,
        }
    }

    pub(crate) const fn value(self) -> Option<f32> {
        self.value
    }

    pub(crate) const fn is_captured(self) -> bool {
        self.capture
    }
}
