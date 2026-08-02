use bon::Builder;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[non_exhaustive]
pub enum GlideInterpolation {
    Linear,
    #[default]
    Quadratic,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Builder)]
#[builder(state_mod(vis = "pub"))]
#[non_exhaustive]
pub struct GlideConfig {
    #[builder(default)]
    pub interpolation: GlideInterpolation,
    #[builder(default = true)]
    pub anti_alias: bool,
}

impl Default for GlideConfig {
    fn default() -> Self {
        Self {
            anti_alias: true,
            interpolation: GlideInterpolation::Quadratic,
        }
    }
}
