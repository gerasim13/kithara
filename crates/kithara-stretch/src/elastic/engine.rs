use super::{ElasticCapabilities, ElasticConfig, ElasticError, ElasticRequest};

/// Exact-span time-stretch engine.
///
/// The caller owns the transport: every call names the source span and the
/// output span in frames, and the frame counts are the only rate control. An
/// engine never chooses a rate, a direction or a phase on its own, so two
/// engines fed the same plan advance through the source identically.
pub trait ElasticEngine: Sized {
    /// Allocates and initializes an engine for a fixed preparation shape,
    /// outside the render core.
    ///
    /// # Errors
    /// Returns [`ElasticError`] when the shape is outside what the engine can
    /// represent, or when the engine cannot be constructed.
    fn prepare(config: ElasticConfig) -> Result<Self, ElasticError>;

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

    /// Clears stream history while retaining the prepared shape and latency.
    ///
    /// # Errors
    /// Returns [`ElasticError`] when an engine that clears state by rebuilding
    /// itself cannot be rebuilt.
    fn reset(&mut self) -> Result<(), ElasticError>;
}

/// Engines that can absorb source history without emitting it.
///
/// Priming resets the engine, consumes the declared source history plus one
/// warmup span, and discards exactly the declared output latency, so the next
/// [`ElasticEngine::process`] call starts at the source frame after the warmup
/// span with no leading gap. An engine whose pipeline can only emit what it
/// has already consumed cannot offer this and does not implement the trait;
/// see the crate `CONTEXT.md` for which backends do.
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
