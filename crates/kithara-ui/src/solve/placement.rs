use super::Size;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "iced", derive(kithara_derive::Mirror))]
#[cfg_attr(feature = "iced", mirror(from = iced::Alignment))]
pub(crate) enum Alignment {
    Start,
    Center,
    End,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[cfg_attr(feature = "iced", derive(kithara_derive::Mirror))]
#[cfg_attr(feature = "iced", mirror(from = iced::Padding))]
pub(crate) struct Padding {
    pub(crate) bottom: f32,
    pub(crate) left: f32,
    pub(crate) right: f32,
    pub(crate) top: f32,
}

impl From<Padding> for Size {
    fn from(padding: Padding) -> Self {
        Self::new(padding.left + padding.right, padding.top + padding.bottom)
    }
}
