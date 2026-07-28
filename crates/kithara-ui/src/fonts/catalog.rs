#[cfg(feature = "vello-backend")]
use super::bytes::{
    INTER_REGULAR_BYTES, INTER_SEMIBOLD_BYTES, JETBRAINS_MONO_MEDIUM_BYTES,
    JETBRAINS_MONO_REGULAR_BYTES, JETBRAINS_MONO_SEMIBOLD_BYTES, SPACE_GROTESK_BOLD_BYTES,
    SPACE_GROTESK_MEDIUM_BYTES, SPACE_GROTESK_REGULAR_BYTES, SPACE_GROTESK_SEMIBOLD_BYTES,
};
use crate::skin::{FontFamily, FontWeight};

#[derive(Clone, Copy, Debug)]
pub(crate) enum FontFace {
    InterRegular,
    InterSemibold,
    JetBrainsMonoRegular,
    JetBrainsMonoMedium,
    JetBrainsMonoSemibold,
    SpaceGroteskRegular,
    SpaceGroteskMedium,
    SpaceGroteskSemibold,
    SpaceGroteskBold,
}

impl FontFace {
    #[cfg(feature = "vello-backend")]
    pub(crate) const fn bytes(self) -> &'static [u8] {
        match self {
            Self::InterRegular => INTER_REGULAR_BYTES,
            Self::InterSemibold => INTER_SEMIBOLD_BYTES,
            Self::JetBrainsMonoRegular => JETBRAINS_MONO_REGULAR_BYTES,
            Self::JetBrainsMonoMedium => JETBRAINS_MONO_MEDIUM_BYTES,
            Self::JetBrainsMonoSemibold => JETBRAINS_MONO_SEMIBOLD_BYTES,
            Self::SpaceGroteskRegular => SPACE_GROTESK_REGULAR_BYTES,
            Self::SpaceGroteskMedium => SPACE_GROTESK_MEDIUM_BYTES,
            Self::SpaceGroteskSemibold => SPACE_GROTESK_SEMIBOLD_BYTES,
            Self::SpaceGroteskBold => SPACE_GROTESK_BOLD_BYTES,
        }
    }

    #[cfg(feature = "render")]
    pub(crate) const fn family_name(self) -> &'static str {
        match self {
            Self::InterRegular | Self::InterSemibold => "Inter",
            Self::JetBrainsMonoRegular
            | Self::JetBrainsMonoMedium
            | Self::JetBrainsMonoSemibold => mono_family_name(),
            Self::SpaceGroteskRegular
            | Self::SpaceGroteskMedium
            | Self::SpaceGroteskSemibold
            | Self::SpaceGroteskBold => "Space Grotesk",
        }
    }
}

pub(crate) const fn select(family: FontFamily, weight: FontWeight) -> FontFace {
    match (family, weight) {
        (FontFamily::Sans, FontWeight::Normal | FontWeight::Medium) => FontFace::InterRegular,
        (FontFamily::Sans, FontWeight::Semibold | FontWeight::Bold) => FontFace::InterSemibold,
        (FontFamily::Mono, FontWeight::Normal) => FontFace::JetBrainsMonoRegular,
        (FontFamily::Mono, FontWeight::Medium) => FontFace::JetBrainsMonoMedium,
        (FontFamily::Mono, FontWeight::Semibold | FontWeight::Bold) => {
            FontFace::JetBrainsMonoSemibold
        }
        (FontFamily::Display, FontWeight::Normal) => FontFace::SpaceGroteskRegular,
        (FontFamily::Display, FontWeight::Medium) => FontFace::SpaceGroteskMedium,
        (FontFamily::Display, FontWeight::Semibold) => FontFace::SpaceGroteskSemibold,
        (FontFamily::Display, FontWeight::Bold) => FontFace::SpaceGroteskBold,
    }
}

#[cfg(all(feature = "render", not(target_vendor = "apple")))]
const fn mono_family_name() -> &'static str {
    "JetBrains Mono"
}

#[cfg(all(feature = "render", target_vendor = "apple"))]
const fn mono_family_name() -> &'static str {
    "Menlo"
}
