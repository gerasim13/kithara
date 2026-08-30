//! Placement of one scene child, and the drag that carries it.

#[cfg(feature = "iced")]
mod iced;

#[cfg(feature = "iced")]
pub(crate) use iced::placed;
