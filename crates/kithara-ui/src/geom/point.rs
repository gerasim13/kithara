/// A toolkit-neutral point in logical pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "iced", derive(kithara_derive::Mirror))]
#[cfg_attr(feature = "iced", mirror(from = iced::Point))]
pub struct Pt {
    pub x: f32,
    pub y: f32,
}

impl Pt {
    #[cfg(any(feature = "render", feature = "vello"))]
    pub(crate) fn distance(self, other: Self) -> f32 {
        (self.x - other.x).hypot(self.y - other.y)
    }
}
