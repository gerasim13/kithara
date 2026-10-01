#[derive(Clone, Debug, thiserror::Error)]
pub enum RenderError {
    #[error("invalid parameter value: {name}={value}")]
    InvalidParameter { name: String, value: f32 },
    #[error("eq band out of range: {band} (bands: {bands})")]
    EqBandOutOfRange { band: usize, bands: usize },
}

#[derive(Clone, Copy, Debug, thiserror::Error)]
pub enum ResponseError {
    #[error("session output response geometry overflowed")]
    GeometryOverflow,
    #[error(
        "session output requires {required_frames} response frames for block {max_block_frames} and quantum {render_quantum_frames}, exceeding budget {budget_frames}"
    )]
    BudgetExceeded {
        max_block_frames: u32,
        render_quantum_frames: usize,
        required_frames: usize,
        budget_frames: usize,
    },
}
