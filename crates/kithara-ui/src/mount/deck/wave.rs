use crate::{module::WaveStyle, mount::Control, size::SizeSpec, skin::SkinDoc};

/// The track's waveform, zoomed and scrubbed.
pub(crate) struct Wave {
    pub(crate) style: WaveStyle,
}

impl Control for Wave {
    /// Each style is a height the rows it stands in are built to, so a fourth
    /// one names its own number before it renders.
    fn size(&self, skin: &SkinDoc) -> SizeSpec {
        match self.style {
            WaveStyle::Default => skin.wave.default_size,
            WaveStyle::Hero => skin.wave.size,
            WaveStyle::Micro => skin.wave.micro_size,
        }
    }
}

#[cfg(any(feature = "iced", feature = "masonry"))]
pub(crate) mod host {
    use bon::Builder;

    use crate::{
        atoms::wave::face::{Drawn, Wave as Face},
        expand::Binding,
        hosts::controls::{Draws, Reading},
        ids::InternId,
        module::WaveStyle,
        render::Skin,
    };

    /// The wave as a host draws it: the badge it wears and the endpoint its
    /// zoom follows.
    #[derive(Builder)]
    pub(crate) struct Wave<'a> {
        pub(crate) badge: Option<InternId>,
        pub(crate) zoom: Option<&'a Binding>,
        pub(crate) style: WaveStyle,
    }

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
    }
}
