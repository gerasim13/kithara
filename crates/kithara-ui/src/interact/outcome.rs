#[derive(Clone, Copy, Debug, PartialEq, fieldwork::Fieldwork)]
#[fieldwork(opt_in, get)]
pub(crate) struct Outcome<T = f32> {
    value: Option<T>,
    #[field(get = is_captured, vis = "pub(crate)")]
    capture: bool,
}

impl<T> Outcome<T> {
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

    /// A verdict that also takes the pointer: the control owns the gesture now
    /// and whatever is behind it must not see the same event.
    pub(crate) const fn set(value: T) -> Self {
        Self {
            value: Some(value),
            capture: true,
        }
    }

    /// A verdict that leaves the pointer alone, so the control underneath keeps
    /// its own click while this one watches the same gesture.
    pub(crate) const fn observed(value: T) -> Self {
        Self {
            value: Some(value),
            capture: false,
        }
    }

    pub(crate) fn value(self) -> Option<T> {
        self.value
    }

    pub(crate) fn map<U>(self, map: impl FnOnce(T) -> U) -> Outcome<U> {
        Outcome {
            value: self.value.map(map),
            capture: self.capture,
        }
    }
}
