use kithara_config::Config;
use kithara_dsp::interp::Interpolation;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Config)]
#[config(default, builder(state_mod(vis = "pub")))]
#[non_exhaustive]
pub struct GlideConfig {
    #[config(value, builder(default = Interpolation::Quadratic))]
    pub interpolation: Interpolation,
    #[config(value, builder(default = true))]
    pub anti_alias: bool,
}
