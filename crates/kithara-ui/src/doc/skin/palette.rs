use serde::{Deserialize, Serialize};

use crate::{error::UiDocError, ids::SourceUri};

/// The design tokens a skin names, written once and expanded wherever the set
/// has to appear again: the palette a document declares, the role a control
/// points at, and the resolved palette a renderer indexes.
macro_rules! color_roles {
    ($expand:ident) => {
        $expand! {
            bg => Bg,
            bg_deep => BgDeep,
            bg_inset => BgInset,
            bg_panel => BgPanel,
            bg_footer => BgFooter,
            bg_panel_2 => BgPanel2,
            bg_select => BgSelect,
            line => Line,
            line_dim => LineDim,
            line_inner => LineInner,
            line_soft => LineSoft,
            line_hi => LineHi,
            line_pop => LinePop,
            text => Text,
            text_dim => TextDim,
            muted => Muted,
            accent => Accent,
            accent_strong => AccentStrong,
            accent_soft => AccentSoft,
            danger => Danger,
            success => Success,
            warning => Warning,
            wave_low => WaveLow,
            wave_mid => WaveMid,
            wave_high => WaveHigh,
            shadow => Shadow,
        }
    };
}

pub(crate) use color_roles;

macro_rules! define_palette {
    ($($field:ident => $role:ident),* $(,)?) => {
        #[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
        #[non_exhaustive]
        pub enum ColorRole {
            $($role,)*
        }

        #[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
        #[serde(deny_unknown_fields)]
        #[non_exhaustive]
        pub struct PaletteDoc {
            $(pub $field: String,)*
        }

        /// What a skin restates of another skin's palette.
        #[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
        #[serde(default, deny_unknown_fields)]
        #[non_exhaustive]
        pub struct PalettePatch {
            $(pub $field: Option<String>,)*
        }

        impl PaletteDoc {
            /// Every role the document declares, paired with the value written
            /// for it.
            pub(crate) fn entries(&self) -> impl Iterator<Item = (ColorRole, &str)> {
                [$((ColorRole::$role, self.$field.as_str()),)*].into_iter()
            }

            /// Takes every role the patch restates, keeping the rest.
            pub(crate) fn patch(&mut self, patch: PalettePatch) {
                $(if let Some(value) = patch.$field {
                    self.$field = value;
                })*
            }
        }
    };
}

color_roles!(define_palette);

impl PaletteDoc {
    pub(crate) fn validate(&self, origin: &SourceUri) -> Result<(), UiDocError> {
        for (_, value) in self.entries() {
            parse_color(value, origin)?;
        }
        Ok(())
    }
}

pub(crate) fn parse_color(value: &str, origin: &SourceUri) -> Result<[u8; 4], UiDocError> {
    let digits = value
        .strip_prefix('#')
        .ok_or_else(|| bad_color(origin, value))?;
    if digits.len() != 6 && digits.len() != 8 {
        return Err(bad_color(origin, value));
    }
    let component = |start| {
        let pair = digits
            .get(start..start + 2)
            .ok_or_else(|| bad_color(origin, value))?;
        u8::from_str_radix(pair, 16).map_err(|_| bad_color(origin, value))
    };
    Ok([
        component(0)?,
        component(2)?,
        component(4)?,
        if digits.len() == 8 {
            component(6)?
        } else {
            255
        },
    ])
}

fn bad_color(origin: &SourceUri, value: &str) -> UiDocError {
    UiDocError::BadColor {
        origin: origin.clone(),
        value: value.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use kithara_test_utils::kithara;

    use super::*;
    use crate::builtin;

    #[kithara::test]
    fn the_palette_names_every_role_once() {
        let roles: Vec<ColorRole> = builtin::skin_doc()
            .palette
            .entries()
            .map(|(role, _)| role)
            .collect();

        assert_eq!(roles.len(), 26);
        for role in &roles {
            assert_eq!(roles.iter().filter(|other| *other == role).count(), 1);
        }
    }

    #[kithara::test]
    fn the_builtin_palette_reads_the_accent_the_document_wrote() {
        let accent = builtin::skin_doc()
            .palette
            .entries()
            .find_map(|(role, value)| (role == ColorRole::Accent).then_some(value));

        assert_eq!(accent, Some("#bb9442"));
    }
}
