use crate::{
    module::ButtonStyle,
    mount::Control,
    size::{Dim, SizeSpec},
    skin::SkinDoc,
};

/// A pressable button, worded and optionally iconed by the document.
pub(crate) struct Button {
    pub(crate) style: ButtonStyle,
}

impl Control for Button {
    fn size(&self, skin: &SkinDoc) -> SizeSpec {
        match self.style {
            ButtonStyle::VisNav => square(skin.vis.nav_cell_size),
            ButtonStyle::MicroPrimary => square(skin.button.micro_size),
            ButtonStyle::Default | ButtonStyle::Transport | ButtonStyle::TransportPrimary => {
                skin.button.size
            }
        }
    }
}

fn square(side: f32) -> SizeSpec {
    SizeSpec::new(Dim::Fixed(side), Dim::Fixed(side))
}
