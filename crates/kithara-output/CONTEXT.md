# kithara-output context

## Ownership

This crate owns transport-neutral master PCM protocols. It does not own an
audio graph, encoder, storage backend, filesystem, or network origin.

## Offline rendering

An `OfflineRenderRequest` is one exact, finite, absolute half-open frame range
with an explicit `AudioSpec`. Reversed ranges and format mismatches fail before
output. A renderer may reject a start behind its consumed cursor; implicit
rewind and render-until-EOF are not part of the protocol.

The protocol does not own scheduling or block budgets. The renderer's owning
session configuration bounds work per iteration. `RenderSink` accepts
interleaved `f32` blocks and has no finalize operation: the composition owner
finishes a concrete sink after a successful report and drops it after
cancellation or failure. `OfflineRenderReport::frames` counts only frames
delivered successfully to the sink.

The core protocol is portable. Native graph and disk adapters are feature- and
target-gated in their owning crates.
