# kithara-worker

Domain-free building blocks for prioritized, cancellable worker dispatch.

The crate owns scheduler threads, admission limits, task priority, bounded
compute submission, and cancellation ancestry. Playback, analysis, storage,
and other domain behavior remain in their owning crates.
