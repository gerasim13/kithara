use crate::{
    draw::{Rgba, TRANSPARENT},
    error::UiDocError,
    ids::SourceUri,
    module::TextStyle,
    render::theme::RenderPalette,
    shaping::{FontPolicy, TextResources},
    skin::{
        ButtonSkin, CellSkin, CheckboxSkin, ChipSkin, ChromeSkin, ColorRole, CrossfaderSkin,
        DeckSkin, DividerSkin, DragSkin, FaderSkin, GlobalBarSkin, KnobSkin, LayoutPreviewSkin,
        LayoutSkin, MenuSkin, MeterSkin, NavSkin, PopSkin, PortalMapSkin, RangeSkin, ReadoutSkin,
        ScrollSkin, SegmentedSkin, SelectSkin, SkinDoc, StatusDotSkin, SwatchSkin, TabLargeSkin,
        TableSkin, TelemetrySkin, TextRoleSkin, TextSkin, ToggleSkin, TreeSkin, VisSkin,
        VuStereoSkin, VuVerticalSkin, WaveSkin, WindowSkin, skin_sections,
    },
    text::TextDoc,
};

/// The three captions painted around a crossfader track.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct CrossfaderLabels {
    pub left: String,
    pub center: String,
    pub right: String,
}

/// Both the resolved skin a renderer reads and the resolve step that fills it
/// in: one arm per section, expanded from the document's own section list, so
/// a new section reaches the renderers by being declared once.
macro_rules! define_skin {
    ($($field:ident: $section:ident => $patch:ident,)*) => {
        /// Resolved skin consumed by renderers.
        #[derive(Clone, Debug, PartialEq, fieldwork::Fieldwork)]
        #[non_exhaustive]
        #[fieldwork(opt_in, get)]
        pub struct Skin {
            pub palette: RenderPalette,
            pub crossfader_labels: CrossfaderLabels,
            pub table_footer_rows: String,
            pub tree_search_placeholder: String,
            $(pub $field: $section,)*
            #[field(get, vis = "pub(crate)")]
            text_resources: TextResources,
            /// The document this skin was resolved from, which is what a
            /// host compiles its pages against: what a page measures comes
            /// from the skin's own numbers, not only what it is painted with.
            #[field(get)]
            document: SkinDoc,
        }

        impl Skin {
            /// Resolves a parsed document under an explicit font policy.
            ///
            /// # Errors
            /// Returns [`UiDocError`] when a palette value or embedded font is
            /// invalid, or [`UiDocError::UnknownTextKey`] when `catalog` is
            /// missing a caption.
            pub fn resolve_with_font_policy(
                document: SkinDoc,
                catalog: &TextDoc,
                origin: &SourceUri,
                font_policy: FontPolicy,
            ) -> Result<Self, UiDocError> {
                Ok(Self {
                    palette: RenderPalette::resolve(&document.palette, origin)?,
                    crossfader_labels: CrossfaderLabels {
                        left: text_field(catalog, "crossfader.left_label", origin)?,
                        center: text_field(catalog, "crossfader.center_label", origin)?,
                        right: text_field(catalog, "crossfader.right_label", origin)?,
                    },
                    table_footer_rows: text_field(catalog, "table.footer_rows", origin)?,
                    tree_search_placeholder: text_field(catalog, "tree.search_placeholder", origin)?,
                    $($field: document.$field,)*
                    text_resources: TextResources::new(font_policy)?,
                    document,
                })
            }
        }
    };
}

skin_sections!(define_skin);

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

impl Skin {
    /// What the skin's own document calls it, which is how anything offering a
    /// choice of skins tells one from another.
    #[must_use]
    pub fn id(&self) -> &str {
        &self.document.id.0
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

    pub(crate) fn rgba(&self, role: ColorRole) -> Rgba {
        self.palette[role]
    }

    /// Resolves one state of a [`StateColors`]: a state naming no role paints
    /// nothing.
    pub(crate) fn tint(&self, role: Option<ColorRole>) -> Rgba {
        role.map_or(TRANSPARENT, |role| self.rgba(role))
    }

    /// Resolves a parsed document into neutral colors and render metrics,
    /// pulling the crossfader, tree search and table footer captions from
    /// `catalog`.
    ///
    /// # Errors
    /// Returns [`UiDocError`] when a palette value or embedded font is invalid,
    /// or [`UiDocError::UnknownTextKey`] when `catalog` is missing one of those
    /// captions.
    pub fn resolve(
        document: SkinDoc,
        catalog: &TextDoc,
        origin: &SourceUri,
    ) -> Result<Self, UiDocError> {
        Self::resolve_with_font_policy(document, catalog, origin, FontPolicy::System)
    }
}

fn text_field(catalog: &TextDoc, key: &str, origin: &SourceUri) -> Result<String, UiDocError> {
    catalog
        .get(key)
        .map(str::to_owned)
        .ok_or_else(|| UiDocError::UnknownTextKey {
            origin: origin.clone(),
            key: key.to_owned(),
            path: format!("skin.{key}"),
        })
}

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
}
