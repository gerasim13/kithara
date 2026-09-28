/// Why a filter could not be designed, built or run.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum FilterError {
    #[error("filter parameters are out of range or unstable")]
    Parameters,
    #[error("the platform could not create the filter")]
    Setup,
    #[error("buffer shape does not match the filter")]
    Shape,
}
