use kithara_config::Config;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[non_exhaustive]
pub enum RubatoAlgorithm {
    #[default]
    Async,
    Fft,
}

#[derive(Clone, Copy, Debug, Default, Config, Eq, PartialEq)]
#[config(builder(state_mod(vis = "pub")))]
#[non_exhaustive]
pub struct RubatoConfig {
    #[config(value, builder(default))]
    pub algorithm: RubatoAlgorithm,
}
