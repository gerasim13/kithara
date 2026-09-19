use serde::{Deserialize, Serialize};

pub use super::controls_secondary::*;
use super::{
    palette::ColorRole,
    primitives::{FaceSkin, FrameSkin, StateColors, TextRoleSkin, TickSkin, ToneColors},
};
use crate::{
    layout::FrameSides,
    module::{TextStyle, text_roles},
    size::SizeSpec,
};

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize, kithara_derive::SkinWalk)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct KnobSkin {
    pub body_border: ColorRole,
    pub body_fill: ColorRole,
    pub indicator_color: ColorRole,
    pub track_color: ColorRole,
    pub value_color: ColorRole,
    pub size: SizeSpec,
    pub label_text: TextRoleSkin,
    pub body_border_width: f32,
    pub body_ratio: f32,
    pub drag_range: f32,
    pub indicator_width: f32,
    pub label_gap: f32,
    pub label_height: f32,
    pub neutral_angle: f32,
    pub outer_inset: f32,
    pub start_angle: f32,
    pub sweep_angle: f32,
    pub track_alpha: f32,
    pub track_width: f32,
    pub wheel_step: f32,
}

/// What a skin may restate of [`KnobSkin`].
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields)]
#[non_exhaustive]
pub struct KnobPatch {
    pub body_border: Option<ColorRole>,
    pub body_fill: Option<ColorRole>,
    pub indicator_color: Option<ColorRole>,
    pub track_color: Option<ColorRole>,
    pub value_color: Option<ColorRole>,
    pub size: Option<SizeSpec>,
    pub label_text: Option<TextRoleSkin>,
    pub body_border_width: Option<f32>,
    pub body_ratio: Option<f32>,
    pub drag_range: Option<f32>,
    pub indicator_width: Option<f32>,
    pub label_gap: Option<f32>,
    pub label_height: Option<f32>,
    pub neutral_angle: Option<f32>,
    pub outer_inset: Option<f32>,
    pub start_angle: Option<f32>,
    pub sweep_angle: Option<f32>,
    pub track_alpha: Option<f32>,
    pub track_width: Option<f32>,
    pub wheel_step: Option<f32>,
}

impl KnobSkin {
    /// Takes every field the patch restates, keeping the rest.
    pub(crate) fn patch(&mut self, patch: KnobPatch) {
        super::patch::patch_field(&mut self.body_border, patch.body_border);
        super::patch::patch_field(&mut self.body_fill, patch.body_fill);
        super::patch::patch_field(&mut self.indicator_color, patch.indicator_color);
        super::patch::patch_field(&mut self.track_color, patch.track_color);
        super::patch::patch_field(&mut self.value_color, patch.value_color);
        super::patch::patch_field(&mut self.size, patch.size);
        super::patch::patch_field(&mut self.label_text, patch.label_text);
        super::patch::patch_field(&mut self.body_border_width, patch.body_border_width);
        super::patch::patch_field(&mut self.body_ratio, patch.body_ratio);
        super::patch::patch_field(&mut self.drag_range, patch.drag_range);
        super::patch::patch_field(&mut self.indicator_width, patch.indicator_width);
        super::patch::patch_field(&mut self.label_gap, patch.label_gap);
        super::patch::patch_field(&mut self.label_height, patch.label_height);
        super::patch::patch_field(&mut self.neutral_angle, patch.neutral_angle);
        super::patch::patch_field(&mut self.outer_inset, patch.outer_inset);
        super::patch::patch_field(&mut self.start_angle, patch.start_angle);
        super::patch::patch_field(&mut self.sweep_angle, patch.sweep_angle);
        super::patch::patch_field(&mut self.track_alpha, patch.track_alpha);
        super::patch::patch_field(&mut self.track_width, patch.track_width);
        super::patch::patch_field(&mut self.wheel_step, patch.wheel_step);
    }
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize, kithara_derive::SkinWalk)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct CrossfaderSkin {
    pub arrow_color: ColorRole,
    pub rail_background: ColorRole,
    pub thumb_color: ColorRole,
    pub thumb_notch_color: ColorRole,
    pub label_text: TextRoleSkin,
    pub letter_text: TextRoleSkin,
    pub rail_frame: FrameSkin,
    pub size: SizeSpec,
    pub ticks: TickSkin,
    pub arrow_gap: f32,
    pub arrow_size: f32,
    pub label_gap: f32,
    pub padding_bottom: f32,
    pub padding_top: f32,
    pub padding_x: f32,
    pub rail_height: f32,
    pub thumb_height: f32,
    pub thumb_notch_height: f32,
    pub thumb_notch_width: f32,
    pub thumb_width: f32,
}

/// What a skin may restate of [`CrossfaderSkin`].
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields)]
#[non_exhaustive]
pub struct CrossfaderPatch {
    pub arrow_color: Option<ColorRole>,
    pub rail_background: Option<ColorRole>,
    pub thumb_color: Option<ColorRole>,
    pub thumb_notch_color: Option<ColorRole>,
    pub label_text: Option<TextRoleSkin>,
    pub letter_text: Option<TextRoleSkin>,
    pub rail_frame: Option<FrameSkin>,
    pub size: Option<SizeSpec>,
    pub ticks: Option<TickSkin>,
    pub arrow_gap: Option<f32>,
    pub arrow_size: Option<f32>,
    pub label_gap: Option<f32>,
    pub padding_bottom: Option<f32>,
    pub padding_top: Option<f32>,
    pub padding_x: Option<f32>,
    pub rail_height: Option<f32>,
    pub thumb_height: Option<f32>,
    pub thumb_notch_height: Option<f32>,
    pub thumb_notch_width: Option<f32>,
    pub thumb_width: Option<f32>,
}

impl CrossfaderSkin {
    /// Takes every field the patch restates, keeping the rest.
    pub(crate) fn patch(&mut self, patch: CrossfaderPatch) {
        super::patch::patch_field(&mut self.arrow_color, patch.arrow_color);
        super::patch::patch_field(&mut self.rail_background, patch.rail_background);
        super::patch::patch_field(&mut self.thumb_color, patch.thumb_color);
        super::patch::patch_field(&mut self.thumb_notch_color, patch.thumb_notch_color);
        super::patch::patch_field(&mut self.label_text, patch.label_text);
        super::patch::patch_field(&mut self.letter_text, patch.letter_text);
        super::patch::patch_field(&mut self.rail_frame, patch.rail_frame);
        super::patch::patch_field(&mut self.size, patch.size);
        super::patch::patch_field(&mut self.ticks, patch.ticks);
        super::patch::patch_field(&mut self.arrow_gap, patch.arrow_gap);
        super::patch::patch_field(&mut self.arrow_size, patch.arrow_size);
        super::patch::patch_field(&mut self.label_gap, patch.label_gap);
        super::patch::patch_field(&mut self.padding_bottom, patch.padding_bottom);
        super::patch::patch_field(&mut self.padding_top, patch.padding_top);
        super::patch::patch_field(&mut self.padding_x, patch.padding_x);
        super::patch::patch_field(&mut self.rail_height, patch.rail_height);
        super::patch::patch_field(&mut self.thumb_height, patch.thumb_height);
        super::patch::patch_field(&mut self.thumb_notch_height, patch.thumb_notch_height);
        super::patch::patch_field(&mut self.thumb_notch_width, patch.thumb_notch_width);
        super::patch::patch_field(&mut self.thumb_width, patch.thumb_width);
    }
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize, kithara_derive::SkinWalk)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct VuStereoSkin {
    pub size: SizeSpec,
    pub carriage_width: f32,
    pub channel_l_y: f32,
    pub channel_r_y: f32,
    pub danger_threshold: f32,
    pub segment_gap: f32,
    pub segment_height: f32,
    pub segment_width: f32,
    pub warning_threshold: f32,
    pub segment_count: usize,
}

/// What a skin may restate of [`VuStereoSkin`].
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields)]
#[non_exhaustive]
pub struct VuStereoPatch {
    pub size: Option<SizeSpec>,
    pub carriage_width: Option<f32>,
    pub channel_l_y: Option<f32>,
    pub channel_r_y: Option<f32>,
    pub danger_threshold: Option<f32>,
    pub segment_gap: Option<f32>,
    pub segment_height: Option<f32>,
    pub segment_width: Option<f32>,
    pub warning_threshold: Option<f32>,
    pub segment_count: Option<usize>,
}

impl VuStereoSkin {
    /// Takes every field the patch restates, keeping the rest.
    pub(crate) fn patch(&mut self, patch: VuStereoPatch) {
        super::patch::patch_field(&mut self.size, patch.size);
        super::patch::patch_field(&mut self.carriage_width, patch.carriage_width);
        super::patch::patch_field(&mut self.channel_l_y, patch.channel_l_y);
        super::patch::patch_field(&mut self.channel_r_y, patch.channel_r_y);
        super::patch::patch_field(&mut self.danger_threshold, patch.danger_threshold);
        super::patch::patch_field(&mut self.segment_gap, patch.segment_gap);
        super::patch::patch_field(&mut self.segment_height, patch.segment_height);
        super::patch::patch_field(&mut self.segment_width, patch.segment_width);
        super::patch::patch_field(&mut self.warning_threshold, patch.warning_threshold);
        super::patch::patch_field(&mut self.segment_count, patch.segment_count);
    }
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize, kithara_derive::SkinWalk)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct VuVerticalSkin {
    pub thumb_color: ColorRole,
    pub thumb_notch_color: ColorRole,
    pub size: SizeSpec,
    pub ticks: TickSkin,
    pub danger_threshold: f32,
    pub fader_width: f32,
    pub segment_gap: f32,
    pub segment_height: f32,
    pub segment_inset_x: f32,
    pub thumb_height: f32,
    pub thumb_notch_height: f32,
    pub thumb_notch_offset: f32,
    pub warning_threshold: f32,
}

/// What a skin may restate of [`VuVerticalSkin`].
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields)]
#[non_exhaustive]
pub struct VuVerticalPatch {
    pub thumb_color: Option<ColorRole>,
    pub thumb_notch_color: Option<ColorRole>,
    pub size: Option<SizeSpec>,
    pub ticks: Option<TickSkin>,
    pub danger_threshold: Option<f32>,
    pub fader_width: Option<f32>,
    pub segment_gap: Option<f32>,
    pub segment_height: Option<f32>,
    pub segment_inset_x: Option<f32>,
    pub thumb_height: Option<f32>,
    pub thumb_notch_height: Option<f32>,
    pub thumb_notch_offset: Option<f32>,
    pub warning_threshold: Option<f32>,
}

impl VuVerticalSkin {
    /// Takes every field the patch restates, keeping the rest.
    pub(crate) fn patch(&mut self, patch: VuVerticalPatch) {
        super::patch::patch_field(&mut self.thumb_color, patch.thumb_color);
        super::patch::patch_field(&mut self.thumb_notch_color, patch.thumb_notch_color);
        super::patch::patch_field(&mut self.size, patch.size);
        super::patch::patch_field(&mut self.ticks, patch.ticks);
        super::patch::patch_field(&mut self.danger_threshold, patch.danger_threshold);
        super::patch::patch_field(&mut self.fader_width, patch.fader_width);
        super::patch::patch_field(&mut self.segment_gap, patch.segment_gap);
        super::patch::patch_field(&mut self.segment_height, patch.segment_height);
        super::patch::patch_field(&mut self.segment_inset_x, patch.segment_inset_x);
        super::patch::patch_field(&mut self.thumb_height, patch.thumb_height);
        super::patch::patch_field(&mut self.thumb_notch_height, patch.thumb_notch_height);
        super::patch::patch_field(&mut self.thumb_notch_offset, patch.thumb_notch_offset);
        super::patch::patch_field(&mut self.warning_threshold, patch.warning_threshold);
    }
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize, kithara_derive::SkinWalk)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct VisSkin {
    pub icon_color: ColorRole,
    pub nav_fill: StateColors,
    pub nav_text: TextRoleSkin,
    pub nav_frame: FrameSkin,
    pub size: SizeSpec,
    pub footer_height: f32,
    pub footer_padding_x: f32,
    pub header_height: f32,
    pub icon_size: f32,
    pub index_padding_x: f32,
    pub name_padding_x: f32,
    pub nav_cell_size: f32,
    pub nav_padding_x: f32,
    pub nav_padding_y: f32,
}

/// What a skin may restate of [`VisSkin`].
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields)]
#[non_exhaustive]
pub struct VisPatch {
    pub icon_color: Option<ColorRole>,
    pub nav_fill: Option<StateColors>,
    pub nav_text: Option<TextRoleSkin>,
    pub nav_frame: Option<FrameSkin>,
    pub size: Option<SizeSpec>,
    pub footer_height: Option<f32>,
    pub footer_padding_x: Option<f32>,
    pub header_height: Option<f32>,
    pub icon_size: Option<f32>,
    pub index_padding_x: Option<f32>,
    pub name_padding_x: Option<f32>,
    pub nav_cell_size: Option<f32>,
    pub nav_padding_x: Option<f32>,
    pub nav_padding_y: Option<f32>,
}

impl VisSkin {
    /// Takes every field the patch restates, keeping the rest.
    pub(crate) fn patch(&mut self, patch: VisPatch) {
        super::patch::patch_field(&mut self.icon_color, patch.icon_color);
        super::patch::patch_field(&mut self.nav_fill, patch.nav_fill);
        super::patch::patch_field(&mut self.nav_text, patch.nav_text);
        super::patch::patch_field(&mut self.nav_frame, patch.nav_frame);
        super::patch::patch_field(&mut self.size, patch.size);
        super::patch::patch_field(&mut self.footer_height, patch.footer_height);
        super::patch::patch_field(&mut self.footer_padding_x, patch.footer_padding_x);
        super::patch::patch_field(&mut self.header_height, patch.header_height);
        super::patch::patch_field(&mut self.icon_size, patch.icon_size);
        super::patch::patch_field(&mut self.index_padding_x, patch.index_padding_x);
        super::patch::patch_field(&mut self.name_padding_x, patch.name_padding_x);
        super::patch::patch_field(&mut self.nav_cell_size, patch.nav_cell_size);
        super::patch::patch_field(&mut self.nav_padding_x, patch.nav_padding_x);
        super::patch::patch_field(&mut self.nav_padding_y, patch.nav_padding_y);
    }
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize, kithara_derive::SkinWalk)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct PortalMapSkin {
    pub arc_color: ColorRole,
    pub arc_selected_color: ColorRole,
    pub axis_color: ColorRole,
    pub background_color: ColorRole,
    pub master_color: ColorRole,
    pub target_color: ColorRole,
    pub tick_color: ColorRole,
    pub size: SizeSpec,
    pub axis_inset_x: f32,
    pub axis_offset_bottom: f32,
    pub arc_height_scale: f32,
    pub arc_top_inset: f32,
    pub line_width: f32,
    pub selected_line_width: f32,
    pub marker_size: f32,
    pub tick_height: f32,
    pub tick_step: f32,
    pub label_offset_x: f32,
    pub label_offset_y: f32,
    pub label: TextRoleSkin,
}

/// What a skin may restate of [`PortalMapSkin`].
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields)]
#[non_exhaustive]
pub struct PortalMapPatch {
    pub arc_color: Option<ColorRole>,
    pub arc_selected_color: Option<ColorRole>,
    pub axis_color: Option<ColorRole>,
    pub background_color: Option<ColorRole>,
    pub master_color: Option<ColorRole>,
    pub target_color: Option<ColorRole>,
    pub tick_color: Option<ColorRole>,
    pub size: Option<SizeSpec>,
    pub axis_inset_x: Option<f32>,
    pub axis_offset_bottom: Option<f32>,
    pub arc_height_scale: Option<f32>,
    pub arc_top_inset: Option<f32>,
    pub line_width: Option<f32>,
    pub selected_line_width: Option<f32>,
    pub marker_size: Option<f32>,
    pub tick_height: Option<f32>,
    pub tick_step: Option<f32>,
    pub label_offset_x: Option<f32>,
    pub label_offset_y: Option<f32>,
    pub label: Option<TextRoleSkin>,
}

impl PortalMapSkin {
    /// Takes every field the patch restates, keeping the rest.
    pub(crate) fn patch(&mut self, patch: PortalMapPatch) {
        super::patch::patch_field(&mut self.arc_color, patch.arc_color);
        super::patch::patch_field(&mut self.arc_selected_color, patch.arc_selected_color);
        super::patch::patch_field(&mut self.axis_color, patch.axis_color);
        super::patch::patch_field(&mut self.background_color, patch.background_color);
        super::patch::patch_field(&mut self.master_color, patch.master_color);
        super::patch::patch_field(&mut self.target_color, patch.target_color);
        super::patch::patch_field(&mut self.tick_color, patch.tick_color);
        super::patch::patch_field(&mut self.size, patch.size);
        super::patch::patch_field(&mut self.axis_inset_x, patch.axis_inset_x);
        super::patch::patch_field(&mut self.axis_offset_bottom, patch.axis_offset_bottom);
        super::patch::patch_field(&mut self.arc_height_scale, patch.arc_height_scale);
        super::patch::patch_field(&mut self.arc_top_inset, patch.arc_top_inset);
        super::patch::patch_field(&mut self.line_width, patch.line_width);
        super::patch::patch_field(&mut self.selected_line_width, patch.selected_line_width);
        super::patch::patch_field(&mut self.marker_size, patch.marker_size);
        super::patch::patch_field(&mut self.tick_height, patch.tick_height);
        super::patch::patch_field(&mut self.tick_step, patch.tick_step);
        super::patch::patch_field(&mut self.label_offset_x, patch.label_offset_x);
        super::patch::patch_field(&mut self.label_offset_y, patch.label_offset_y);
        super::patch::patch_field(&mut self.label, patch.label);
    }
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize, kithara_derive::SkinWalk)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct RangeSkin {
    pub rail_background: ColorRole,
    pub selection_color: ColorRole,
    pub size: SizeSpec,
    pub thumb_color: ColorRole,
    pub rail_height: f32,
    pub thumb_height: f32,
    pub thumb_width: f32,
}

/// What a skin may restate of [`RangeSkin`].
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields)]
#[non_exhaustive]
pub struct RangePatch {
    pub rail_background: Option<ColorRole>,
    pub selection_color: Option<ColorRole>,
    pub size: Option<SizeSpec>,
    pub thumb_color: Option<ColorRole>,
    pub rail_height: Option<f32>,
    pub thumb_height: Option<f32>,
    pub thumb_width: Option<f32>,
}

impl RangeSkin {
    /// Takes every field the patch restates, keeping the rest.
    pub(crate) fn patch(&mut self, patch: RangePatch) {
        super::patch::patch_field(&mut self.rail_background, patch.rail_background);
        super::patch::patch_field(&mut self.selection_color, patch.selection_color);
        super::patch::patch_field(&mut self.size, patch.size);
        super::patch::patch_field(&mut self.thumb_color, patch.thumb_color);
        super::patch::patch_field(&mut self.rail_height, patch.rail_height);
        super::patch::patch_field(&mut self.thumb_height, patch.thumb_height);
        super::patch::patch_field(&mut self.thumb_width, patch.thumb_width);
    }
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize, kithara_derive::SkinWalk)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ToggleSkin {
    pub active: FaceSkin,
    pub idle: FaceSkin,
    pub active_frame: FrameSkin,
    pub inactive_frame: FrameSkin,
    pub size: SizeSpec,
    pub thumb_inset: f32,
    pub thumb_radius: f32,
    pub thumb_size: f32,
}

/// What a skin may restate of [`ToggleSkin`].
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields)]
#[non_exhaustive]
pub struct TogglePatch {
    pub active: Option<FaceSkin>,
    pub idle: Option<FaceSkin>,
    pub active_frame: Option<FrameSkin>,
    pub inactive_frame: Option<FrameSkin>,
    pub size: Option<SizeSpec>,
    pub thumb_inset: Option<f32>,
    pub thumb_radius: Option<f32>,
    pub thumb_size: Option<f32>,
}

impl ToggleSkin {
    /// Takes every field the patch restates, keeping the rest.
    pub(crate) fn patch(&mut self, patch: TogglePatch) {
        super::patch::patch_field(&mut self.active, patch.active);
        super::patch::patch_field(&mut self.idle, patch.idle);
        super::patch::patch_field(&mut self.active_frame, patch.active_frame);
        super::patch::patch_field(&mut self.inactive_frame, patch.inactive_frame);
        super::patch::patch_field(&mut self.size, patch.size);
        super::patch::patch_field(&mut self.thumb_inset, patch.thumb_inset);
        super::patch::patch_field(&mut self.thumb_radius, patch.thumb_radius);
        super::patch::patch_field(&mut self.thumb_size, patch.thumb_size);
    }
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize, kithara_derive::SkinWalk)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct CheckboxSkin {
    pub active: FaceSkin,
    pub idle: FaceSkin,
    pub active_frame: FrameSkin,
    pub inactive_frame: FrameSkin,
    pub size: SizeSpec,
}

/// What a skin may restate of [`CheckboxSkin`].
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields)]
#[non_exhaustive]
pub struct CheckboxPatch {
    pub active: Option<FaceSkin>,
    pub idle: Option<FaceSkin>,
    pub active_frame: Option<FrameSkin>,
    pub inactive_frame: Option<FrameSkin>,
    pub size: Option<SizeSpec>,
}

impl CheckboxSkin {
    /// Takes every field the patch restates, keeping the rest.
    pub(crate) fn patch(&mut self, patch: CheckboxPatch) {
        super::patch::patch_field(&mut self.active, patch.active);
        super::patch::patch_field(&mut self.idle, patch.idle);
        super::patch::patch_field(&mut self.active_frame, patch.active_frame);
        super::patch::patch_field(&mut self.inactive_frame, patch.inactive_frame);
        super::patch::patch_field(&mut self.size, patch.size);
    }
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize, kithara_derive::SkinWalk)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ReadoutSkin {
    pub tones: ToneColors,
    pub label: TextRoleSkin,
    pub value: TextRoleSkin,
    pub frame: FrameSkin,
    pub size: SizeSpec,
    pub padding_x: f32,
    pub padding_y: f32,
    pub spacing: f32,
}

/// What a skin may restate of [`ReadoutSkin`].
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields)]
#[non_exhaustive]
pub struct ReadoutPatch {
    pub tones: Option<ToneColors>,
    pub label: Option<TextRoleSkin>,
    pub value: Option<TextRoleSkin>,
    pub frame: Option<FrameSkin>,
    pub size: Option<SizeSpec>,
    pub padding_x: Option<f32>,
    pub padding_y: Option<f32>,
    pub spacing: Option<f32>,
}

impl ReadoutSkin {
    /// Takes every field the patch restates, keeping the rest.
    pub(crate) fn patch(&mut self, patch: ReadoutPatch) {
        super::patch::patch_field(&mut self.tones, patch.tones);
        super::patch::patch_field(&mut self.label, patch.label);
        super::patch::patch_field(&mut self.value, patch.value);
        super::patch::patch_field(&mut self.frame, patch.frame);
        super::patch::patch_field(&mut self.size, patch.size);
        super::patch::patch_field(&mut self.padding_x, patch.padding_x);
        super::patch::patch_field(&mut self.padding_y, patch.padding_y);
        super::patch::patch_field(&mut self.spacing, patch.spacing);
    }
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize, kithara_derive::SkinWalk)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ChipSkin {
    pub deck_text: TextRoleSkin,
    pub routing_text: TextRoleSkin,
    pub active: FaceSkin,
    pub idle: FaceSkin,
    /// What a pivot chip outlines itself with, whichever face it wears.
    pub pivot_border: ColorRole,
    pub pivot_family_text: TextRoleSkin,
    pub pivot_multiplier_text: TextRoleSkin,
    pub active_frame: FrameSkin,
    pub inactive_frame: FrameSkin,
    pub pivot_frame: FrameSkin,
    pub size: SizeSpec,
    pub padding_x: f32,
    pub padding_y: f32,
    pub pivot_family_padding_x: f32,
    pub pivot_family_padding_y: f32,
    pub pivot_multiplier_padding_x: f32,
    pub pivot_multiplier_padding_y: f32,
}

/// What a skin may restate of [`ChipSkin`].
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields)]
#[non_exhaustive]
pub struct ChipPatch {
    pub deck_text: Option<TextRoleSkin>,
    pub routing_text: Option<TextRoleSkin>,
    pub active: Option<FaceSkin>,
    pub idle: Option<FaceSkin>,
    pub pivot_border: Option<ColorRole>,
    pub pivot_family_text: Option<TextRoleSkin>,
    pub pivot_multiplier_text: Option<TextRoleSkin>,
    pub active_frame: Option<FrameSkin>,
    pub inactive_frame: Option<FrameSkin>,
    pub pivot_frame: Option<FrameSkin>,
    pub size: Option<SizeSpec>,
    pub padding_x: Option<f32>,
    pub padding_y: Option<f32>,
    pub pivot_family_padding_x: Option<f32>,
    pub pivot_family_padding_y: Option<f32>,
    pub pivot_multiplier_padding_x: Option<f32>,
    pub pivot_multiplier_padding_y: Option<f32>,
}

impl ChipSkin {
    /// Takes every field the patch restates, keeping the rest.
    pub(crate) fn patch(&mut self, patch: ChipPatch) {
        super::patch::patch_field(&mut self.deck_text, patch.deck_text);
        super::patch::patch_field(&mut self.routing_text, patch.routing_text);
        super::patch::patch_field(&mut self.active, patch.active);
        super::patch::patch_field(&mut self.idle, patch.idle);
        super::patch::patch_field(&mut self.pivot_border, patch.pivot_border);
        super::patch::patch_field(&mut self.pivot_family_text, patch.pivot_family_text);
        super::patch::patch_field(&mut self.pivot_multiplier_text, patch.pivot_multiplier_text);
        super::patch::patch_field(&mut self.active_frame, patch.active_frame);
        super::patch::patch_field(&mut self.inactive_frame, patch.inactive_frame);
        super::patch::patch_field(&mut self.pivot_frame, patch.pivot_frame);
        super::patch::patch_field(&mut self.size, patch.size);
        super::patch::patch_field(&mut self.padding_x, patch.padding_x);
        super::patch::patch_field(&mut self.padding_y, patch.padding_y);
        super::patch::patch_field(
            &mut self.pivot_family_padding_x,
            patch.pivot_family_padding_x,
        );
        super::patch::patch_field(
            &mut self.pivot_family_padding_y,
            patch.pivot_family_padding_y,
        );
        super::patch::patch_field(
            &mut self.pivot_multiplier_padding_x,
            patch.pivot_multiplier_padding_x,
        );
        super::patch::patch_field(
            &mut self.pivot_multiplier_padding_y,
            patch.pivot_multiplier_padding_y,
        );
    }
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize, kithara_derive::SkinWalk)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ButtonSkin {
    pub primary_text: TextRoleSkin,
    pub text: TextRoleSkin,
    /// What a button that is present but not asking to be read draws its
    /// word and its mark in.
    pub dim_text_color: ColorRole,
    pub fill: StateColors,
    pub primary_fill: StateColors,
    pub transport_fill: StateColors,
    /// A transport cell draws no border of its own; these sides say where the
    /// seam between neighbouring cells goes.
    pub transport_sides: FrameSides,
    pub frame: FrameSkin,
    pub primary_frame: FrameSkin,
    pub size: SizeSpec,
    pub icon_gap: f32,
    pub icon_size: f32,
    pub micro_icon_size: f32,
    pub micro_size: f32,
    pub padding_x: f32,
    pub padding_y: f32,
    pub transport_icon_size: f32,
    pub primary_portion: u16,
    pub transport_portion: u16,
}

/// What a skin may restate of [`ButtonSkin`].
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields)]
#[non_exhaustive]
pub struct ButtonPatch {
    pub primary_text: Option<TextRoleSkin>,
    pub text: Option<TextRoleSkin>,
    pub dim_text_color: Option<ColorRole>,
    pub fill: Option<StateColors>,
    pub primary_fill: Option<StateColors>,
    pub transport_fill: Option<StateColors>,
    pub transport_sides: Option<FrameSides>,
    pub frame: Option<FrameSkin>,
    pub primary_frame: Option<FrameSkin>,
    pub size: Option<SizeSpec>,
    pub icon_gap: Option<f32>,
    pub icon_size: Option<f32>,
    pub micro_icon_size: Option<f32>,
    pub micro_size: Option<f32>,
    pub padding_x: Option<f32>,
    pub padding_y: Option<f32>,
    pub transport_icon_size: Option<f32>,
    pub primary_portion: Option<u16>,
    pub transport_portion: Option<u16>,
}

impl ButtonSkin {
    /// Takes every field the patch restates, keeping the rest.
    pub(crate) fn patch(&mut self, patch: ButtonPatch) {
        super::patch::patch_field(&mut self.primary_text, patch.primary_text);
        super::patch::patch_field(&mut self.text, patch.text);
        super::patch::patch_field(&mut self.dim_text_color, patch.dim_text_color);
        super::patch::patch_field(&mut self.fill, patch.fill);
        super::patch::patch_field(&mut self.primary_fill, patch.primary_fill);
        super::patch::patch_field(&mut self.transport_fill, patch.transport_fill);
        super::patch::patch_field(&mut self.transport_sides, patch.transport_sides);
        super::patch::patch_field(&mut self.frame, patch.frame);
        super::patch::patch_field(&mut self.primary_frame, patch.primary_frame);
        super::patch::patch_field(&mut self.size, patch.size);
        super::patch::patch_field(&mut self.icon_gap, patch.icon_gap);
        super::patch::patch_field(&mut self.icon_size, patch.icon_size);
        super::patch::patch_field(&mut self.micro_icon_size, patch.micro_icon_size);
        super::patch::patch_field(&mut self.micro_size, patch.micro_size);
        super::patch::patch_field(&mut self.padding_x, patch.padding_x);
        super::patch::patch_field(&mut self.padding_y, patch.padding_y);
        super::patch::patch_field(&mut self.transport_icon_size, patch.transport_icon_size);
        super::patch::patch_field(&mut self.primary_portion, patch.primary_portion);
        super::patch::patch_field(&mut self.transport_portion, patch.transport_portion);
    }
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize, kithara_derive::SkinWalk)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct NavSkin {
    pub header_icon_color: ColorRole,
    pub header_height: f32,
    pub header_icon_size: f32,
    pub header_text_size: f32,
    pub icon_gap: f32,
    pub icon_size: f32,
    pub item_height: f32,
    pub marker_width: f32,
    pub pad_y: f32,
    pub text_pad_x: f32,
    pub text: TextRoleSkin,
    /// What the row the reader is on paints behind itself, and the bar it
    /// carries on its edge.
    pub selected_fill: ColorRole,
    pub marker_color: ColorRole,
    pub idle_text_color: ColorRole,
}

/// What a skin may restate of [`NavSkin`].
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields)]
#[non_exhaustive]
pub struct NavPatch {
    pub header_icon_color: Option<ColorRole>,
    pub header_height: Option<f32>,
    pub header_icon_size: Option<f32>,
    pub header_text_size: Option<f32>,
    pub icon_gap: Option<f32>,
    pub icon_size: Option<f32>,
    pub item_height: Option<f32>,
    pub marker_width: Option<f32>,
    pub pad_y: Option<f32>,
    pub text_pad_x: Option<f32>,
    pub text: Option<TextRoleSkin>,
    pub selected_fill: Option<ColorRole>,
    pub marker_color: Option<ColorRole>,
    pub idle_text_color: Option<ColorRole>,
}

impl NavSkin {
    /// Takes every field the patch restates, keeping the rest.
    pub(crate) fn patch(&mut self, patch: NavPatch) {
        super::patch::patch_field(&mut self.header_icon_color, patch.header_icon_color);
        super::patch::patch_field(&mut self.header_height, patch.header_height);
        super::patch::patch_field(&mut self.header_icon_size, patch.header_icon_size);
        super::patch::patch_field(&mut self.header_text_size, patch.header_text_size);
        super::patch::patch_field(&mut self.icon_gap, patch.icon_gap);
        super::patch::patch_field(&mut self.icon_size, patch.icon_size);
        super::patch::patch_field(&mut self.item_height, patch.item_height);
        super::patch::patch_field(&mut self.marker_width, patch.marker_width);
        super::patch::patch_field(&mut self.pad_y, patch.pad_y);
        super::patch::patch_field(&mut self.text_pad_x, patch.text_pad_x);
        super::patch::patch_field(&mut self.text, patch.text);
        super::patch::patch_field(&mut self.selected_fill, patch.selected_fill);
        super::patch::patch_field(&mut self.marker_color, patch.marker_color);
        super::patch::patch_field(&mut self.idle_text_color, patch.idle_text_color);
    }
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize, kithara_derive::SkinWalk)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct TabLargeSkin {
    pub height: f32,
    pub pad_x: f32,
    pub pad_y: f32,
    pub text: TextRoleSkin,
    pub idle_text_color: ColorRole,
    pub underline_color: ColorRole,
    pub underline_width: f32,
}

/// What a skin may restate of [`TabLargeSkin`].
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields)]
#[non_exhaustive]
pub struct TabLargePatch {
    pub height: Option<f32>,
    pub pad_x: Option<f32>,
    pub pad_y: Option<f32>,
    pub text: Option<TextRoleSkin>,
    pub idle_text_color: Option<ColorRole>,
    pub underline_color: Option<ColorRole>,
    pub underline_width: Option<f32>,
}

impl TabLargeSkin {
    /// Takes every field the patch restates, keeping the rest.
    pub(crate) fn patch(&mut self, patch: TabLargePatch) {
        super::patch::patch_field(&mut self.height, patch.height);
        super::patch::patch_field(&mut self.pad_x, patch.pad_x);
        super::patch::patch_field(&mut self.pad_y, patch.pad_y);
        super::patch::patch_field(&mut self.text, patch.text);
        super::patch::patch_field(&mut self.idle_text_color, patch.idle_text_color);
        super::patch::patch_field(&mut self.underline_color, patch.underline_color);
        super::patch::patch_field(&mut self.underline_width, patch.underline_width);
    }
}

macro_rules! define_text_skin {
    ($($(#[$attr:meta])* $field:ident => $role:ident),* $(,)?) => {
        #[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize, kithara_derive::SkinWalk)]
        #[serde(deny_unknown_fields)]
        #[non_exhaustive]
        pub struct TextSkin {
            pub deck_letter_active: ColorRole,
            pub size: SizeSpec,
            $(pub $field: TextRoleSkin,)*
        }

        /// What a skin may restate of [`TextSkin`].
        #[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Serialize)]
        #[serde(default, deny_unknown_fields)]
        #[non_exhaustive]
        pub struct TextPatch {
            pub deck_letter_active: Option<ColorRole>,
            pub size: Option<SizeSpec>,
            $(pub $field: Option<TextRoleSkin>,)*
        }

        impl TextSkin {
            /// The entry this skin gives one typographic role.
            pub(crate) fn role(&self, style: TextStyle) -> TextRoleSkin {
                match style {
                    $(TextStyle::$role => self.$field,)*
                }
            }

            /// Takes every field the patch restates, keeping the rest.
            pub(crate) fn patch(&mut self, patch: TextPatch) {
                if let Some(value) = patch.deck_letter_active {
                    self.deck_letter_active = value;
                }
                if let Some(value) = patch.size {
                    self.size = value;
                }
                $(if let Some(value) = patch.$field {
                    self.$field = value;
                })*
            }
        }
    };
}

text_roles!(define_text_skin);
