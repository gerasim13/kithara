use serde::{Deserialize, Serialize};

use super::{
    palette::ColorRole,
    primitives::{FrameSkin, ShadowSkin, StateColors, TextRoleSkin},
    section::skin_section,
};
use crate::size::SizeSpec;

skin_section! {
    pub struct WaveSkin => WavePatch {
        pub background: ColorRole,
        /// The three levels a band nests by, the ground under them, the grid
        /// over them, and the part already played.
        pub band_low_color: ColorRole,
        pub band_mid_color: ColorRole,
        pub band_high_color: ColorRole,
        pub trough_color: ColorRole,
        pub grid_color: ColorRole,
        pub label_color: ColorRole,
        pub played_color: ColorRole,
        /// Extent cached ahead of the playhead; the played part takes the accent.
        pub cache_strip_color: ColorRole,
        /// Rails and boundaries of a range the analysis has not covered.
        pub coverage_edge_color: ColorRole,
        /// The covered baseline and the stubs standing in for missing columns.
        pub coverage_mark_color: ColorRole,
        pub cue_badge_background: ColorRole,
        pub cue_badge_text: TextRoleSkin,
        pub frame: FrameSkin,
        /// `WaveStyle::Default`.
        pub default_size: SizeSpec,
        /// `WaveStyle::Micro`.
        pub micro_size: SizeSpec,
        /// `WaveStyle::Hero`.
        pub size: SizeSpec,
        pub overlay: WaveOverlaySkin,
        pub bar_gap: f32,
        /// Width of every band bar; bands nest by level, not by width.
        pub bar_width: f32,
        pub cache_strip_alpha: f32,
        pub cache_strip_height: f32,
        pub content_inset: f32,
        pub coverage_baseline_alpha: f32,
        /// Height of the covered baseline and width of a region boundary.
        pub coverage_hairline: f32,
        pub coverage_rail_height: f32,
        pub coverage_stub_alpha: f32,
        pub coverage_stub_height: f32,
        pub cue_badge_size: f32,
        pub cue_line_width: f32,
        pub downbeat_alpha: f32,
        pub grid_alpha: f32,
        pub grid_width: f32,
        pub loop_bound_width: f32,
        pub loop_fill_alpha: f32,
        /// Overview strips dim harder than the hero wave.
        pub overview_played_alpha: f32,
        pub played_alpha: f32,
        pub playhead_marker_height: f32,
        pub playhead_marker_width: f32,
        pub playhead_width: f32,
    }
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct WaveOverlaySkin {
    pub art_background: ColorRole,
    pub background: ColorRole,
    pub badge_background: ColorRole,
    pub bpm_color: ColorRole,
    pub key_color: ColorRole,
    pub readout_background: ColorRole,
    pub remain_color: ColorRole,
    pub art_frame: FrameSkin,
    pub badge_frame: FrameSkin,
    pub readout_frame: FrameSkin,
    pub art_label: TextRoleSkin,
    pub artist: TextRoleSkin,
    pub badge_text: TextRoleSkin,
    pub readout_label: TextRoleSkin,
    pub readout_value: TextRoleSkin,
    pub title: TextRoleSkin,
    pub art_size: f32,
    pub background_alpha: f32,
    pub badge_size: f32,
    pub bpm_width: f32,
    pub gap: f32,
    pub height: f32,
    pub key_width: f32,
    pub padding_x: f32,
    pub padding_y: f32,
    pub readout_gap: f32,
    pub readout_height: f32,
    pub readout_padding_x: f32,
    pub readout_padding_y: f32,
    pub remain_width: f32,
    pub summary_gap: f32,
}

skin_section! {
    pub struct DeckSkin => DeckPatch {
        pub clock_background: ColorRole,
        pub panel_color: ColorRole,
        pub artist: TextRoleSkin,
        pub bpm_text: TextRoleSkin,
        pub micro_source: TextRoleSkin,
        pub micro_title: TextRoleSkin,
        pub readout_label: TextRoleSkin,
        pub time_text: TextRoleSkin,
        pub title: TextRoleSkin,
        pub bpm_size: SizeSpec,
        pub summary_size: SizeSpec,
        pub time_size: SizeSpec,
        pub micro_summary_gap: f32,
        pub readout_gap: f32,
        pub summary_padding_x: f32,
        pub summary_padding_y: f32,
        pub time_padding_x: f32,
        pub time_padding_y: f32,
    }
}

skin_section! {
    pub struct GlobalBarSkin => GlobalBarPatch {
        pub brand_text: TextRoleSkin,
        pub chip_text: TextRoleSkin,
        /// What a preset chip fills with when it is not the one in use, and
        /// when it is.
        pub chip_fill: StateColors,
        pub chip_active_fill: StateColors,
        pub chip_active_text_color: ColorRole,
        pub settings_fill: StateColors,
        pub settings_icon_color: ColorRole,
        pub panel_fill: ColorRole,
        pub selector_fill: ColorRole,
        pub chip_frame: FrameSkin,
        pub selector_frame: FrameSkin,
        pub settings_frame: FrameSkin,
        pub brand_size: SizeSpec,
        pub preset_size: SizeSpec,
        pub settings_size: SizeSpec,
        pub spacer_size: SizeSpec,
        pub brand_gap: f32,
        pub brand_padding_x: f32,
        pub brand_padding_y: f32,
        pub brand_width: f32,
        pub chip_gap: f32,
        pub chip_padding_x: f32,
        pub chip_padding_y: f32,
        pub gear_size: f32,
        pub height: f32,
        pub selector_padding_x: f32,
        pub selector_padding_y: f32,
        pub selector_width: f32,
        pub settings_padding: f32,
    }
}

skin_section! {
    /// Horizontal fill bar reporting one scalar, as the design's CPU cell draws it:
    /// an inset track with a hairline frame, filled from the left.
    pub struct MeterSkin => MeterPatch {
        pub background: ColorRole,
        pub fill: ColorRole,
        pub frame: FrameSkin,
        pub size: SizeSpec,
    }
}

skin_section! {
    /// Hairline between adjacent cells or control sections.
    pub struct DividerSkin => DividerPatch {
        pub color: ColorRole,
        pub width: f32,
    }
}

skin_section! {
    /// The label the pointer carries while it drags an item.
    pub struct DragSkin => DragPatch {
        pub background: ColorRole,
        pub frame: FrameSkin,
        pub text: TextRoleSkin,
        pub height: f32,
        pub pad_x: f32,
        pub width: f32,
    }
}

skin_section! {
    /// Pop-over chrome; the frame and the cap draw outward of the content column.
    pub struct PopSkin => PopPatch {
        pub background: ColorRole,
        pub cap_color: ColorRole,
        pub frame: FrameSkin,
        pub shadow: ShadowSkin,
        pub cap_height: f32,
    }
}

skin_section! {
    pub struct TelemetrySkin => TelemetryPatch {
        pub inset_color: ColorRole,
        pub text: TextRoleSkin,
        pub frame: FrameSkin,
        pub size: SizeSpec,
        pub padding_x: f32,
        pub padding_y: f32,
        pub percent_scale: f64,
        pub percent_precision: usize,
        pub percent_width: usize,
        pub scalar_precision: usize,
    }
}

skin_section! {
    pub struct TreeSkin => TreePatch {
        pub context_background: ColorRole,
        pub context_icon_color: ColorRole,
        /// What a row lays down behind itself, and the bar it carries on its
        /// edge while it is the row in use. A row nobody is on lays nothing.
        pub row_selected_fill: ColorRole,
        pub row_hovered_fill: ColorRole,
        pub row_marker_color: ColorRole,
        pub row_text_color: ColorRole,
        pub row_muted_text_color: ColorRole,
        pub row_idle_text_color: ColorRole,
        pub chevron_color: ColorRole,
        pub search_caret_color: ColorRole,
        pub search_icon_color: ColorRole,
        pub search_placeholder_color: ColorRole,
        pub search_selection_fill: ColorRole,
        pub context_divider: ColorRole,
        pub panel_background: ColorRole,
        pub scope_background: ColorRole,
        pub scope_chevron_color: ColorRole,
        pub scope_menu_background: ColorRole,
        pub scope_menu_text: ColorRole,
        pub scope_selected_background: ColorRole,
        pub scope_selected_text: ColorRole,
        pub scope_text_color: ColorRole,
        pub scrollbar_background: ColorRole,
        pub scroller_color: ColorRole,
        pub search_background: ColorRole,
        pub search_divider: ColorRole,
        pub context_text: TextRoleSkin,
        pub count_text: TextRoleSkin,
        pub label_text: TextRoleSkin,
        pub scope_text: TextRoleSkin,
        pub search_text: TextRoleSkin,
        pub scope_frame: FrameSkin,
        pub scope_menu_frame: FrameSkin,
        pub size: SizeSpec,
        pub chevron_size: f32,
        pub chevron_width: f32,
        pub content_gap: f32,
        pub context_divider_width: f32,
        pub context_gap: f32,
        pub context_height: f32,
        pub context_icon_size: f32,
        pub context_padding_x: f32,
        pub icon_size: f32,
        pub indent_base: f32,
        pub indent_step: f32,
        pub marker_width: f32,
        pub panel_padding_bottom: f32,
        pub panel_padding_top: f32,
        pub row_height: f32,
        pub row_padding_right: f32,
        pub scope_chevron_size: f32,
        pub scope_gap: f32,
        pub scope_item_height: f32,
        pub scope_padding_x: f32,
        pub scrollbar_margin: f32,
        pub scrollbar_width: f32,
        pub search_height: f32,
        pub search_icon_size: f32,
        pub search_icon_width: f32,
        pub search_padding_x: f32,
    }
}

skin_section! {
    pub struct TableSkin => TablePatch {
        pub metric_badge_background: ColorRole,
        /// The ground the grid, the header strip and the footer strip lay down
        /// before any row is drawn.
        pub grid_color: ColorRole,
        pub header_fill: ColorRole,
        pub footer_fill: ColorRole,
        pub badge_fill: ColorRole,
        pub meter_bar_fill: ColorRole,
        pub row_fill: StateColors,
        pub row_selected_fill: ColorRole,
        pub divider_color: ColorRole,
        pub meter_bar_background: ColorRole,
        pub scrollbar_background: ColorRole,
        pub scroller_color: ColorRole,
        pub secondary_text: TextRoleSkin,
        pub metric_text: TextRoleSkin,
        pub badge_text: TextRoleSkin,
        pub meter_text: TextRoleSkin,
        pub footer_text: TextRoleSkin,
        pub header_text: TextRoleSkin,
        pub index_text: TextRoleSkin,
        pub mono_text: TextRoleSkin,
        pub time_text: TextRoleSkin,
        pub primary_text: TextRoleSkin,
        pub transition_text: TextRoleSkin,
        pub metric_badge_frame: FrameSkin,
        pub badge_frame: FrameSkin,
        pub row_frame: FrameSkin,
        pub size: SizeSpec,
        pub metric_badge_height: f32,
        pub metric_badge_padding_x: f32,
        pub cell_padding_x: f32,
        pub badge_height: f32,
        pub badge_width: f32,
        pub divider_hit_width: f32,
        pub divider_width: f32,
        pub meter_bar_gap: f32,
        pub meter_bar_height: f32,
        pub meter_bar_width: f32,
        pub footer_height: f32,
        pub footer_padding_x: f32,
        pub grid_gap: f32,
        pub header_height: f32,
        pub min_column_width: f32,
        pub row_height: f32,
        pub scrollbar_margin: f32,
        pub scrollbar_width: f32,
    }
}

skin_section! {
    pub struct LayoutPreviewSkin => LayoutPreviewPatch {
        pub height: f32,
        pub line_width: f32,
        pub module_inset: f32,
    }
}

#[cfg(test)]
mod tests {
    use kithara_test_utils::kithara;

    use super::*;
    use crate::builtin;

    #[kithara::test]
    fn pop_holds_exactly_the_declared_chrome() {
        assert_eq!(
            builtin::skin_doc().pop,
            PopSkin {
                background: ColorRole::BgFooter,
                frame: FrameSkin {
                    radius: 0.0,
                    border_width: 1.0,
                    border: ColorRole::LineHi,
                },
                cap_height: 2.0,
                cap_color: ColorRole::Accent,
                shadow: ShadowSkin {
                    color: ColorRole::Shadow,
                    alpha: 0.6,
                    offset_x: 0.0,
                    offset_y: 16.0,
                    blur: 40.0,
                },
            }
        );
    }
}
