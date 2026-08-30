/// Logical two-dimensional extent used by custom content.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[non_exhaustive]
pub struct Size2 {
    pub w: f32,
    pub h: f32,
}

impl Size2 {
    #[must_use]
    pub const fn new(w: f32, h: f32) -> Self {
        Self { w, h }
    }
}

/// Minimum and maximum logical extents available during intrinsic measurement.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct SizeLimits {
    min: Size2,
    max: Size2,
}

impl SizeLimits {
    #[must_use]
    pub const fn new(min: Size2, max: Size2) -> Self {
        Self { min, max }
    }

    #[must_use]
    pub const fn min(self) -> Size2 {
        self.min
    }

    #[must_use]
    pub const fn max(self) -> Size2 {
        self.max
    }
}
