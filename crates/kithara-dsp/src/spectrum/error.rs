/// Why a spectrum could not be built or taken.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum SpectrumError {
    #[error("FFT length is not f·2ⁿ with f in 1, 3, 5, 15 and n ≥ 4")]
    Length,
    #[error("the backend could not build the FFT")]
    Setup,
    #[error("buffer shape does not match the FFT")]
    Shape,
}
