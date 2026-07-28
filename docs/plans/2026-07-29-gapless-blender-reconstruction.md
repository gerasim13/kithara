# Gapless Blender Reconstruction

## Goal

Deliver audibly gapless adaptive and manual non-seek variant changes, including
AAC to FLAC and FLAC to AAC, through one explicit decoder-generation state
machine and one always-present PCM blender. A seek cancels this lifecycle and
replaces it with one generation immediately; it never blends old and new seek
positions.

This is a reconstruction of the mechanisms that made the target oracles green,
not a clean-room rewrite. Proven mechanisms are retained behind canonical
owners. Codec guesses, finite preparation holdback, shadow state, and direct
reader-control handles are removed.

## Success Signal

- [ ] Automatic ABR and manual non-seek switches use the same
      `Transition` lifecycle.
- [ ] AAC/FLAC switches in all four backend/codec combinations have no added
      pause, click, duplicate span, or phase shift.
- [ ] Target preparation may take longer than any fixed audio reserve while the
      outgoing generation remains audible.
- [ ] One decoder generation is active normally; exactly two exist only while
      preparing and completing a transition.
- [ ] Every generation owns its decoder, reader session, decoder profile,
      cancellation scope, trimmer, and staged post-trim PCM.
- [ ] The blender is always in the PCM path. Its single-input state is an
      identity apart from one configured overlap-sized rolling window.
- [ ] Blend input is aligned only in canonical post-trim presentation frames.
- [ ] Effects and playback host-rate resampling run once after the blender.
- [ ] Seek cancels every transition descendant, discards staged PCM, advances
      the epoch, and installs one `Replace` generation without mixing.
- [ ] The committed target oracles remain strict and non-vacuous.
- [ ] Every production commit after the preserved red oracles first become
      green passes its scoped tests and the complete `just test` gate in a
      clean checkout of that exact commit. Earlier reconstruction commits
      compile the complete test graph and introduce no failures beyond the
      explicitly preserved target-red set.
- [ ] The final complete gate reports all workspace tests green, and a manual
      switch in `kithara-app` is audibly gapless.

## Evidence Baseline

The architecture is derived from these observed states:

- The historical target-green state is the union of tracked snapshot
  `0f3c95d5800c` and untracked snapshot parent `4e045c6edf6b`.
- The preserved target oracles are committed in `a46db92ff`.
- `a46db92ff` also preserved two HLS target-body owner tests whose production
  API was still only in the dirty WIP. At `884c01c91` the complete test graph
  therefore stops at compile time on the missing `ReadLease` target-body
  contract. That owner contract is restored before adding another oracle; it
  is not replaced with a test edit or a compatibility shim.
- The existing ownership contract is committed in `5c27aa29b`.
- `c76a1557e` made a local Symphonia AAC emitted-frame measurement explicit,
  but also let that measurement redefine global seek/duration coordinates.
  Near-end seek regressions followed, so `884c01c91` reverted it.
- The current dirty WIP made target oracles green through a mixture of genuine
  mechanisms and accidental compensations. It is evidence, not the target
  architecture.
- A clean full-gate comparison around `c76a1557e` showed that target-only green
  results cannot replace the workspace gate.
- Manual `kithara-app` AAC to FLAC playback still exposed an audible pause, so
  test green alone is not final acceptance.

## Evidence Matrix

| Oracle or observation | Mechanism that produced green | Root cause it exposed | Shared invariant | Decision |
| --- | --- | --- | --- | --- |
| Four-direction manual AAC/FLAC continuity | Independent incoming HLS reader, exact transition identity, outgoing decode continuing during target construction, post-trim frame alignment | One byte cursor and one decoder cannot cross a codec/container boundary without losing readable outgoing state | Outgoing and incoming generations own independent pinned sessions; publication changes only after audio commits the exact incoming generation | Retain |
| Delayed target rebuild continuity | Outgoing stayed live while target bytes and decoder were prepared | Target construction can exceed any fixed lead and compressed read-ahead can be far ahead of audible PCM | Preparation never consumes a finite audible reserve; target catches the current presentation frontier while outgoing continues | Retain the lifecycle; delete the fixed lead |
| Current 250 ms holdback | `DecoderHandoff::lead`, `Keep::Lead`, and playing holdback masked preparation latency | A finite reserve can hide one delay but necessarily fails for a longer delay and adds latency in the steady path | Only the configured overlap window may be retained in the steady path | Delete |
| Sweep timeline oracle | Per-generation trimmer, format-boundary finish, origin rebasing, exact frame-range checks | Generations have different priming, queued-output, and container timestamp behavior | Each generation converts its own decoder output to one canonical post-trim presentation timeline before blending | Retain the invariant; replace codec guesses |
| Measured Symphonia AAC lag | 661 frames of FDK output delay plus one 1024-frame queued access unit explained the 1685-frame seam | The packet fed to a queue codec is not necessarily the PCM returned by that call | A decoded chunk timestamp names its first emitted PCM frame, not the triggering input packet | Retain inside the Symphonia codec adapter |
| Same-codec AAC switches | Equivalent delays on both sides often cancelled | Mutual error can make a same-codec oracle green while cross-codec alignment is wrong | Acceptance must cover all cross-codec directions and independent source correlation | Do not treat same-codec green as sufficient |
| Click/onset oracle | Short equal-gain ramp | Both sides are correlated copies of the same music; equal-power would create a 3 dB onset | One shared equal-gain law over one exact overlap window | Retain |
| `target=NaN` near-end regressions after `c76a1557e` | A generation-local emitted counter was injected into global `ComposedDecoder` seek/duration semantics | Container, emitted-output, and presentation clocks were collapsed into one mutable value | Translate clocks at explicit boundaries; never let an AAC queue clock redefine demux duration or seek authority | Retain the local measurement; reject that ownership |
| Seek/transition races | Exact handoff tickets and seek epochs prevented stale promotion | A stale transition can attach to bytes or intent from another seek epoch | Seek has priority and performs `Cancel + Replace`; non-transition rebuilds never inherit a transition ticket | Retain |
| Historical `variant_reader_control` | The green snapshot put `pin`, `finish_construction`, and `release` on an `Arc` handle returned by `Decoder` | Each decoder generation needs reader-session lifecycle, but audio reached through the decoder into HLS ownership | Keep the session-bound lifecycle, but route it through the existing decoder/reader hook with typed actions; do not expose a mutable control handle | Replace the shape |
| Historical `content_origin_frames` and `blend_duration` | Separate ad-hoc `Decoder` methods supplied trim/origin and mixer behavior | A cohesive decoder output contract was missing | One standard decoder profile supplies trim, blend alignment, and reader input facts; mixer policy remains audio-owned | Replace with the profile |
| Target body retention from decoded frontier | HLS planned target retention from the PCM frontier rather than the compressed cursor | Apple and other demuxers can read far ahead of PCM actually emitted | Retention follows the exact outgoing reader session's decoded PCM frontier | Retain; move feedback to the session-bound hook |
| Current reader seek hook | `ReaderSeekSignal` already carries `landed_byte` and `PrerollHint`, but HLS leaves preroll as a TODO | The decoder knows its exact seek landing and warm-up requirement; HLS owns byte retention/fetch | Decoder reports byte-domain facts; HLS executes them without codec knowledge | Complete the hook contract |

### Oracle limitations that must not become acceptance loopholes

- Delayed-target and desktop-shaped tests must prove live target PCM after the
  transition, not only compare silence/click metrics over possibly empty
  output.
- A threshold derived from the same error distribution under test cannot be
  the only click/pause bound.
- A sine recurrence residual does not prove absolute phase. The independent
  sweep/source correlation remains required.
- A codec-control assertion must not derive its expected origin from the same
  codec table used by production.
- Existing thresholds are never relaxed to fit the implementation. Any
  non-vacuity assertion is added in a separate tests-only commit.

## Decoder And Backend Matrix

| Backend / codec | Proven decoder behavior | Profile resolution | Reader requirement | What audio sees |
| --- | --- | --- | --- | --- |
| Apple AAC | `AudioConverterPrimeInfo` may become available only after the first decode call. Apple also has explicit AAC seek warm-up requirements. | Reader facts are available at construction. Output profile may remain `Pending` until PrimeInfo and the first output are known, then becomes immutable. | Init/landing range plus the exact seek preroll reported by the decoder hook | One resolved trim profile and one post-trim presentation anchor; no Apple branch |
| Apple FLAC | Prime information is normally available at open and output is effectively immediate | Explicit identity/zero-delay output profile, ready at construction | Incremental or init requirement selected by the actual demuxer; no AAC preroll | The same generic generation contract |
| Symphonia AAC / FDK | A packet can be consumed with zero PCM; a later call can return PCM belonging to an earlier access unit. FDK output delay is backend-local. | The codec adapter owns the queued input-PTS to emitted-PCM mapping. The profile becomes ready only when the mapping is stable. | Segment/init requirement from the chosen demuxer; seek warm-up stays decoder-owned | Correctly labelled emitted PCM and a canonical presentation anchor; no `AAC = 1024` guess |
| Symphonia FLAC | Packet PTS and emitted PCM are effectively one-to-one | Explicit identity profile, ready at construction | Incremental input and ordinary seek | The same generic generation contract |

FLAC does not inherit a default profile accidentally. It implements the
identity profile explicitly. AAC differences remain inside the Apple and
Symphonia adapters and never appear as codec-enum branches in
`kithara-audio`, `kithara-hls`, or the blender.

## Decoder Profile Contract

Every runtime decoder implements one standard profile provider. The aggregate
profile has three projections with different consumers:

```rust
trait DecoderProfile {
    fn reader_profile(&self) -> ReaderProfile;
    fn output_profile(&self) -> OutputProfileState;
}

enum OutputProfileState {
    Pending,
    Ready(DecoderOutputProfile),
}

struct DecoderOutputProfile {
    trim: TrimProfile,
    timeline: TimelineProfile,
}
```

The names above describe ownership and may reuse existing domain types during
implementation. Their semantics are frozen:

- `ReaderProfile` contains only transport-neutral byte-domain requirements:
  demux construction shape, init/header requirement, bounded read-ahead, and
  decoder warm-up requirements. Its canonical type lives in
  `kithara-stream`, so the base stream never depends on `kithara-decode`.
- `TrimProfile` contains the fully resolved leading/trailing content trim and
  output-domain tail compensation for this decoder generation.
- `TimelineProfile` is the blender-facing decoder profile. It contains an
  exact anchor from the generation-local emitted
  PCM frame clock to the canonical post-trim presentation-frame clock, plus
  the PCM specification in which that anchor is expressed.
- `TimelineProfile` does not contain gain law, overlap duration, fallback policy,
  or HLS state. Those are audio transition policy.
- `Pending -> Ready` is a one-way transition. A ready profile is immutable.
  A later change is a typed decoder contract failure.
- A decoder may consume input while its output profile is pending, but the
  canonical staged-PCM queue remains empty. At most the first unprofiled
  output is held in a decoder/bootstrap slot. When a decode call both resolves
  late Apple metadata and returns PCM, the profile is frozen before that PCM
  enters the trimmer.
- `ComposedDecoder` combines demux/container metadata with codec-adapter facts
  and delegates the standard trait. It does not guess from `AudioCodec`.
- `ResampledDecoder` delegates the profile and transforms every frame-domain
  value into its actual output rate exactly once.
- `DecoderTrackInfo`, `default_priming_frames`, `content_origin_frames`, and
  `DecoderHandoff` must not remain parallel mutable or fallback sources. The
  profile becomes canonical in one staged migration and the old accessors are
  removed.

### Boundary semantics

The profile describes decoder output; generation context describes why the
decoder exists:

- `TrackStart` applies the profile's track-leading and trailing contract.
- `FormatBoundary` creates a fresh local trimmer and emitted clock, aligns the
  new generation to the continuing presentation clock, and finishes the
  outgoing generation with `GaplessEnd::FormatBoundary`.
- `Seek` discards the old generation, uses decoder seek/preroll to land at the
  requested presentation point, and does not blend.
- `TrackEof` is the only boundary that applies logical-track trailing trim.

There is no blanket `notify_seek()` on a format-boundary successor.

## Decoder / Reader Hook Contract

The existing single-owner decoder-to-reader hook becomes the only feedback and
reader-session lifecycle boundary. It replaces the historical
`Arc<dyn VariantReaderControl>` exposed by `Decoder`.

The hook has one concrete delegation path:

```text
DecoderGeneration
    -> Decoder::reader_action(action)
    -> ComposedDecoder
    -> its owned ReaderEventSink
    -> the exact HLS/file reader session
```

`Decoder::reader_action` is one typed delegation method, not a returned handle.
Decoder-originated `profile/chunk/seek` facts travel in the opposite direction
through the same owned hook. Audio never receives the hook or HLS session
object itself.

The hook carries two classes of typed messages:

1. Decoder facts:
   - install the immutable `ReaderProfile` once;
   - report each emitted chunk's `ChunkPosition`, including its exact decoded
     presentation frontier and source-byte facts;
   - report `landed_byte` and exact `PrerollHint` after seek;
   - report pending and terminal outcomes.
2. Generation lifecycle:
   - pin the outgoing session for one exact transition ticket;
   - mark the incoming session construction-ready;
   - commit/publish the exact incoming session after blend completion;
   - abort a ticket;
   - retire the session.

The hook is bound to one reader session when that reader is opened. Therefore
messages cannot accidentally target a later global "current reader".

RT rules:

- Per-chunk and per-seek calls perform only fixed-size, lock-free state updates
  or enqueue one typed deferred action.
- Resource retention, publication, cancellation, and destruction run from the
  existing off-RT `flush`/scheduler shell.
- No lossy queue is allowed for lifecycle messages. At most one exact
  transition is owned by a generation, so a fixed state slot or typed
  backpressure is sufficient.

The hook transmits only the reader projection of the decoder profile. Trim
frames, blend anchors, and gain policy never flow into the base byte stream.
HLS receives byte requirements and exact frontiers, not codec identities.

## Clock Domains And Translation Owners

Three clocks exist. Treating them as one caused the reverted AAC regression.

| Clock | Meaning | Canonical owner | Allowed consumer |
| --- | --- | --- | --- |
| Container clock | Demux packet PTS, duration, seek target, landed position | Demuxer and decoder seek implementation | Decoder construction and seek only |
| Emitted PCM clock | Frames actually returned by this codec instance, including queued-output behavior | Concrete codec adapter and its decoder generation | Profile resolution and per-generation trim |
| Presentation clock | Audible logical-track frames after decoder-specific trim | `DecoderGeneration` | Blender, effects, playhead, and output |

Translation is one-way:

```text
container packet
    -> codec-local queued-output mapping
    -> generation-local emitted PCM
    -> resolved TrimProfile
    -> TimelineProfile presentation anchor
    -> canonical presentation PCM
```

`PcmMeta` leaving `DecoderGeneration` names the canonical presentation span.
The demuxer retains its own container duration and seek state. No presentation
counter is written back into the demuxer or global decoder duration.

## Canonical Owners

| Owner | Owns | Does not own |
| --- | --- | --- |
| ABR | Revisioned automatic/manual selection intent and committed current variant value | Sessions, bytes, decoders, blending, rollback |
| HLS transition owner | Exact ticket, outgoing and incoming `ReadSession`, target byte preparation, retention, publication, abort | Decoder state, trim, PCM, gain |
| Decoder adapter | Backend packet/output mapping and decoder profile resolution | HLS sessions or transition policy |
| `DecoderGeneration` | Decoder, one reader hook/session binding, child cancellation scope, resolved profile, local trimmer, staged canonical PCM | ABR decision state or global publication |
| `DecodeCore` | Exactly one `ActiveDecode` state and its generation lifecycle | Protocol-specific byte planning |
| `ActiveDecode` | `Single` or exact two-generation `Blend`, rolling overlap window, equal-gain DSP | Decoder construction, HLS, seek intent |
| `SeekEngine` | Seek target and producer epoch | Transition promotion or blending |
| Scheduler shell | Deferred reader actions and off-RT retired-generation destruction | Domain state duplicated from the owners above |

ABR publishes an exact immutable intent. HLS reserves it as one exact ticket.
Audio carries that ticket through one generation lifecycle. The ABR committed
variant changes only when HLS publishes the exact session that audio has
finished promoting.

## PCM Topology

```text
outgoing decoder -> output profile -> per-generation trimmer --\
                                                              \
                                                               -> always-on blender
                                                              /        |
incoming decoder -> output profile -> per-generation trimmer --         v
                                                        effects -> host resampler -> output
```

- In `Single`, one generation feeds the blender. The blender keeps only its
  configured overlap-sized rolling window and otherwise behaves as an
  identity.
- Target construction and catch-up do not enlarge this window.
- The incoming generation decodes from its pinned target session and catches
  the outgoing presentation frontier off output.
- `ReadyToBlend` is a proof containing one ticket, one seek epoch, one
  `PcmSpec`, and one exact canonical frame range covered by both generations.
- `Blend` consumes only that range. A range/spec/epoch mismatch emits nothing
  and returns a typed error.
- The ramp is equal-gain because both inputs are correlated versions of the
  same signal.
- Decoder-internal normalization required to produce the declared blend spec
  remains inside the decoder adapter. Playback effects and host-route
  resampling occur once downstream.
- No allocation, sorting, locking, decoder construction, resource release, or
  destructor runs in the sample-mixing path.

## Transition State Machine

```text
Single(outgoing)
    |
    | Transition(ticket)
    v
Preparing(outgoing, ticket, incoming session)
    |
    | decoder built
    v
Priming(outgoing, incoming generation)
    |
    | profiles ready + exact overlap proof
    v
Blending(outgoing, incoming, overlap)
    |
    | overlap consumed + exact publish succeeds
    v
Single(incoming) -> retire outgoing off RT
```

State rules:

- `Single`, `Preparing`, and `Priming` all continue emitting outgoing PCM
  through the same blender.
- A newer non-seek intent during `Preparing` or `Priming` aborts that exact
  incoming ticket and starts the newer one from the still-audible outgoing
  generation.
- `Blending` is a short atomic PCM operation. A newer non-seek intent is kept
  as the latest revision and starts after the current overlap completes.
- Incoming construction failure, target EOF, profile failure, or readiness
  mismatch aborts the exact target and leaves outgoing ownership unchanged.
  It is reported as a typed transition failure; there is no automatic fallback
  chain or retry with changed semantics.
- Outgoing retirement begins only after both blend completion and exact HLS
  publication succeed.
- A stale ticket or seek epoch can never publish.

## Seek Is Replace

At every state, a seek performs the same ordered operation:

1. Cancel the transition child scope and close its exact HLS ticket.
2. Discard outgoing/incoming rolling and staged PCM.
3. Retire both transition generations off RT.
4. Advance the seek/decode epoch and clear effect/output state owned by the old
   epoch.
5. Resolve the current manual or ABR intent.
6. Open one reader session and construct one replacement generation.
7. Seek/prime that generation and install it as `Single` without a ramp.

Seek never promotes an in-flight incoming generation, drains the old position,
or inherits a transition ticket. Route/device/recovery replacement uses the
same explicit `Replace` plan unless its owning contract states otherwise.

## Typed Failure Semantics

- `ProfilePending`: incoming generation is still priming; outgoing continues.
- `ProfileChangedAfterReady`: decoder contract failure; abort target.
- `TicketStale` or `EpochStale`: close target without publication.
- `InputNotReady`: remain in `Preparing`/`Priming` with the same exact
  operation; do not mutate its identity or reinterpret it as success.
- `OverlapMismatch`: emit no mixed PCM and abort target.
- `TargetEof`: abort target; outgoing remains.
- `ReplaceFailed`: report the seek/replacement failure; do not resurrect a
  cancelled transition generation.

`SoftFailed`, "minimum coverage means ready", adaptive retries with changed
state, sentinel values, and fallback codec/origin tables are forbidden.

## Retain And Delete Map

### Retain behind the new owners

- Independent generation-bound HLS reading sessions.
- Exact `VariantHandoff` / transition identity and seek-epoch checks.
- Decoder-local queued-output timestamp mapping.
- Per-generation `GaplessStage` and `GaplessEnd::{FormatBoundary, TrackEof}`.
- Contiguous staged PCM range primitives.
- Typed readiness proof concepts.
- Equal-gain blend math.
- Effects-once placement after blend.
- Off-core decoder construction and off-RT retirement.
- Reader hook ownership by the decoder instance.

### Delete or replace

- `DecoderHandoff::lead`, `Keep::{Lead, Ramp}`, ramp prefix, and the 250 ms
  preparation reserve.
- Audio-layer codec-origin tables, `anchor_codec`, and
  `AudioCodec::encoder_priming_frames` as a blend coordinate.
- Historical `Decoder::content_origin_frames`, `blend_duration`, and
  `variant_reader_control` as unrelated methods.
- Direct `Arc<dyn VariantReaderControl>` access from audio.
- A shared/global decoded frontier read by HLS when the exact outgoing reader
  hook can own it.
- `TransitionMode` and `tick_outgoing` as a second lifecycle API.
- `Option<ActiveDecode>` temporary ownership sentinel.
- Seek cancellation that promotes incoming.
- `SoftFailed`, degraded readiness, lossy completion handling, pending-result
  overwrite, and fallback/retry chains.
- Shadow flags including `sealed`, `seeked`, `superseded_seek`,
  `blending_handoff`, and `pending_handoff_close`.
- Any manual queue sorting. Decode and lifecycle order remain FIFO and typed.

## Affected Paths

- `crates/kithara-decode/src/{traits,codec,composed,resampled,types}.rs`
- `crates/kithara-decode/src/{apple,symphonia}/`
- `crates/kithara-stream/src/{hooks,reader,source,variant,publication}.rs`
- `crates/kithara-hls/src/{reader,peer,stream}/`
- `crates/kithara-abr/src/{controller,state,handle}.rs`
- `crates/kithara-audio/src/pipeline/{blend,decode,gapless,rebuild,seek,track}/`
- `crates/kithara-audio/src/{audio,renderer}/`
- `crates/kithara-play/src/player/`
- `tests/tests/kithara_play/quality_switch_continuity/`

`.config/arch/thresholds.toml`, unrelated asset-cache changes, and unrelated
Apple RIFF accounting are outside every production slice.

## Execution Isolation

- Production commits are built only in
  `/Volumes/Render/dev/multistream-read/wt-gapless-reconstruction` on
  `codex/gapless-blender-reconstruction`, starting from `884c01c91`.
- `/Volumes/Render/dev/multistream-read/wt` remains untouched and read-only. Its
  tracked and untracked WIP is evidence, not an execution surface.
- The staged plan from the source worktree is copied into the reconstruction
  worktree and committed as P0. No source-worktree stash, reset, checkout, or
  branch-ref update is permitted.
- `btls-sys` is prebuilt outside the Git hook with Git worktree variables
  removed. Dependency versions and repository source are not changed to work
  around the hook environment.
- Production writes are serialized by commit slice. Read-only review may run in
  parallel only after a slice has a stable diff.

## Required Reads

- `AGENTS.md`
- `docs/workflows/rust-ai.md`
- `docs/guides/architecture-shape.md`
- `docs/guides/cancel-policy.md`
- `docs/guides/performance.md`
- `docs/guides/test-harness.md`
- `crates/kithara-audio/CONTEXT.md`
- `crates/kithara-decode/CONTEXT.md`
- `crates/kithara-hls/CONTEXT.md`
- `crates/kithara-abr/CONTEXT.md`
- `crates/kithara-stream/CONTEXT.md`
- `../handoffs/HANDOFF-2026-07-28-S6-for-codex.md`

## Constraints

- Preserve all current WIP until its proven mechanisms have been mapped into a
  committed slice. No destructive reset, checkout, or stash operation.
- Treat the historical green snapshots and current dirty WIP as evidence, not
  code to port wholesale.
- No production commit may depend on later uncommitted WIP.
- No test is weakened, skipped, or rewritten to match an implementation.
- No public API is widened beyond the profile/hook contract without updating
  its canonical owner documentation and tests in the same slice.
- Every mutable fact has one owner. Read-through compatibility accessors may
  exist only inside one migration commit and cannot become a second source.
- Cancellation is descendant-scoped and propagated down.
- Hot-path work is bounded, allocation-free after warm-up, unsorted, and
  lock-free.
- Effects are applied once.

## Commit Sequence

Each production commit is self-contained and validated before the next starts.
If its exact clean commit fails the full gate, the slice is redesigned or
reverted; no later situational fixes are stacked on it.

### P0. `docs: freeze gapless blender reconstruction`

- Commit this plan only from the clean reconstruction worktree.
- Validation: document paths, refs, and repository formatting/hygiene.
- No runtime claim.

### P0.5. `fix(hls): restore target-body oracle dependency`

- Make `ReadLease` the owner of the exact ordered target-body fetch window
  already required by the committed HLS tests.
- Land the first production transition consumer in the same commit; an API
  reachable only under `cfg(test)` or left dead until a later slice is a
  forbidden compatibility shim.
- Build the target plan directly in canonical order; never collect, sort, or
  positionally insert into the fetch queue.
- Preserve the existing preallocated queue and its exact-size priority
  semantics. Rebuilding a newer immutable target-coverage version must not
  allocate after lease construction or lose in-flight slot state.
- Validate the two existing target-body owner tests and compile the complete
  integration-test graph before adding any new test.

### P1. `test: require live PCM after decoder transition`

- Add only missing non-vacuity assertions to the already committed oracles.
- Do not change thresholds or expected behavior.
- Validate the focused oracle set, then classify the complete `just test`
  result against the preserved target-red set.

### P2. `refactor(decode): expose one decoder transition profile`

- Add the standard profile provider and explicit FLAC identity profiles.
- Implement Apple AAC late-ready and Symphonia AAC queued-output facts inside
  their adapters.
- Delegate and scale through composed/resampled decoders.
- Replace `track_info`, default priming, content-origin, and handoff fallbacks
  without creating a parallel source of truth.
- Preserve global demux seek/duration behavior.

### P3. `refactor(stream): route decoder facts through the reader hook`

- Extend the session-bound hook with immutable reader profile, exact chunk
  frontier, seek/preroll, and typed lifecycle actions.
- Remove direct reader-control handles from `Decoder`.
- Make HLS target retention use the exact outgoing session frontier.
- Keep file/progressive readers as explicit no-op/identity implementations.

### P4. `refactor(audio): make DecoderGeneration the lifecycle owner`

- Fold `DecoderSession`, the current blend side, profile, trimmer, staged PCM,
  and child cancellation into one type.
- Store `ActiveDecode` directly in `DecodeCore`.
- Preserve single-generation output behavior and effects order.
- Move retirement obligations to the scheduler shell.

### P5. `refactor(hls): reduce transition ownership to one exact ticket`

- Keep one outgoing and at most one incoming session.
- Reduce ABR to revisioned intent and commit acknowledgement.
- Remove HLS/ABR rollback shadows, parallel current-variant values, and lossy
  completion paths.
- Preserve pinned outgoing readability and incoming construction.

### P6. `feat(audio): blend exact canonical overlap`

- Replace finite preparation holdback with the overlap-sized rolling window.
- Build `ReadyToBlend` from exact ticket/epoch/spec/frame coverage.
- Mix one equal-gain window, publish incoming, and retire outgoing.
- Remove codec-origin correction and silent partial-mix exits.

### P7. `fix(audio): implement seek as cancel plus Replace`

- Make seek preempt every transition state.
- Discard all staged PCM and install one generation from current intent.
- Remove seek promotion/drain behavior and transition-ticket inheritance.

### P8. `refactor(audio): keep effects and host resampling after blend`

- Remove any duplicate per-side playback effects/host-rate resampling.
- Retain only decoder-internal output normalization required by the profile.
- Validate the `kithara-app`-shaped Apple path and route-change behavior.

### P9. `refactor: remove obsolete transition scaffolding`

- Delete old flags, fallback branches, unused adapters, and temporary
  compatibility accessors.
- Run code hygiene and full diff review.

## Validation Scope

For every production slice:

1. Format and compile the exact affected crates.
2. Run their focused unit/owner tests.
3. Run the complete committed quality-switch oracle set, including delayed
   target, independent sweep timeline, desktop-shaped Apple, and all four
   codec directions.
4. Run isolated seek/ABR regressions that previously failed, including
   near-end seek and stress cases.
5. Before the preserved target set first becomes green, commit only when the
   complete test graph compiles and every failure is in the explicit target-red
   set. After that point, commit only when every probe is green.
6. Validate a clean detached checkout of that exact commit with `just test`.
   Record the total count and exact expected-red set during reconstruction; a
   small crate count or 229 tests is not the full gate. The final result has no
   expected-red exception.

Final validation:

- One complete `just test` gate from the final clean commit.
- Re-run the gapless target set as one invocation to expose shared-state races.
- Run the relevant stress repetitions with no concurrent heavy gate.
- Launch `kithara-app` with the production Apple decoder path and manually
  switch AAC to FLAC and back without a seek.
- Inspect the full campaign diff against `e517d428e`, removing obsolete
  scaffolding and confirming every changed file belongs to a named owner.

## Split Map

- Evidence-only agents may inspect decoder/backend, oracle, and byte-stream
  contracts in parallel.
- Production writes are serialized by commit slice because the profile, hook,
  generation, and transition state machine are sequencing dependencies.
- A separate reviewer may inspect a completed slice read-only while the
  integrator prepares validation, but no two writers touch the same owner.

## Sequencing Dependencies

- The profile contract lands before audio generation ownership.
- The reader hook contract lands before HLS transition simplification.
- Generation ownership and exact HLS sessions land before the real overlap
  blend.
- The blend lands before seek removes the old promotion path.
- Cleanup lands only after behavior and the full gate are green.

## Integrator

- `/root`: owns the architecture contract, commit boundaries, clean full-gate
  proof, final diff review, and user-visible handoff.

## Risks And Non-Goals

Known risks:

- Apple AAC profile metadata resolves later than FLAC metadata.
- Queue codecs require exact emitted-output labelling without changing demux
  seek/duration authority.
- A hook that grows into a generic control facade would recreate the current
  architecture problem. Its messages stay reader-session-specific.
- A dirty worktree can mask missing commit dependencies. Every production
  commit therefore requires clean-checkout validation.
- Manual app acceptance can reveal a pause that synthetic metrics miss.

Non-goals:

- No gapless mixing across a user seek.
- No general cross-track DJ crossfade.
- No codec-specific mixer implementations.
- No ABR policy rewrite.
- No new retry or degraded-mode system.
- No cache eviction redesign beyond retaining the exact ranges required by
  the active reader sessions.
- No unrelated asset, RIFF, lint-threshold, or test-fixture cleanup.
