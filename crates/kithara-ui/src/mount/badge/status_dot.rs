use bon::Builder;

#[cfg(any(feature = "iced", feature = "masonry"))]
use crate::{expand::Binding, ids::InternId, module::Tone};

/// A toned dot beside a word.
#[derive(Builder, kithara_derive::ViewControl, kithara_derive::Control)]
#[control(size = skin.status_dot.size)]
#[derive(kithara_derive::NodeControl)]
pub(crate) struct StatusDot<#[cfg(any(feature = "iced", feature = "masonry"))] 'a> {
    #[cfg(any(feature = "iced", feature = "masonry"))]
    pub(crate) label: InternId,
    #[cfg(any(feature = "iced", feature = "masonry"))]
    pub(crate) active: Option<&'a Binding>,
    #[cfg(any(feature = "iced", feature = "masonry"))]
    pub(crate) active_tone: Option<Tone>,
    #[cfg(any(feature = "iced", feature = "masonry"))]
    pub(crate) dot_size: Option<f32>,
    #[cfg(any(feature = "iced", feature = "masonry"))]
    pub(crate) tone: Tone,
}

#[cfg(any(feature = "iced", feature = "masonry"))]
mod host {
    use super::StatusDot;
    use crate::{
        atoms::design::status_dot::{StatusDot as Face, StatusDotData},
        render::{
            ReadValue, Skin,
            controls::{Draws, Reading},
        },
    };

    impl Draws for StatusDot<'_> {
        type Painter = Face;

        fn data(&self, read: Reading<'_>) -> Option<StatusDotData> {
            Some(StatusDotData {
                active: self.active.is_some_and(|binding| {
                    matches!(read.ctx.read(binding), Some(ReadValue::Bool(true)))
                }),
                label: read.ctx.ui.resolve(self.label).to_owned(),
            })
        }

        fn painter(&self, skin: &Skin) -> Face {
            Face::with_active_tone(self.tone, self.active_tone, self.dot_size, skin)
        }
    }
}
