use serde::{Deserialize, Serialize};

use super::{
    document::{FontFamily, FontWeight},
    palette::ColorRole,
    section::skin_section,
};
use crate::module::{Tone, WindowControlsStyle};

/// One of the two looks a control switches between: what it paints under
/// itself, and what it draws on top. A face naming no fill paints none, and
/// sits on the surface it is mounted in.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct FaceSkin {
    pub content: ColorRole,
    pub fill: Option<ColorRole>,
}

/// What a control paints under itself in each pointer state. A state naming
/// no colour paints nothing, which is how a control sits on the surface it is
/// mounted in rather than over it.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct StateColors {
    pub hovered: Option<ColorRole>,
    pub idle: Option<ColorRole>,
    pub pressed: Option<ColorRole>,
}

/// The colour each of a control's four tones names. A control the document
/// hands a tone reads its colour here rather than from a palette role named
/// in Rust.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ToneColors {
    pub accent: ColorRole,
    pub danger: ColorRole,
    pub neutral: ColorRole,
    pub success: ColorRole,
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

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct TextRoleSkin {
    pub color: ColorRole,
    pub font: FontFamily,
    pub weight: FontWeight,
    pub size: f32,
    pub spacing: f32,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct FrameSkin {
    pub border: ColorRole,
    pub border_width: f32,
    pub radius: f32,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ShadowSkin {
    pub color: ColorRole,
    pub alpha: f32,
    pub blur: f32,
    pub offset_x: f32,
    pub offset_y: f32,
}

/// Scale beside a fader: hairlines with a longer, brighter one at centre.
/// `thickness` runs along the scale, `length` across it, whatever the axis.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct TickSkin {
    pub center_color: ColorRole,
    pub color: ColorRole,
    pub center_length: f32,
    pub gap: f32,
    pub inset: f32,
    pub length: f32,
    pub thickness: f32,
    pub count: usize,
}

skin_section! {
    pub struct LayoutSkin => LayoutPatch {
        /// What a host clears the target to wherever no document reaches.
        pub page_background: ColorRole,
        pub grid_gap: f32,
        pub grid_pad: f32,
    }
}

skin_section! {
    /// The indicator a viewport draws over its own right edge. `min_length` keeps
    /// a window over very long content from showing a thumb too short to see.
    pub struct ScrollSkin => ScrollPatch {
        pub thumb: ColorRole,
        pub track: ColorRole,
        pub inset: f32,
        pub min_length: f32,
        pub width: f32,
    }
}

skin_section! {
    pub struct ChromeSkin => ChromePatch {
        pub chevron_color: ColorRole,
        pub chip_background: ColorRole,
        pub corner_color: ColorRole,
        pub drop_zone_color: ColorRole,
        pub footer_background: ColorRole,
        pub header_background: ColorRole,
        pub inner_line: ColorRole,
        pub panel_background: ColorRole,
        pub title_background: ColorRole,
        pub chip_text: TextRoleSkin,
        pub footer_text: TextRoleSkin,
        pub title_text: TextRoleSkin,
        pub chevron_frame: FrameSkin,
        pub chip_frame: FrameSkin,
        pub footer_frame: FrameSkin,
        pub frame: FrameSkin,
        pub header_frame: FrameSkin,
        pub secondary_frame: FrameSkin,
        pub title_frame: FrameSkin,
        pub chevron_icon_size: f32,
        pub chevron_size: f32,
        pub chevron_stroke_width: f32,
        pub chip_pad: f32,
        pub corner_offset: f32,
        pub corner_size: f32,
        pub corner_width: f32,
        pub footer_height: f32,
        pub footer_pad: f32,
        pub header_height: f32,
        pub inner_line_width: f32,
    }
}

skin_section! {
    pub struct WindowSkin => WindowPatch {
        pub icon_color: ColorRole,
        pub icon_hover_color: ColorRole,
        pub titlebar_text: TextRoleSkin,
        pub standard: WindowControlSkin,
        pub compact: WindowControlSkin,
        pub close_wide: WindowControlSkin,
        pub close_micro: WindowControlSkin,
        pub close_framed: WindowControlSkin,
        pub icon_stroke_width: f32,
        /// Thickness of the drag zones framing a window that draws its own chrome.
        pub resize_edge: f32,
        pub titlebar_height: f32,
        pub titlebar_padding_x: f32,
    }
}

/// How one window-controls style draws: a row of buttons, or a single close
/// cell.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub enum WindowControlSkin {
    Buttons {
        minus_icon_size: f32,
        maximize_icon_size: f32,
        close_icon_size: f32,
        gap: f32,
        padding: f32,
    },
    Close {
        cell_size: f32,
        icon_size: f32,
        frame: Option<FrameSkin>,
        divider: Option<(f32, ColorRole)>,
    },
}

impl WindowSkin {
    pub(crate) const fn controls(self, style: WindowControlsStyle) -> WindowControlSkin {
        match style {
            WindowControlsStyle::Standard => self.standard,
            WindowControlsStyle::Compact => self.compact,
            WindowControlsStyle::CloseWide => self.close_wide,
            WindowControlsStyle::CloseMicro => self.close_micro,
            WindowControlsStyle::CloseFramed => self.close_framed,
        }
    }
}
