# kithara-signal

`kithara-signal` is Kithara's dependency-light decoded-audio data plane.

It owns the value types shared by decoders, playback, Warp, and streaming
analysis: format, owning chunk, timeline/provenance facts, frame/sample units,
and pure sample/time conversion. Pool mechanics remain in `kithara-bufpool`.
Encoded/container media facts remain in `kithara-stream`.

The crate has no decoder, network, asset, worker, scheduler, Warp, stretch,
player, analyzer, backend feature, or configuration responsibility.

See [CONTEXT.md](CONTEXT.md) for the ownership contract.
