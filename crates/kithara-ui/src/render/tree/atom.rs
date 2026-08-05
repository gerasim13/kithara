use iced::{Color, Element, widget::Space};
use num_traits::cast::AsPrimitive;

use super::geometry::active_tone;
use crate::{
    atoms::{
        design::{
            cell::Cell, meter::Meter, segmented::Segmented, select::Select, status_dot::StatusDot,
            swatch::Swatch,
        },
        readout::Readout,
        toggle::Binary,
    },
    compile::CompiledUi,
    ids::InternId,
    module::{ChipStyle, FaderStyle, GlyphStyle, IconName, Tone},
    render::{
        IcedSkin, InputOwner, ReadValue, Skin, UiEvent,
        controls::{Gesture, KnobPaint, KnobProgram, Paint},
        icons::document_icon,
    },
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

pub(super) fn chip<'a>(
    path: &'a str,
    label: &'a str,
    style: ChipStyle,
    value: Option<&ReadValue<'_>>,
    skin: &'a Skin,
    owner: InputOwner,
) -> Element<'a, UiEvent> {
    crate::render::controls::chip(path, label, style, value, skin, owner)
}

pub(super) fn toggle<'a>(
    path: &'a str,
    value: Option<&ReadValue<'_>>,
    skin: &'a Skin,
    owner: InputOwner,
) -> Element<'a, UiEvent> {
    binary(Binary::toggle(skin), path, value, skin, owner)
}

pub(super) fn checkbox<'a>(
    path: &'a str,
    value: Option<&ReadValue<'_>>,
    skin: &'a Skin,
    owner: InputOwner,
) -> Element<'a, UiEvent> {
    binary(Binary::checkbox(skin), path, value, skin, owner)
}

/// A switch draws nothing at all until its endpoint says which way it is set.
fn binary<'a>(
    painter: Binary,
    path: &'a str,
    value: Option<&ReadValue<'_>>,
    skin: &'a Skin,
    owner: InputOwner,
) -> Element<'a, UiEvent> {
    let Some(ReadValue::Bool(active)) = value else {
        return Space::new().into();
    };
    let paint = Paint::new(painter, *active, skin);
    match owner {
        InputOwner::Leaf => Gesture::press(path, paint).view(),
        InputOwner::Engine => paint.view(),
    }
}

pub(super) fn vu_stereo<'a>(
    path: &'a str,
    value: Option<&ReadValue<'_>>,
    skin: &'a Skin,
    owner: InputOwner,
) -> Element<'a, UiEvent> {
    crate::render::controls::vu_stereo(path, value, skin, owner)
}

pub(super) fn vu_vertical<'a>(
    path: &'a str,
    ticks: bool,
    value: Option<&ReadValue<'_>>,
    skin: &'a Skin,
    owner: InputOwner,
) -> Element<'a, UiEvent> {
    crate::render::controls::vu_vertical(path, ticks, value, skin, owner)
}

pub(super) fn meter<'a>(value: Option<&ReadValue<'_>>, skin: &'a Skin) -> Element<'a, UiEvent> {
    let level = match value {
        Some(ReadValue::Scalar(level)) => level.clamp(0.0, 1.0).as_(),
        _ => 0.0,
    };
    Paint::new(Meter::new(skin), level, skin).view()
}

pub(super) fn knob<'a>(
    path: &'a str,
    label: Option<&'a str>,
    value: Option<&ReadValue<'_>>,
    skin: &'a Skin,
    owner: InputOwner,
) -> Element<'a, UiEvent> {
    let Some(ReadValue::Scalar(value)) = value else {
        return Space::new().into();
    };
    let value = value.clamp(0.0, 1.0).as_();
    match owner {
        InputOwner::Leaf => KnobProgram::new(path, label, value, skin).view(),
        InputOwner::Engine => KnobPaint::new(label, value, skin).view(),
    }
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

pub(super) fn swatch<'a>(
    role: ColorRole,
    label: InternId,
    ui: &'a CompiledUi,
    skin: &Skin,
) -> Element<'a, UiEvent> {
    Swatch::builder()
        .role(role)
        .label(ui.resolve(label))
        .skin(skin)
        .build()
        .view()
}

pub(super) fn status_dot<'a>(
    label: InternId,
    tone: Tone,
    ui: &'a CompiledUi,
    skin: &'a Skin,
) -> Element<'a, UiEvent> {
    Paint::new(
        StatusDot::new(tone, skin),
        ui.resolve(label).to_owned(),
        skin,
    )
    .view()
}

pub(super) fn cell<'a>(
    label: Option<InternId>,
    highlighted: bool,
    ui: &'a CompiledUi,
    skin: &Skin,
) -> Element<'a, UiEvent> {
    Cell::builder()
        .maybe_label(label.map(|id| ui.resolve(id)))
        .highlighted(highlighted)
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

pub(super) fn nav_item<'a>(
    path: &'a str,
    label: &'a str,
    icon: IconName,
    value: Option<&ReadValue<'_>>,
    skin: &'a Skin,
    owner: InputOwner,
) -> Element<'a, UiEvent> {
    crate::render::controls::nav_item(path, label, document_icon(icon), value, skin, owner)
}

pub(super) fn tab_large<'a>(
    path: &'a str,
    label: &'a str,
    value: Option<&ReadValue<'_>>,
    skin: &'a Skin,
    owner: InputOwner,
) -> Element<'a, UiEvent> {
    crate::render::controls::tab_large(path, label, value, skin, owner)
}

#[cfg(test)]
mod tests {
    use iced::advanced::widget::Tree;
    use kithara_test_utils::kithara;

    use super::*;
    use crate::builtin;

    #[kithara::test]
    fn engine_owned_knob_selects_the_paint_only_program() {
        let skin = builtin::skin();
        let value = ReadValue::Scalar(0.5);
        let hosted = knob(
            "mixer/gain",
            Some("GAIN"),
            Some(&value),
            skin,
            InputOwner::Engine,
        );
        let painted = KnobPaint::new(Some("GAIN"), 0.5, skin).view();
        let interactive = KnobProgram::new("mixer/gain", Some("GAIN"), 0.5, skin).view();
        let hosted_tree = Tree::new(hosted.as_widget());
        let painted_tree = Tree::new(painted.as_widget());
        let interactive_tree = Tree::new(interactive.as_widget());

        assert!(
            hosted_tree.tag == painted_tree.tag,
            "InputOwner::Engine must select the paint-only knob program"
        );
        assert!(
            hosted_tree.tag != interactive_tree.tag,
            "the hosted knob must not retain the leaf gesture state"
        );
    }

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
