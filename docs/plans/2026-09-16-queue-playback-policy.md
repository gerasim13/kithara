# Typed Queue Playback Policy

<task_packet>
Goal: Replace legacy queue booleans with typed traversal, terminal-action, selection-transport, and crossfade-profile policy across Rust and every supported binding.
Affected paths: `crates/kithara-queue`, `crates/kithara-play`, `crates/kithara-ffi`, Apple and Android wrappers, web bindings, demos, and focused queue/play integration tests.
Read first: `AGENTS.md`, `docs/workflows/rust-ai.md`, the queue and play README contracts, and the product references named in the task request.
Same-as example: Existing `RepeatMode` ownership and cross-surface event propagation.
Constraints: Queue navigation solely owns traversal state. Queue policy captures automatic decisions and selection transport at admission. Play owns RT-safe execution of an immutable validated fade profile. Generated bindings come only from repository tooling. No compatibility adapters, parallel mutable authorities, fallback inference, or dependency on PR #321.
Non-goals: Cue, sync, Warp, beat-grid, musical-grid, decoder, DSP beyond the requested fade gain law, and public RNG/seed configuration.
Expected output: One independent draft PR against `production/main`, split into logical commits, with exact-head GitHub and GitLab CI green.
Validation scope: Format and fast lint harnesses; focused queue, play, FFI, web, Apple, and Android tests; native compile; WASM check; current required CI graph on both forges.
Split proposal: None. The public types and generated surfaces share one contract and one integrator.
</task_packet>

## Ownership and design

- `QueueConfig` owns serialized defaults: sequential order, advance at item end, and the validated crossfade profile.
- Queue runtime owns mutable policy and emits typed changes; its navigation object exclusively owns shuffle bag/history by `TrackId`.
- A pending selection stores its reason, transport decision, and complete transition value so delayed completion cannot reinterpret live policy.
- `kithara-play` owns `SelectionPlayback`, crossfade validation, and the allocation-free gain calculation used by the render path.
- FFI and platform wrappers translate the same typed values and reject invalid external values at their boundary.

## Validation

Use focused pairwise contract tests from the task request, then the repository acceptance commands. Record exact base and exact PR head separately; a failure inherited from `production/main` is not PR evidence.
