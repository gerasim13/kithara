# 2026-08-13-cochlea-sync-test-matrix

## Goal

Add one deterministic acceptance matrix that proves Kithara keeps two to four
decks clean and beat-aligned through the public playback path for synthetic,
MP3, distinct-track, and locally served Silvercomet HLS media. Every rendered
audio scenario uses Cochlea on deck stems, the final mix, and a time-aligned
solo control; state, frame, latency, performance, and RT properties keep their
native typed oracles alongside Cochlea.

## Success Signal

- [ ] The source axis contains named cases for synthetic distinct tracks, the
      same MP3, distinct MP3s, the same Silvercomet HLS asset, and HLS plus MP3.
- [ ] Deck B starts exactly three eighths of its analyzed beat after deck A and
      every sync case first proves the decks were audibly out of phase.
- [ ] Cochlea rejects injected silence, clipping, and a one-quantum phase shift
      before it is trusted as the continuity and beat oracle.
- [ ] The matrix covers ownership/readiness, stopped and already-playing decks,
      two and four decks, operation order, tempo rides, sync lifecycle, seek,
      reload, load, HLS ABR, sample-rate and render-quantum axes.
- [ ] Command-to-audible latency, plan cost, RTSan, underrun, and xrun contracts
      retain their native exact oracles; Cochlea runs outside measured hot
      sections. A real bound-path no-allocation probe remains a separate
      follow-up until it can fail without aborting the test process.
- [ ] No sync behavioral acceptance uses the external network, sleeps to create
      a musical offset, or bypasses Queue/Player/OfflineSession for its PCM.
- [ ] Existing no-sync unity passthrough remains bit-identical and Cochlea-clean
      with and without bounded shared-worker load.
- [ ] A native app test presses the real raw UI SYNC controls and covers the
      pending first grid, primary adoption, actual selected tracks, final PCM,
      and SYNC disable without calling the transport helper directly.
- [ ] Only test code, fixture media, test dependencies/routing, CI test wiring,
      benchmarks, and this plan file are committed; the dirty production WIP
      remains unstaged.
- [ ] Setting `KITHARA_SYNC_ARTIFACT_DIR` writes listenable deck stems, final
      mix, time-aligned control, and a machine-readable Cochlea manifest even
      when the case later fails; the default run writes nothing.
- [ ] Setting `KITHARA_SYNC_LIBRARY` adds a reproducibly selected local-library
      pair, records its exact paths and seed, and reuses content-addressed
      analysis fixtures instead of reanalysing unchanged tracks.
- [ ] Repository MP3 and HLS rows load checked-in, content-bound complete
      `TrackAnalysis` sidecars and never run BeatThis during the sync scenario.
- [ ] A separate cold-analysis acceptance requests SYNC before the final map is
      ready, preserves uninterrupted free PCM, then converges after publication
      without reload or seek.

## Affected Paths

- `docs/plans/2026-08-13-cochlea-sync-test-matrix.md`
- `assets/sync-silvercomet.mp3`
- `assets/sync-analysis/*.ksan`
- `Cargo.lock`
- `crates/kithara-app/Cargo.toml`
- `crates/kithara-app/src/gui/mod.rs`
- `crates/kithara-app/src/gui/sync.rs`
- `tests/src/cochlea.rs`
- `tests/src/sync_artifact.rs`
- `tests/src/sync_fixture.rs`
- `tests/src/sync_matrix/*.rs`
- `tests/src/lib.rs`
- `tests/src/offline/player.rs`
- `tests/tests/kithara_audio/stream_source_tests.rs`
- `tests/tests/kithara_queue/sync_behavioral_matrix.rs`
- `tests/tests/kithara_queue/sync_latency.rs`
- `tests/tests/kithara_queue/sync_library.rs`
- `tests/tests/kithara_queue/sync_media.rs`
- `tests/tests/kithara_queue/sync_rt.rs`
- `tests/tests/suite_sync_acceptance.rs`
- `tests/benches/sync_plan.rs`
- `tests/Cargo.toml`
- `tests/build.rs`
- `.config/just/test.just`
- `.config/xtask.toml`
- `.github/workflows/rtsan.yml`

## Required Reads

- `AGENTS.md`
- `docs/workflows/rust-ai.md`
- `docs/guides/test-harness.md`
- `docs/guides/performance.md`
- `docs/guides/performance/benchmarking.md`
- `crates/kithara-audio/CONTEXT.md`
- `crates/kithara-play/CONTEXT.md`
- `crates/kithara-queue/README.md`
- `crates/kithara-queue/CONTEXT.md`

## Validation Scope

- Format only the owned test/fixture/benchmark paths through `just fmt`.
- Compile and run the smallest named case for each new harness layer first.
- Run the synthetic correctness cases before MP3 and local HLS cases.
- Run the focused latency and plan benchmark probes separately; do not use a
  shared-CI wall-clock result as a hard correctness gate.
- Run the complete local matrix only through the feature-gated
  `sync-acceptance` lane when the focused cases compile and the production WIP
  is capable of reaching the public sync state.
- Leave the full workspace/CI gate to the fork after the focused matrix is
  locally healthy, as requested.

## Split Map

- `integrator`: owns this plan, `tests/src/cochlea.rs`, `tests/src/sync_fixture.rs`,
  `tests/src/lib.rs`, suite routing, final staging, and the commit.
- `sync-core`: owns the common `tests/src/sync_matrix` harness and synthetic
  acceptance rows; forbidden from editing production or media fixture caching.
- `sync-media`: owns repository HLS/MP3 and opt-in library rows; forbidden from
  duplicating the common runner or weakening production analysis validation.
- `sync-budget`: owns `tests/tests/kithara_queue/sync_latency.rs`,
  `tests/tests/kithara_queue/sync_rt.rs`, and `tests/benches/sync_plan.rs`, plus
  the latency, deadline-load, RTSan, and planning-cost probes; forbidden from
  editing production, shared fixture truth, or behavioral acceptance files.

## Sequencing Dependencies

- The integrator freezes the shared `CochleaReport`, media-source descriptor,
  capture, and oracle APIs before the three file owners write against them.
- Named source cases are explicit rather than a hidden Cartesian loop so a
  failure identifies one source and one behavior.
- Repository media remain the deterministic CI truth. The local-library case
  sorts supported audio paths, selects a pair from `KITHARA_SYNC_LIBRARY` with
  `KITHARA_SYNC_LIBRARY_SEED`, and writes the chosen paths to the artifact
  manifest. It never silently substitutes another source when the opt-in path
  is absent or invalid.
- `KITHARA_SYNC_ARTIFACT_DIR` is the sole write opt-in. A case creates a
  collision-free subdirectory and writes IEEE-float WAV files for every deck,
  the final mix, and its time-aligned control before assertions, plus JSON with
  source identity, analysis identity, operation order, frame ledger, sync
  state, Cochlea metrics, and thresholds. Default and CI runs create no audio
  artifact directory.
- Track analysis is produced by `TrackAnalysisRunner` and cached under a key
  derived from media content identity plus analyzer configuration for opt-in
  library rows. Repository rows instead load checked-in complete analysis
  sidecars; missing, stale, corrupt, or mismatched sidecars are hard failures
  and never fall back to runtime analysis.
- Missing product capabilities are expressed through current public operations
  (for example successive analysis revisions through Queue) and must fail as a
  behavioral assertion, never through a compile error, private test hook, or
  fallback path. The machine-local library axis is explicitly ignored by
  default and becomes a hard setup contract when selected with `--run-ignored`.
- The benchmark and RT probes reuse the same source/map fixtures but never call
  Cochlea inside the measured or realtime section. This commit deliberately
  does not claim a bound-path allocation guarantee: the available process-wide
  allocator guard aborts instead of returning an attributable test failure.

## Integrator

- `/root`: owns the shared API, conflict resolution, focused validation, exact
  staging, and the final test-only commit summary.

## Risks And Non-Goals

- Known risks: current production WIP contains unrelated source-test compile
  blockers; focused test execution may be blocked before a new acceptance case
  runs. Local HLS and four-deck cases are intentionally heavy and live in the
  manual `sync-acceptance` suite rather than any default gate. Current public
  APIs do not yet expose a SyncGroup snapshot or a streaming BeatMap trait, so
  the test layer cannot manufacture private state to simulate them.
- Non-goals: no production fix, public API addition, external Silvercomet
  network access, device-dependent assertion in the deterministic suite, lint
  suppression, default-gate regression, giant nested-loop test, or weakening
  of existing PCM/frame thresholds. Local music is not copied into Git or the
  fixture cache.

## Understanding Summary

- The automated matrix must exercise the same public mix path the application
  uses, then leave an optional artifact a human can audition.
- The artifact is diagnostic evidence, not a replacement for Cochlea, exact
  PCM, state, or frame assertions.
- Both passing and failing cases must preserve their rendered audio when output
  was explicitly requested.
- CI and ordinary local runs must not write large audio files.
- Stable repository fixtures provide reproducibility; an opt-in music library
  broadens realism without becoming repository content.
- Real tracks must use production analysis and a durable content/config keyed
  analysis cache.
- Prepared-map and cold-analysis are separate contracts: the first isolates
  mixing from NN latency, while the second owns readiness and publication.
- Random library coverage must be replayable from the recorded seed and paths.

## Assumptions

- Diagnostic audio uses stereo IEEE-float WAV so rendering does not introduce
  an additional lossy encode or integer quantization seam.
- Artifact directories are user-owned after creation and are not automatically
  deleted.
- The initial local library root is `/Volumes/Render/music`, supplied explicitly
  through `KITHARA_SYNC_LIBRARY`; tests do not hard-code its existence.
- A library case without two supported readable tracks is a typed setup failure,
  not a skipped or silently degraded case.

## Decision Log

- Chosen: deterministic repository fixtures plus an opt-in external library.
  This keeps CI reproducible while enabling listening tests against real music.
- Chosen: derive one stable progressive MP3 from the vendored lossless
  Silvercomet HLS fixture. CI consumes the checked-in MP3 through the local HTTP
  server and never needs FFmpeg or the external network.
- Rejected: copy selected Paul Blackford files into `assets`. The collection is
  large and its redistribution rights are not established.
- Rejected: external library only. Paths and contents are machine-local, so it
  cannot be the sole regression contract.
- Chosen: one explicit artifact environment variable. Always-on output would
  grow CI disks and make ordinary test runs stateful.
- Chosen: deterministic seeded selection from a sorted library inventory. An
  unrecorded random choice cannot reproduce a failure.
- Chosen: content/config-addressed analysis cache. Caching by path or timestamp
  alone can serve a stale BeatMap after bytes or analyzer settings change.
- Chosen: checked-in repository `TrackAnalysis` sidecars use a stable schema,
  exact media digest, transport profile/variant, analyzer cache tag, and payload
  checksum. They do not use the ephemeral fixture-build hash and cannot silently
  invoke BeatThis when stale.
- Chosen: persist `TrackAnalysis`, not `TrackBeatMap`. The latter is derived for
  the host sample rate, so one serialized map would be wrong for the 44.1/48 kHz
  matrix.
- Chosen: cold analysis remains a distinct RED app lifecycle. Today an early
  SYNC receives `BindUnavailable`; the intended contract retains the request,
  keeps free PCM clean, and binds when the final map arrives without reload.
- Rejected: the first allocation-free probe. It rendered a direct tone instead
  of a bound Queue/Player deck, and the available allocator guard could either
  abort or become a no-op. Keeping it would create a false green result. A
  fail-closed allocator harness for the real bound callback is follow-up work.
