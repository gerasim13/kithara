/// Why [`interpolate`](super::interpolate) refused a call.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum InterpError {
    #[error("interpolation position lies outside the window")]
    OutOfWindow,
}
