# Progress

What is in flight right now. The
[GitHub Projects board](https://github.com/users/gerasim13/projects/3) owns
capability status and the roadmap, and git owns the facts. This file owns
intent: what is being worked on, what comes next, what is stuck. Update it in
the change that lands the work, and keep it short.

## In Flight

- Mac CI host cleanup. The host spent a day refusing jobs for space while its
  hourly pass was gone: the agent hung inside `opendir` on a volume that had
  stopped answering, and launchd starts no second instance while the first is
  alive. A watchdog thread now ends a pass at `cleanup_deadline_seconds`. The
  pass that did run freed nothing, because `build_cache_size` is a ceiling over
  one cache and says nothing about whether the volume has room; under
  `Aggressive` or `Reject` it now reclaims what the volume is short of the soft
  floor as well. Cleanup also judges by the volume it measured rather than
  reading free space a second time.

  Verifying any of this was blocked by a second defect. `deps:deny` spent
  twenty-five minutes listing the `boringssl` submodule's refs before its
  job timed out: `GIT_CONFIG_COUNT` pins the HTTP version for the git
  binary, and libgit2, which Cargo fetches with, does not read it. Cargo
  now fetches through the binary, so both halves see one version. The lane
  still gates a quarantine pipeline directly instead of reporting to the
  verdict, which is what let one network stall hold every pull request;
  that is open.

  Then six pull requests went red at once for a queue someone emptied. The
  bridge read a cancelled pipeline as a verdict and recorded it terminal, so
  nothing addressed them again; a cancellation now releases the run and opens
  the next attempt. The sweep that removes verification branches also kept
  every ref naming the current base, including the ones whose pull request had
  moved on, and cancels a ref's queued run before deleting it.

- One owner of track analysis in `kithara-app`, `AnalysisService`, and one
  extent per pass in `kithara-analysis`. The grid is published at the tempo
  level the detector reports, tagged `grid_bpm_from_beats_v4`. Left: the
  reported deck scenario on a release build with the full model, and the size
  of the resume blob.

- `SpectralBeats`, a beat detector needing no model, beside the neural one. It
  searches the `Tempo` its caller hands it, and a build picks the model it
  embeds; the cache tag names both. Left: nothing.

- Harness and document revision. `AGENTS.md` routes instead of restating; the
  `style` namespace budgets documents with `doc_size`, blocks drift with
  `doc_staleness`, and holds every crate README to one shape with
  `readme_shape`. All three queues are at zero.

- Release optimization for every third-party package. `[profile.release]` sets
  `opt-level = "z"` workspace-wide, and the `[profile.dev.package.*]` block right
  above it is the repository's own list of what is too slow unoptimised - release
  threw that list away, so both time-stretch backends, the AAC decoder and the
  pure-Rust DSP all shipped compiled for size. `[profile.release.package."*"]`
  now raises every non-workspace package to 3 (329 on wasm, 339 Apple, 405
  Android, 708 workspace-wide); the workspace's own crates keep `"z"`. The
  override moves each build script's `OPT_LEVEL`, which is what reaches native C
  and C++. Captured A/B for `fdk-aac-sys`: 171 files, `-Oz` -> `-O3`, zero
  crossover, `aacdecoder.cpp` among them. `bungee-sys` is partial by construction
  - its `build.rs` reads `PROFILE`, not `OPT_LEVEL`, so CMake was already
  building the vendored core `Release`.

  Measured cost: `test-release` and `bench` inherit the overrides, so the
  seventeen lanes on that profile pay ~+82% compile CPU (319 of 676 units move
  from opt-level 0 to 3); the Apple release graph goes 476 -> 1105 CPU-seconds
  (2.32x); `web-size` +42% CPU and dist 3137 -> 3565 KiB. `build-override` does
  not beat the glob - verified in the unit graph - so build scripts cannot be
  held back while the natives move.

  Two side effects the glob cannot express. The `-Z build-std` sysroot is
  non-workspace, so its codegen moves to 3 while `optimize_for_size` still
  applies. And the TLS natives grow for nothing on symmetric crypto: their AES,
  ChaCha20-Poly1305 and SHA kernels are assembly (123 assembled `.S` objects in
  `btls-sys`, 98 in `aws-lc-sys`) that is byte-identical at `-Os` and `-O3`, so
  BoringSSL `libcrypto` `__TEXT` +55% buys throughput nowhere. Named
  `[profile.release.package.<name>]` entries beat the glob if they are pinned
  back.

## Next

- The workspace's own crates are still at `"z"` - `kithara-audio`, `-decode`,
  `-resampler` and the rest carry the size setting a per-package glob cannot
  reach. Raising them is a measured change of its own.
- `crates/kithara-ffi/.wasm-slim.toml` budgets the wasm bundle at
  29000/31000/33000 KiB against a May baseline of ~28.2 MiB, while a local
  `dist` weighs 3565 KiB. Either the gate is an order of magnitude stale or the
  two numbers weigh different things; the `web-size` lane on GitLab is the only
  place that settles it, and nothing here has run it.
- No runtime number backs the optimization yet. Decode throughput, stretch cost
  and render-budget headroom were never measured before or after, so the case
  rests on codegen rather than on a benchmark.
- Work the comment queue down by hand: `--fix` is exhausted for comments, so
  all 668 are decisions (497 body comments, 105 long doc blocks, 50 oversized
  inline comments, 16 dense functions).
- 439 ordering findings are mechanical; one `just lint style --fix` clears
  them but rewrites declarations across every crate, so it wants its own
  change.
- Wire `just lint style` to a gate: nothing runs it today. A warm run is 58 s,
  too much for every commit, nothing for a lane. The lane catalog owns that.

## Blocked

- Nothing.
