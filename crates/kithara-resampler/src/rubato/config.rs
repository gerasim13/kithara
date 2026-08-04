use bon::Builder;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[non_exhaustive]
pub enum RubatoAlgorithm {
    #[default]
    Async,
    Fft,
}

#[derive(Clone, Copy, Debug, Default, Builder, Eq, PartialEq)]
#[builder(state_mod(vis = "pub"))]
#[non_exhaustive]
pub struct RubatoConfig {
    #[builder(default)]
    pub algorithm: RubatoAlgorithm,
}
