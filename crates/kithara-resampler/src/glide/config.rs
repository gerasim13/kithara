use bon::Builder;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[non_exhaustive]
pub enum GlideInterpolation {
    Linear,
    #[default]
    Quadratic,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Builder)]
#[builder(const, state_mod(vis = "pub"))]
#[non_exhaustive]
pub struct GlideConfig {
    #[builder(default = GlideInterpolation::Quadratic)]
    pub interpolation: GlideInterpolation,
    #[builder(default = true)]
    pub anti_alias: bool,
}

impl Default for GlideConfig {
    fn default() -> Self {
        Self::builder().build()
    }
}
