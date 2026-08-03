use iced::Color;

/// Resolved color palette consumed by renderers.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct RenderPalette {
    pub accent: Color,
    pub accent_soft: Color,
    pub accent_strong: Color,
    pub bg: Color,
    pub bg_deep: Color,
    pub bg_footer: Color,
    pub bg_inset: Color,
    pub bg_panel: Color,
    pub bg_panel_2: Color,
    pub bg_select: Color,
    pub danger: Color,
    pub line: Color,
    pub line_dim: Color,
    pub line_hi: Color,
    pub line_inner: Color,
    pub line_pop: Color,
    pub line_soft: Color,
    pub muted: Color,
    pub shadow: Color,
    pub success: Color,
    pub text: Color,
    pub text_dim: Color,
    pub warning: Color,
    pub wave_high: Color,
    pub wave_low: Color,
    pub wave_mid: Color,
}
