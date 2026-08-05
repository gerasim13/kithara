use crate::{mount::Control, size::SizeSpec, skin::SkinDoc};

/// The global bar's own button, which opens the settings surface.
pub(crate) struct Settings;

impl Control for Settings {
    fn size(&self, skin: &SkinDoc) -> SizeSpec {
        skin.global_bar.settings_size
    }
}
