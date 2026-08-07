use iced::{Color, Element};

use super::geometry::active_tone;
use crate::{
    atoms::{
        design::{segmented::Segmented, select::Select},
        readout::Readout,
    },
    compile::CompiledUi,
    ids::InternId,
    module::{FaderStyle, GlyphStyle, IconName, Tone},
    render::{IcedSkin, InputOwner, ReadValue, Skin, UiEvent, icons::document_icon},
    skin::ColorRole,
    widgets::{Widget, fader::Fader, nav::Glyph},
};

pub(super) fn crossfader<'a>(
    path: &'a str,
    ticks: bool,
    value: Option<&ReadValue<'_>>,
    skin: &'a Skin,
    owner: InputOwner,
) -> Element<'a, UiEvent> {
    crate::render::controls::crossfader(path, ticks, value, skin, owner)
}

pub(super) fn fader<'a>(
    path: &'a str,
    style: FaderStyle,
    label: Option<InternId>,
    value: Option<&ReadValue<'_>>,
    ui: &'a CompiledUi,
    skin: &'a Skin,
    owner: InputOwner,
) -> Element<'a, UiEvent> {
    Fader::builder()
        .path(path)
        .style(style)
        .maybe_label(label.map(|id| ui.resolve(id)))
        .maybe_value(value)
        .skin(skin)
        .owner(owner)
        .build()
        .view()
}

pub(super) fn readout<'a>(
    label: Option<InternId>,
    tone: Tone,
    framed: bool,
    value: Option<&ReadValue<'_>>,
    ui: &'a CompiledUi,
    skin: &'a Skin,
) -> Element<'a, UiEvent> {
    Readout::builder()
        .maybe_label(label.map(|id| ui.resolve(id)))
        .tone(tone)
        .framed(framed)
        .maybe_value(value)
        .skin(skin)
        .build()
        .view()
}

pub(super) fn segmented<'a>(
    path: &'a str,
    items: &[InternId],
    value: Option<&ReadValue<'_>>,
    ui: &'a CompiledUi,
    skin: &Skin,
    owner: InputOwner,
) -> Element<'a, UiEvent> {
    let segmented = Segmented::builder()
        .path(path)
        .items(items.iter().map(|id| ui.resolve(*id)).collect())
        .maybe_value(value)
        .skin(skin)
        .build();
    match owner {
        InputOwner::Leaf => segmented.view(),
        InputOwner::Engine => segmented.painted(),
    }
}

pub(super) fn select<'a>(label: InternId, ui: &'a CompiledUi, skin: &Skin) -> Element<'a, UiEvent> {
    Select::builder()
        .label(ui.resolve(label))
        .skin(skin)
        .build()
        .view()
}

pub(super) fn glyph(
    icon: IconName,
    active_icon: Option<IconName>,
    style: GlyphStyle,
    color: Option<ColorRole>,
    active_color: Option<ColorRole>,
    active: bool,
    skin: &Skin,
) -> Element<'static, UiEvent> {
    let icon = active.then_some(active_icon).flatten().unwrap_or(icon);
    let tone = glyph_tone(color, active_color, active, skin);
    Glyph::builder()
        .icon(document_icon(icon))
        .size(glyph_size(style, skin))
        .color(tone.unwrap_or_else(|| glyph_base(style, skin)))
        .build()
        .view()
}

fn glyph_size(style: GlyphStyle, skin: &Skin) -> f32 {
    match style {
        GlyphStyle::Default => skin.nav.header_icon_size,
        GlyphStyle::Vis => skin.vis.icon_size,
        GlyphStyle::Menu => skin.menu.icon_size,
        GlyphStyle::MenuBurger => skin.menu.burger_icon_size,
        GlyphStyle::MenuSmall => skin.menu.small_icon_size,
        GlyphStyle::MenuCell => skin.menu.cell_icon_size,
    }
}

fn glyph_base(style: GlyphStyle, skin: &Skin) -> Color {
    match style {
        GlyphStyle::Vis => skin.color(skin.vis.icon_color),
        GlyphStyle::Default
        | GlyphStyle::Menu
        | GlyphStyle::MenuBurger
        | GlyphStyle::MenuSmall
        | GlyphStyle::MenuCell => skin.palette.text.into(),
    }
}

fn glyph_tone(
    color: Option<ColorRole>,
    active_color: Option<ColorRole>,
    active: bool,
    skin: &Skin,
) -> Option<Color> {
    active_tone(color, active_color, active).map(|role| skin.color(role))
}

#[cfg(test)]
mod tests {
    use kithara_test_utils::kithara;

    use super::*;
    use crate::builtin;

    #[kithara::test]
    fn every_glyph_style_takes_its_own_skin_icon_size() {
        let skin = builtin::skin();

        for (style, size) in [
            (GlyphStyle::Default, skin.nav.header_icon_size),
            (GlyphStyle::Vis, skin.vis.icon_size),
            (GlyphStyle::Menu, skin.menu.icon_size),
            (GlyphStyle::MenuBurger, skin.menu.burger_icon_size),
            (GlyphStyle::MenuSmall, skin.menu.small_icon_size),
            (GlyphStyle::MenuCell, skin.menu.cell_icon_size),
        ] {
            assert_eq!(glyph_size(style, skin), size, "{style:?}");
        }
    }

    #[kithara::test]
    fn a_declared_glyph_pair_switches_on_the_active_flag() {
        let skin = builtin::skin();
        let tone = |active| {
            glyph_tone(
                Some(ColorRole::Muted),
                Some(ColorRole::Danger),
                active,
                skin,
            )
        };

        assert_eq!(tone(false), Some(skin.color(ColorRole::Muted)));
        assert_eq!(tone(true), Some(skin.color(ColorRole::Danger)));
    }

    #[kithara::test]
    fn an_active_glyph_naming_no_active_colour_keeps_its_base() {
        let skin = builtin::skin();
        let tone = glyph_tone(Some(ColorRole::Accent), None, true, skin);

        assert_eq!(tone, Some(skin.color(ColorRole::Accent)));
    }

    #[kithara::test]
    fn a_glyph_naming_no_colour_leaves_the_tone_to_its_style() {
        let skin = builtin::skin();

        assert_eq!(glyph_tone(None, None, false, skin), None);
        assert_eq!(glyph_tone(None, None, true, skin), None);
        assert_eq!(
            glyph_base(GlyphStyle::Vis, skin),
            skin.color(skin.vis.icon_color)
        );

        for style in [
            GlyphStyle::Default,
            GlyphStyle::Menu,
            GlyphStyle::MenuBurger,
            GlyphStyle::MenuSmall,
            GlyphStyle::MenuCell,
        ] {
            assert_eq!(
                glyph_base(style, skin),
                skin.color(ColorRole::Text),
                "{style:?}"
            );
        }
    }
}
