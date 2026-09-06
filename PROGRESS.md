# Progress

What is in flight right now. The
[GitHub Projects board](https://github.com/users/gerasim13/projects/3) owns
capability status and the roadmap, and git owns the facts. This file owns
intent: what is being worked on, what comes next, what is stuck. Update it in
the change that lands the work, and keep it short.

## In Flight

- Harness and document revision. `AGENTS.md` routes instead of restating, and
  the `style` namespace now budgets documents with `doc_size`, blocks drift with
  `doc_staleness`, and holds every crate README to one shape with `readme_shape`:
  a header that stays inside the package, badges keyed to `publish` and to the
  manifest's license, a `# <package name>` title, then `Usage` / `Key Types` /
  `Features` / `Integration` and nothing else. All three queues are at zero, and
  the rewrites turned up claims the sources contradict - a wrong feature list, a
  file that no longer exists, an inverted description of a known leak, an MPL-2.0
  crate wearing the MIT badge, two crates naming a dead owner, and a logo no
  published crate page could load.

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

  `crates/kithara-ffi/.wasm-slim.toml` carried a 29000/31000/33000 budget from a
  May build made with no `[profile.release]` and default features - roughly nine
  times the real bundle, so the gate could not have caught an eightfold
  regression. Reset to 3600/4000/4500 against the measured 3565 KiB.

## Next

- The workspace's own crates are still at `"z"` - `kithara-audio`, `-decode`,
  `-resampler` and the rest carry the size setting a per-package glob cannot
  reach. Raising them is a measured change of its own.
- No runtime number backs the optimization yet. Decode throughput, stretch cost
  and render-budget headroom were never measured before or after, so the case
  rests on codegen rather than on a benchmark.

- Work the comment queue down by hand. `--fix` is exhausted for comments - a
  second run on a clean tree changes nothing - so all 668 are decisions: 497
  comments carrying prose outside a doc comment, 105 doc blocks past a dozen
  lines, 50 oversized inline comments, 16 dense functions. A body comment has no
  mechanical destination.
- 439 ordering findings are still mechanical: `struct_field_order` 160,
  `trait_item_order` 188, `struct_init_order` 91. One `just lint style --fix`
  clears them, but it rewrites declarations across every crate, so it wants its
  own change.
- Wire `just lint style` to a gate. Nothing runs it today - not the commit hook,
  not a CI lane - which is why the ratchet drifted unseen. A warm run is 58 s:
  too much for every commit, nothing for a lane. The lane catalog owns that
  change, so it does not belong in this one.

## Blocked

- Nothing.
