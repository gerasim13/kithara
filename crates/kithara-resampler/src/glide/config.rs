use bon::Builder;
use kithara_dsp::interp::Interpolation;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Builder)]
#[builder(const, state_mod(vis = "pub"))]
#[non_exhaustive]
#[derive(kithara_derive::BuiltDefault)]
pub struct GlideConfig {
    #[builder(default = Interpolation::Quadratic)]
    pub interpolation: Interpolation,
    #[builder(default = true)]
    pub anti_alias: bool,
}
