use crate::{
    draw::{Rgba, TRANSPARENT},
    module::{TextStyle, Tone, text_roles},
    render::Skin,
    skin::{ColorRole, FontFamily, FontWeight, TextRoleSkin, TextSkin, ToneColors},
};

/// The one rule that picks between a node's own colour and its active one.
///
/// A node is active or it is not, and the active role only wins while it is;
/// a node naming no active role keeps the base one it declared.
pub(crate) fn active_tone(
    base: Option<ColorRole>,
    active: Option<ColorRole>,
    on: bool,
) -> Option<ColorRole> {
    on.then_some(active).flatten().or(base)
}

/// The role one tone names in a control's own tone set.
pub(crate) const fn tone_color(tone: Tone, tones: ToneColors) -> ColorRole {
    match tone {
        Tone::Accent => tones.accent,
        Tone::Danger => tones.danger,
        Tone::Neutral => tones.neutral,
        Tone::Success => tones.success,
    }
}

impl Skin {
    pub(crate) fn rgba(&self, role: ColorRole) -> Rgba {
        self.palette[role]
    }
    /// The typography one document text style names, with the tone already
    /// selected.
    ///
    /// Both hosts ask the skin rather than keeping a table each: a style the
    /// two answered differently would paint the same document in two
    /// typefaces, which is the one thing the shared base exists to prevent.
    /// There is no wildcard arm, so a new style does not build until it is
    /// given a skin entry.
    pub(crate) fn text_role(
        &self,
        style: TextStyle,
        color: Option<ColorRole>,
        active_color: Option<ColorRole>,
        active: bool,
    ) -> TextRoleSkin {
        let role = self.text.role(style);
        let skin_active = (style == TextStyle::DeckLetter).then_some(self.text.deck_letter_active);
        TextRoleSkin {
            color: active_tone(color, active_color.or(skin_active), active).unwrap_or(role.color),
            ..role
        }
    }
    /// Resolves one state of a [`StateColors`]: a state naming no role paints
    /// nothing.
    pub(crate) fn tint(&self, role: Option<ColorRole>) -> Rgba {
        role.map_or(TRANSPARENT, |role| self.rgba(role))
    }
}

impl TextStyle {
    /// The words this style sets, which are not always the words the document
    /// wrote: a micro label is small capitals, so it is set in capitals whatever
    /// case it was given.
    ///
    /// Every host asks here rather than deciding for itself, because the case a
    /// run is set in changes how wide it is, and two hosts that answered
    /// separately would lay the same document out differently.
    pub(crate) fn cased(self, content: String) -> String {
        match self {
            Self::MicroLabel => content.to_uppercase(),
            _ => content,
        }
    }
}

impl TextRoleSkin {
    /// This role set in the face a run names, keeping the skin's where it
    /// names none.
    pub(crate) fn faced(self, font: Option<FontFamily>, weight: Option<FontWeight>) -> Self {
        Self {
            font: font.unwrap_or(self.font),
            weight: weight.unwrap_or(self.weight),
            ..self
        }
    }
}

macro_rules! define_text_role {
    ($($(#[$attr:meta])* $field:ident => $role:ident),* $(,)?) => {
        impl TextSkin {
            /// The entry this skin gives one typographic role.
            pub(crate) fn role(&self, style: TextStyle) -> TextRoleSkin {
                match style {
                    $(TextStyle::$role => self.$field,)*
                }
            }
        }
    };
}

text_roles!(define_text_role);

#[cfg(test)]
mod tests {
    use kithara_test_utils::kithara;

    use super::*;
    use crate::{builtin, module::TextStyle, skin::ColorRole};

    #[kithara::test]
    fn a_node_colour_stands_in_for_the_one_the_role_carries() {
        let skin = builtin::skin();

        assert_eq!(
            skin.text_role(TextStyle::Mono, Some(ColorRole::Text), None, false),
            TextRoleSkin {
                color: ColorRole::Text,
                ..skin.text.mono
            }
        );
    }

    #[kithara::test]
    fn a_node_switches_between_the_two_colours_it_names() {
        let skin = builtin::skin();
        let role = |active| {
            skin.text_role(
                TextStyle::Mono,
                Some(ColorRole::Muted),
                Some(ColorRole::Accent),
                active,
            )
        };

        assert_eq!(
            role(true),
            TextRoleSkin {
                color: ColorRole::Accent,
                ..skin.text.mono
            }
        );
        assert_eq!(
            role(false),
            TextRoleSkin {
                color: ColorRole::Muted,
                ..skin.text.mono
            }
        );
    }

    #[kithara::test]
    fn an_active_node_naming_one_colour_keeps_it() {
        let skin = builtin::skin();

        assert_eq!(
            skin.text_role(TextStyle::Caption, Some(ColorRole::Accent), None, true),
            TextRoleSkin {
                color: ColorRole::Accent,
                ..skin.text.caption
            }
        );
    }

    #[kithara::test]
    fn the_deck_letter_takes_the_active_colour_its_skin_entry_declares() {
        let skin = builtin::skin();
        let base = skin.text_role(TextStyle::DeckLetter, None, None, false);

        assert_eq!(base, skin.text.deck_letter);
        assert_eq!(
            skin.text_role(TextStyle::DeckLetter, None, None, true),
            TextRoleSkin {
                color: skin.text.deck_letter_active,
                ..base
            }
        );
        assert_eq!(
            skin.text_role(TextStyle::DeckLetter, None, Some(ColorRole::Warning), true),
            TextRoleSkin {
                color: ColorRole::Warning,
                ..base
            }
        );
    }

    #[kithara::test]
    fn brand_small_resolves_under_the_display_family_and_never_the_mono_one() {
        let skin = builtin::skin();
        let role = skin.text_role(TextStyle::BrandSmall, None, None, false);

        assert_eq!(role, skin.text.brand_small);
        assert_eq!(
            skin.text_role(TextStyle::BrandSmall, None, None, true),
            role
        );
        assert_ne!(
            role.font, skin.text.mono.font,
            "the mono micro roles are Mono and the brand pair is Display"
        );
    }

    #[kithara::test]
    fn a_style_declaring_no_active_colour_ignores_the_flag() {
        let skin = builtin::skin();

        for style in [
            TextStyle::Body,
            TextStyle::Brand,
            TextStyle::TrackTitle,
            TextStyle::Telemetry,
            TextStyle::MicroLabel,
            TextStyle::Section,
            TextStyle::Mono,
            TextStyle::PivotArrow,
            TextStyle::PivotTitle,
            TextStyle::PivotValue,
            TextStyle::Caption,
            TextStyle::VisFooter,
            TextStyle::VisMeta,
            TextStyle::VisTitle,
        ] {
            assert_eq!(
                skin.text_role(style, None, None, true),
                skin.text_role(style, None, None, false),
                "{style:?}"
            );
        }
    }

    #[kithara::test]
    fn active_tone_takes_the_active_role_only_while_the_flag_is_set() {
        let pair =
            |active| active_tone(Some(ColorRole::LineInner), Some(ColorRole::Accent), active);

        assert_eq!(pair(true), Some(ColorRole::Accent));
        assert_eq!(pair(false), Some(ColorRole::LineInner));
        assert_eq!(
            active_tone(Some(ColorRole::LineHi), None, true),
            Some(ColorRole::LineHi)
        );
        assert_eq!(active_tone(None, None, true), None);
    }
}
