use kithara_bufpool::HasPool;
use kithara_derive::Ranged;

use super::{ElasticCapabilities, ElasticConfig, ElasticDrain, ElasticError, ElasticRequest};

/// Valid native pitch factor shared by every elastic backend.
#[derive(Clone, Copy, Debug, PartialEq, PartialOrd, Ranged)]
#[ranged(min = 0.25, max = 4.0, default = 1.0)]
pub(crate) struct PitchScale(f64);

/// Exact-span time-stretch engine.
///
/// The caller owns the transport: every call names the source span and the
/// output span in frames, and the frame counts are the only rate control. An
/// engine never chooses a rate, a direction or a phase on its own, so two
/// engines fed the same plan advance through the source identically.
pub trait ElasticEngine: Send + 'static {
    /// Immutable limits, latency and rate window of this engine.
    fn capabilities(&self) -> ElasticCapabilities;

    /// Drains terminal audio into caller-owned non-empty whole-frame storage.
    /// Repeat until [`ElasticDrain`] is complete. Incomplete steps write a non-empty
    /// contiguous tail within capacity; the final active step completes with its
    /// last non-empty portion. Fresh, reset or completed engines return an empty
    /// completed step until [`prime`](Self::prime) or [`process`](Self::process).
    ///
    /// # Errors
    /// An active drain returns [`ElasticError`] for empty or partial-frame output
    /// or span overflow. An inactive drain completes without accessing `output`.
    fn flush(&mut self, output: &mut [f32]) -> Result<ElasticDrain, ElasticError>;

    /// Allocates and initializes an engine for a fixed preparation shape,
    /// outside the render core.
    ///
    /// # Errors
    /// Returns [`ElasticError`] when the shape is outside what the engine can
    /// represent, or when the engine cannot be constructed.
    fn prepare<S>(config: ElasticConfig<S>) -> Result<Self, ElasticError>
    where
        Self: Sized,
        S: HasPool<f32>;

    /// Clears stream state, absorbs history/lookahead, and renders one latency-sized
    /// warmup span into caller-owned discard storage. `source_lookahead` holds exactly
    /// the declared source latency from the audible cue; `source` follows it at the
    /// request's rate. The next [`process`](Self::process) input follows `source`,
    /// while its first output starts at the lookahead cue after latency is absorbed.
    ///
    /// # Errors
    /// Returns [`ElasticError`] for a request not matching declared latency,
    /// buffer lengths not matching the request, or a rate outside the envelope.
    fn prime(
        &mut self,
        request: ElasticRequest,
        source_history: &[f32],
        source_lookahead: &[f32],
        source: &[f32],
        discarded_output: &mut [f32],
    ) -> Result<(), ElasticError>;

    /// Renders exactly `request.output_frames()` interleaved frames and admits
    /// exactly `request.source_frames()`. `output_source_frames()` separately names
    /// the audible source advance: projected callers can admit future source for a
    /// delayed pipeline. Cue-anchored engines schedule that span; input-driven ones
    /// retain native delay, without changing lengths or adding a rate control.
    /// A ratio change affects emitted audio within one declared output latency,
    /// including across adjacent calls; no extra software buffering delay is allowed.
    ///
    /// # Errors
    /// Returns [`ElasticError`] for prepared-limit/rate-envelope violations,
    /// length mismatches or a differently rendered span.
    fn process(
        &mut self,
        request: ElasticRequest,
        source: &[f32],
        output: &mut [f32],
    ) -> Result<(), ElasticError>;

    /// Clears stream history while retaining the prepared shape and latency.
    ///
    /// # Errors
    /// Returns [`ElasticError`] when the resident backend state cannot be cleared.
    fn reset(&mut self) -> Result<(), ElasticError>;

    /// Sets pitch independently from source-to-output frame advance. Across
    /// immediately adjacent [`process`](Self::process) calls, a changed pitch
    /// must affect emitted audio within the declared output latency; engines
    /// must not add a second software-buffering delay.
    ///
    /// # Errors
    /// Returns [`ElasticError`] when `scale` is outside the common native
    /// range `0.25..=4.0` or is not finite.
    fn set_pitch(&mut self, scale: f64) -> Result<(), ElasticError>;
}
