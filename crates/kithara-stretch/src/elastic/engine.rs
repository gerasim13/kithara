use super::{ElasticCapabilities, ElasticConfig, ElasticError, ElasticRequest};

/// Exact-span time-stretch engine.
///
/// The caller owns the transport: every call names the source span and the
/// output span in frames, and the frame counts are the only rate control. An
/// engine never chooses a rate, a direction or a phase on its own, so two
/// engines fed the same plan advance through the source identically.
pub trait ElasticEngine: Send + 'static {
    /// Allocates and initializes an engine for a fixed preparation shape,
    /// outside the render core.
    ///
    /// # Errors
    /// Returns [`ElasticError`] when the shape is outside what the engine can
    /// represent, or when the engine cannot be constructed.
    fn prepare(config: ElasticConfig) -> Result<Self, ElasticError>
    where
        Self: Sized;

    /// Immutable limits, latency and rate window of this engine.
    fn capabilities(&self) -> ElasticCapabilities;

    /// Renders exactly `request.output_frames()` interleaved output frames
    /// from exactly `request.source_frames()` interleaved source frames.
    ///
    /// # Errors
    /// Returns [`ElasticError`] when the request is outside the prepared
    /// limits or the declared rate envelope, when a buffer length does not
    /// match the request, or when the engine renders a different span.
    fn process(
        &mut self,
        request: ElasticRequest,
        source: &[f32],
        output: &mut [f32],
    ) -> Result<(), ElasticError>;

    /// Sets pitch independently from source-to-output frame advance.
    ///
    /// # Errors
    /// Returns [`ElasticError`] when `scale` is not finite, positive, or
    /// representable by the engine.
    fn set_pitch(&mut self, scale: f64) -> Result<(), ElasticError>;

    /// Writes the terminal buffered tail at most once per stream into caller-owned storage.
    ///
    /// The caller supplies storage for
    /// `capabilities().latency().output_frames() * channels` samples. Returns
    /// the number of interleaved frames written. Fresh or reset engines, and
    /// engines without a true tail drain, may ignore that storage and return
    /// zero. A successful repeat without an intervening
    /// [`process`](Self::process) is idempotent.
    ///
    /// # Errors
    /// Returns [`ElasticError`] when `output` does not match the engine's
    /// declared terminal span or when sizing that span overflows.
    fn flush(&mut self, output: &mut [f32]) -> Result<usize, ElasticError>;

    /// Clears stream history while retaining the prepared shape and latency.
    ///
    /// # Errors
    /// Returns [`ElasticError`] when an engine that clears state by rebuilding
    /// itself cannot be rebuilt.
    fn reset(&mut self) -> Result<(), ElasticError>;
}

/// Engine capability for absorbing source history without emitting it.
/// Priming leaves the next render aligned after the supplied warmup span.
pub trait ElasticPriming: ElasticEngine {
    /// Resets state and seeds exact history and warmup spans into
    /// `discarded_output`.
    ///
    /// # Errors
    /// Returns [`ElasticError`] when the warmup request does not match the
    /// declared latency, when a buffer length does not match the request, or
    /// when the rate is outside the declared envelope.
    fn prime(
        &mut self,
        request: ElasticRequest,
        source_history: &[f32],
        source: &[f32],
        discarded_output: &mut [f32],
    ) -> Result<(), ElasticError>;
}
