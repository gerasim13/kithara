# kithara-test-fixtures — Context

Contracts and invariants for kithara-test-fixtures; the README is the overview.

## Store Layout

```
<root>/<fingerprint>/<id>.<ext>
<root>/<fingerprint>/<id>.lock
```

- **root** — `KITHARA_FIXTURE_CACHE` when set, otherwise
  `<temp_dir>/kithara-fixture-cache`. CI points it at a persisted directory, so
  a fresh job finds every entry already there.
- **fingerprint** — 16 hex characters identifying the build that produced the
  entries below it. See *Invalidation*.
- **id** — `sha2-256(func ‖ 0x00 ‖ case)`, first 16 bytes, hex. The separator is
  what keeps `("sine", "wav_6s")` and `("sine_wav", "6s")` apart.

An entry is one file. A **zero-length file is a miss**, not an empty asset: a
producer that dies mid-write must not leave something a reader will serve.
Writes go to `<id>.<ext>.tmp.<pid>`, are `sync_all`ed, then renamed — a reader
sees the entry whole or not at all.

## Identity Without `module_path!()`

The id hashes the function name and the case name, and nothing else. It
deliberately omits the module path, which inside a build script expands to
`build_script_build::defs::…` — an artefact of how the generators are compiled,
not a property of the asset. Uniqueness holds by construction: every accessor
lands in one flat generated module, so two cases sharing both halves could not
coexist there. The build script asserts it anyway.

## Invalidation

Content-addressing over the accessor name alone cannot notice that a generator
changed its output. The fingerprint is what stands between a changed generator
and the bytes the previous one produced: it hashes the generator sources and the
inputs they encode with. A changed generator lands in a new namespace and
regenerates; the old namespace stays until the store is pruned, which is what
lets a branch switch back and forth without paying twice.

The fingerprint is a build-script concern; the store only receives it as an
opaque directory name.

## Two Producers, One Entry

Every consumer of the store — parallel build scripts, several checkouts sharing
one root — races on the same entries. The protocol is double-checked:

1. Read the entry. A hit ends it.
2. Take `lock_entry`, which blocks until the entry is this process's to produce.
3. Read again. Another producer may have finished while this one waited.
4. Produce, write atomically, release.

`EntryLock` releases with its file handle, so a producer that panics or is
killed does not wedge the store.

## `embed`

`#[kithara::asset(..., embed)]` changes how an asset is *served*, never how it is
produced. The build script materializes it into the store exactly as it does any
other asset, then emits an accessor that reads that file back with
`include_bytes!` — one generation pass, bytes baked into the binary.

Consequences, in the order they matter:

- `Asset::path()` returns `None`. An embedded asset has no file at run time, so
  a test that needs a path on disk must not embed.
- The bytes cost binary size in every test binary that links the accessor. Embed
  where disk access is the thing under test or unavailable — wasm has no
  filesystem, and only embedded accessors compile there.
- rustc records `include_bytes!` paths in dep-info, so cargo rebuilds the
  accessor when the store entry it was built from changes.

## Generators Stay Out Of The Library

`src/defs/` is reached only through `#[path]` from `build.rs`. Two consequences,
both load-bearing:

- Encoding is a **build-dependency**. `kithara-encode` enters the target build
  for its `PcmSource` trait alone, with FFmpeg's encoder features off the
  library's own default path; nothing in a target build calls an encoder.
- Generation happens exactly once per fingerprint, in the build script. Nothing
  in the library can synthesize an asset, which is the whole point: a test's
  deadline never contains an encode.

## One Way To Make A Signal

`src/signal/` is the exception, and the only one: it is the library's, and the
build script reaches it through `#[path]` exactly as it reaches `src/store.rs`.
The same source file, two roots, one visibility.

It exists because a waveform is needed on both sides of the build line. `defs/`
renders assets from it before the tests start; the integration suite renders
the bodies its HTTP fixtures serve from it while they run. Two implementations
of a sine would drift, and a test that asserts on decoded samples cannot tell
which one it is asserting against.

- `Wave` — the waveform vocabulary. One enum, one `sample(frame, sample_rate)`.
  `TONE` names the 440 Hz full-scale sine that a fixture carries unless it says
  otherwise.
- `Pcm` — interleaved 16-bit PCM in memory; `PcmSource` off wasm.
- `wav` / `wav_of_size` / `header` — the RIFF writer, including the streaming
  header whose size fields say `0xFFFFFFFF` because the total is not known when
  it is written.

`Pcm::new` and `wav` take a `Wave`; `Pcm::from_fn` and `wav_from_fn` take a
per-frame closure, for the bodies no single waveform describes.

The same module owns the reading side, because a reader that disagrees with the
writer is worse than no reader at all:

- `phase` — the saw decoder. `units` recovers a frame's phase from a decoded
  sample, `delta` is the shortest signed step between two phases, `distance`
  the same ignoring direction. `SAW_PERIOD` is the one period, and `phase`
  fails to compile if its signed twin ever stops matching it.
- `detect_direction` / `SignalDirection` — which way a saw runs, by voting over
  the first frames of a buffer.
- `classify_windows` / `ascending_phase_replays` / `FrameClass` / `Replay` —
  per-window provenance: which stretch of a decoded stream ascends, descends,
  or is silent, and where a stream replays phase it already served.
- `goertzel_magnitude` — energy at one frequency, for telling two generated
  tones apart without a full transform.

These were four copies of the saw period and five of the phase decoder spread
across `tests/src/` and individual suites. A test that measures a fixture and a
build script that writes one now agree by construction.

`SignalAsset` sits beside `signal` rather than inside `assets`, because it has
to compile for wasm and `assets` does not: the store is a host filesystem, and
the browser lane names an asset and fetches its bytes over HTTP. Its census
tests hold the two sides together — every declared asset is registered by a
generator, every registered one is declarable, and the extensions agree.

This is the workspace's only route to a generated signal. `tests/src/wav.rs`
and `tests/src/signal_pcm.rs` were the other two — a `SignalFn` trait with one
struct per waveform, a lazy renderer with an `Infinite` length nothing asked
for, and a second RIFF writer. They are gone, and nothing may grow back beside
`signal`: a suite that asserts on decoded samples has to know which sine it is
asserting against.

## What A Build Costs

Measured with `cargo build -p kithara-test-fixtures` on an already-compiled tree,
three assets in the matrix (two six-second WAVs, one two-second MP3):

| Case | Wall clock |
| --- | --- |
| Warm — store populated, script not rerun | 1.8–2.8 s |
| Cold — store wiped, every asset regenerated | 6.0–7.1 s |

The difference, ~4 s, is the whole of generation: the build script binary
starts, every generator runs, the encoders produce bytes, the store takes
them. Warm is cargo walking the dependency graph and finding nothing to do —
a floor this crate does not control.

Two costs sit outside the table. Compiling the encode stack the first time
dominates both columns and amortizes like any build dependency. And the run
that regenerates pays once for the whole workspace, where before every test
binary that wanted an asset paid inside its own deadline.

These numbers are the baseline the next migration stages are measured
against: the cold column grows with the matrix, the warm column must not.

## Where The Analyzers Cannot Follow

Two workspace checks are told about this crate rather than worked around at the
call site, because both misread it for the same reason: the code that consumes
these exports is generated into `OUT_DIR` or lives in the build script, and a
source scanner sees neither.

- `dead_exports` (`.config/arch/thresholds.toml`) counts kithara-test-fixtures among
  the test crates. Its exports are reached only from generated accessors and its
  own build script, and a call it makes into a workspace API is a fixture, not a
  shipped caller.
- `perf.prefer-primitive-pool` skips this crate, alongside the other test
  scaffolding. A generator returns one complete asset that crosses an ownership
  boundary into the store; there is no pool in a build script to lease from, and
  the rule itself exempts that shape.

## Transitional Coupling

`tests/src/fixture_cache.rs` is the disk cache this store replaces. It is frozen
for the duration of the migration and dies once its last consumer moves over.
Nothing new should be built on it.
