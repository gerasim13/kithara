use serde::{Deserialize, Serialize};

use super::{
    palette::ColorRole,
    primitives::{FrameSkin, TextRoleSkin, ToneColors},
};
use crate::size::SizeSpec;

/// Menu icon sizes. Row geometry lives in the markup and menu typography in [`TextSkin`].
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize, kithara_derive::SkinWalk)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct MenuSkin {
    pub icon_color: ColorRole,
    pub burger_icon_size: f32,
    pub cell_icon_size: f32,
    pub icon_size: f32,
    pub small_icon_size: f32,
}

/// What a skin may restate of [`MenuSkin`].
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields)]
#[non_exhaustive]
pub struct MenuPatch {
    pub burger_icon_size: Option<f32>,
    pub cell_icon_size: Option<f32>,
    pub icon_color: Option<ColorRole>,
    pub icon_size: Option<f32>,
    pub small_icon_size: Option<f32>,
}

impl MenuSkin {
    /// Takes every field the patch restates, keeping the rest.
    pub(crate) fn patch(&mut self, patch: MenuPatch) {
        super::patch::patch_field(&mut self.icon_color, patch.icon_color);
        super::patch::patch_field(&mut self.burger_icon_size, patch.burger_icon_size);
        super::patch::patch_field(&mut self.cell_icon_size, patch.cell_icon_size);
        super::patch::patch_field(&mut self.icon_size, patch.icon_size);
        super::patch::patch_field(&mut self.small_icon_size, patch.small_icon_size);
    }
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize, kithara_derive::SkinWalk)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct SegmentedSkin {
    pub active_background: ColorRole,
    pub active_text: ColorRole,
    pub background: ColorRole,
    pub frame: FrameSkin,
    pub size: SizeSpec,
    pub text: TextRoleSkin,
    pub padding_x: f32,
}

/// What a skin may restate of [`SegmentedSkin`].
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields)]
#[non_exhaustive]
pub struct SegmentedPatch {
    pub active_background: Option<ColorRole>,
    pub active_text: Option<ColorRole>,
    pub background: Option<ColorRole>,
    pub frame: Option<FrameSkin>,
    pub padding_x: Option<f32>,
    pub size: Option<SizeSpec>,
    pub text: Option<TextRoleSkin>,
}

impl SegmentedSkin {
    /// Takes every field the patch restates, keeping the rest.
    pub(crate) fn patch(&mut self, patch: SegmentedPatch) {
        super::patch::patch_field(&mut self.active_background, patch.active_background);
        super::patch::patch_field(&mut self.active_text, patch.active_text);
        super::patch::patch_field(&mut self.background, patch.background);
        super::patch::patch_field(&mut self.text, patch.text);
        super::patch::patch_field(&mut self.frame, patch.frame);
        super::patch::patch_field(&mut self.size, patch.size);
        super::patch::patch_field(&mut self.padding_x, patch.padding_x);
    }
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize, kithara_derive::SkinWalk)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct SelectSkin {
    pub background: ColorRole,
    pub chevron_color: ColorRole,
    pub frame: FrameSkin,
    pub size: SizeSpec,
    pub text: TextRoleSkin,
    pub chevron_size: f32,
    pub padding_x: f32,
    pub padding_y: f32,
}

/// What a skin may restate of [`SelectSkin`].
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields)]
#[non_exhaustive]
pub struct SelectPatch {
    pub background: Option<ColorRole>,
    pub chevron_color: Option<ColorRole>,
    pub chevron_size: Option<f32>,
    pub frame: Option<FrameSkin>,
    pub padding_x: Option<f32>,
    pub padding_y: Option<f32>,
    pub size: Option<SizeSpec>,
    pub text: Option<TextRoleSkin>,
}

impl SelectSkin {
    /// Takes every field the patch restates, keeping the rest.
    pub(crate) fn patch(&mut self, patch: SelectPatch) {
        super::patch::patch_field(&mut self.background, patch.background);
        super::patch::patch_field(&mut self.chevron_color, patch.chevron_color);
        super::patch::patch_field(&mut self.text, patch.text);
        super::patch::patch_field(&mut self.frame, patch.frame);
        super::patch::patch_field(&mut self.size, patch.size);
        super::patch::patch_field(&mut self.chevron_size, patch.chevron_size);
        super::patch::patch_field(&mut self.padding_x, patch.padding_x);
        super::patch::patch_field(&mut self.padding_y, patch.padding_y);
    }
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize, kithara_derive::SkinWalk)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct StatusDotSkin {
    pub size: SizeSpec,
    pub text: TextRoleSkin,
    pub tones: ToneColors,
    pub dot_size: f32,
    pub gap: f32,
}

/// What a skin may restate of [`StatusDotSkin`].
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields)]
#[non_exhaustive]
pub struct StatusDotPatch {
    pub dot_size: Option<f32>,
    pub gap: Option<f32>,
    pub size: Option<SizeSpec>,
    pub text: Option<TextRoleSkin>,
    pub tones: Option<ToneColors>,
}

impl StatusDotSkin {
    /// Takes every field the patch restates, keeping the rest.
    pub(crate) fn patch(&mut self, patch: StatusDotPatch) {
        super::patch::patch_field(&mut self.tones, patch.tones);
        super::patch::patch_field(&mut self.text, patch.text);
        super::patch::patch_field(&mut self.size, patch.size);
        super::patch::patch_field(&mut self.dot_size, patch.dot_size);
        super::patch::patch_field(&mut self.gap, patch.gap);
    }
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize, kithara_derive::SkinWalk)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct SwatchSkin {
    pub frame: FrameSkin,
    pub size: SizeSpec,
    pub hex: TextRoleSkin,
    pub label: TextRoleSkin,
    pub box_height: f32,
    pub box_label_gap: f32,
    pub label_hex_gap: f32,
}

/// What a skin may restate of [`SwatchSkin`].
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields)]
#[non_exhaustive]
pub struct SwatchPatch {
    pub box_height: Option<f32>,
    pub box_label_gap: Option<f32>,
    pub frame: Option<FrameSkin>,
    pub hex: Option<TextRoleSkin>,
    pub label: Option<TextRoleSkin>,
    pub label_hex_gap: Option<f32>,
    pub size: Option<SizeSpec>,
}

impl SwatchSkin {
    /// Takes every field the patch restates, keeping the rest.
    pub(crate) fn patch(&mut self, patch: SwatchPatch) {
        super::patch::patch_field(&mut self.frame, patch.frame);
        super::patch::patch_field(&mut self.size, patch.size);
        super::patch::patch_field(&mut self.hex, patch.hex);
        super::patch::patch_field(&mut self.label, patch.label);
        super::patch::patch_field(&mut self.box_height, patch.box_height);
        super::patch::patch_field(&mut self.box_label_gap, patch.box_label_gap);
        super::patch::patch_field(&mut self.label_hex_gap, patch.label_hex_gap);
    }
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize, kithara_derive::SkinWalk)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct CellSkin {
    pub background: ColorRole,
    pub frame: FrameSkin,
    pub highlighted_frame: FrameSkin,
    pub size: SizeSpec,
    pub label_gap: f32,
    pub label_height: f32,
}

/// What a skin may restate of [`CellSkin`].
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields)]
#[non_exhaustive]
pub struct CellPatch {
    pub background: Option<ColorRole>,
    pub frame: Option<FrameSkin>,
    pub highlighted_frame: Option<FrameSkin>,
    pub label_gap: Option<f32>,
    pub label_height: Option<f32>,
    pub size: Option<SizeSpec>,
}

impl CellSkin {
    /// Takes every field the patch restates, keeping the rest.
    pub(crate) fn patch(&mut self, patch: CellPatch) {
        super::patch::patch_field(&mut self.background, patch.background);
        super::patch::patch_field(&mut self.frame, patch.frame);
        super::patch::patch_field(&mut self.highlighted_frame, patch.highlighted_frame);
        super::patch::patch_field(&mut self.size, patch.size);
        super::patch::patch_field(&mut self.label_gap, patch.label_gap);
        super::patch::patch_field(&mut self.label_height, patch.label_height);
    }
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize, kithara_derive::SkinWalk)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct FaderSkin {
    pub handle_color: ColorRole,
    pub icon_color: ColorRole,
    pub panel_color: ColorRole,
    pub rail_empty: ColorRole,
    pub rail_filled: ColorRole,
    pub segment_dim: ColorRole,
    pub segment_lit: ColorRole,
    pub tick_color: ColorRole,
    pub handle_frame: FrameSkin,
    pub rail_frame: FrameSkin,
    pub strip_frame: FrameSkin,
    pub size: SizeSpec,
    pub label: TextRoleSkin,
    pub content_gap: f32,
    pub control_height: f32,
    pub control_padding_x: f32,
    pub control_padding_y: f32,
    pub icon_size: f32,
    pub icon_width: f32,
    pub label_width: f32,
    pub rail_width: f32,
    pub segment_gap: f32,
    pub segment_height: f32,
    pub slider_height: f32,
    pub strip_height: f32,
    pub strip_padding: f32,
    pub tick_height: f32,
    pub tick_step: f32,
    pub tick_width: f32,
    pub ticks_height: f32,
    pub step: f64,
    pub handle_width: u16,
    pub segment_count: usize,
}

/// What a skin may restate of [`FaderSkin`].
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields)]
#[non_exhaustive]
pub struct FaderPatch {
    pub content_gap: Option<f32>,
    pub control_height: Option<f32>,
    pub control_padding_x: Option<f32>,
    pub control_padding_y: Option<f32>,
    pub handle_color: Option<ColorRole>,
    pub handle_frame: Option<FrameSkin>,
    pub handle_width: Option<u16>,
    pub icon_color: Option<ColorRole>,
    pub icon_size: Option<f32>,
    pub icon_width: Option<f32>,
    pub label: Option<TextRoleSkin>,
    pub label_width: Option<f32>,
    pub panel_color: Option<ColorRole>,
    pub rail_empty: Option<ColorRole>,
    pub rail_filled: Option<ColorRole>,
    pub rail_frame: Option<FrameSkin>,
    pub rail_width: Option<f32>,
    pub segment_count: Option<usize>,
    pub segment_dim: Option<ColorRole>,
    pub segment_gap: Option<f32>,
    pub segment_height: Option<f32>,
    pub segment_lit: Option<ColorRole>,
    pub size: Option<SizeSpec>,
    pub slider_height: Option<f32>,
    pub step: Option<f64>,
    pub strip_frame: Option<FrameSkin>,
    pub strip_height: Option<f32>,
    pub strip_padding: Option<f32>,
    pub tick_color: Option<ColorRole>,
    pub tick_height: Option<f32>,
    pub tick_step: Option<f32>,
    pub tick_width: Option<f32>,
    pub ticks_height: Option<f32>,
}

impl FaderSkin {
    /// Takes every field the patch restates, keeping the rest.
    pub(crate) fn patch(&mut self, patch: FaderPatch) {
        super::patch::patch_field(&mut self.handle_color, patch.handle_color);
        super::patch::patch_field(&mut self.icon_color, patch.icon_color);
        super::patch::patch_field(&mut self.panel_color, patch.panel_color);
        super::patch::patch_field(&mut self.rail_empty, patch.rail_empty);
        super::patch::patch_field(&mut self.rail_filled, patch.rail_filled);
        super::patch::patch_field(&mut self.segment_dim, patch.segment_dim);
        super::patch::patch_field(&mut self.segment_lit, patch.segment_lit);
        super::patch::patch_field(&mut self.tick_color, patch.tick_color);
        super::patch::patch_field(&mut self.label, patch.label);
        super::patch::patch_field(&mut self.handle_frame, patch.handle_frame);
        super::patch::patch_field(&mut self.rail_frame, patch.rail_frame);
        super::patch::patch_field(&mut self.strip_frame, patch.strip_frame);
        super::patch::patch_field(&mut self.size, patch.size);
        super::patch::patch_field(&mut self.content_gap, patch.content_gap);
        super::patch::patch_field(&mut self.control_height, patch.control_height);
        super::patch::patch_field(&mut self.control_padding_x, patch.control_padding_x);
        super::patch::patch_field(&mut self.control_padding_y, patch.control_padding_y);
        super::patch::patch_field(&mut self.icon_size, patch.icon_size);
        super::patch::patch_field(&mut self.icon_width, patch.icon_width);
        super::patch::patch_field(&mut self.label_width, patch.label_width);
        super::patch::patch_field(&mut self.rail_width, patch.rail_width);
        super::patch::patch_field(&mut self.segment_gap, patch.segment_gap);
        super::patch::patch_field(&mut self.segment_height, patch.segment_height);
        super::patch::patch_field(&mut self.slider_height, patch.slider_height);
        super::patch::patch_field(&mut self.strip_height, patch.strip_height);
        super::patch::patch_field(&mut self.strip_padding, patch.strip_padding);
        super::patch::patch_field(&mut self.tick_height, patch.tick_height);
        super::patch::patch_field(&mut self.tick_step, patch.tick_step);
        super::patch::patch_field(&mut self.tick_width, patch.tick_width);
        super::patch::patch_field(&mut self.ticks_height, patch.ticks_height);
        super::patch::patch_field(&mut self.step, patch.step);
        super::patch::patch_field(&mut self.handle_width, patch.handle_width);
        super::patch::patch_field(&mut self.segment_count, patch.segment_count);
    }
}
