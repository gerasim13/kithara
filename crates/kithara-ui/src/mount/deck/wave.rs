use bon::Builder;

#[cfg(any(feature = "iced", feature = "masonry"))]
use crate::{expand::Binding, ids::InternId};
use crate::{module::WaveStyle, size::SizeSpec, skin::SkinDoc};

/// The track's waveform, zoomed and scrubbed.
#[derive(Builder, kithara_derive::Control, kithara_derive::NodeControl)]
#[control(size = size(self.style, skin))]
pub(crate) struct Wave<#[cfg(any(feature = "iced", feature = "masonry"))] 'a> {
    #[cfg(any(feature = "iced", feature = "masonry"))]
    pub(crate) badge: Option<InternId>,
    #[cfg(any(feature = "iced", feature = "masonry"))]
    pub(crate) zoom: Option<&'a Binding>,
    pub(crate) style: WaveStyle,
}

/// Each style fixes the height of its containing row.
fn size(style: WaveStyle, skin: &SkinDoc) -> SizeSpec {
    match style {
        WaveStyle::Default => skin.wave.default_size,
        WaveStyle::Hero => skin.wave.size,
        WaveStyle::Micro => skin.wave.micro_size,
    }
}

#[cfg(any(feature = "iced", feature = "masonry"))]
mod host {
    use super::Wave;
    #[cfg(feature = "masonry")]
    use crate::render::controls::DataRefresh;
    use crate::{
        atoms::wave::face::{Drawn, Wave as Face},
        render::{
            Skin,
            controls::{Draws, Reading},
        },
    };

    impl Draws for Wave<'_> {
        type Painter = Face;

        /// A deck with no track loaded still draws its wave: an empty shape in
        /// the frame, and — on the hero wave — the panel saying so.
        fn data(&self, read: Reading<'_>) -> Option<Drawn> {
            Some(Drawn::read(
                self.style,
                read.ctx.wave_zoom(self.zoom),
                self.badge.map(|id| read.ctx.ui.resolve(id)),
                read.value,
                &read.ctx,
                read.scope,
            ))
        }

        fn painter(&self, skin: &Skin) -> Face {
            Face::new(self.style, skin)
        }

        #[cfg(feature = "masonry")]
        fn retained_refresh(
            &self,
            read: Reading<'_>,
            _endpoint: Option<&str>,
        ) -> Option<DataRefresh<Drawn>> {
            let scope = read.scope.to_owned();
            let zoom = self
                .zoom
                .map(|binding| read.ctx.ui.resolve(binding.key).to_owned());
            Some(Box::new(move |data, ctx| {
                data.refresh(&ctx, &scope, zoom.as_deref())
            }))
        }
    }
}
