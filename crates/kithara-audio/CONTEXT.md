# kithara-audio — Context

Contracts and invariants. The README owns overview, features, and type inventory.

## Threads and transports

Four contexts touch one track. **Consumer thread** — `Audio<S>` (`AudioRead` +
`AudioSession` + `AudioControl`, umbrella `AudioReader`), normally the host audio
callback: never allocates, frees, or locks. **Playback worker** — one shared OS
thread and generic scheduler owned by `kithara_play::PlayWorker`.
`kithara-play` owns task registration, `DecoderNode`, final producer admission,
and the RAII lease; `Audio<S>` retains only a restricted wake capability. This
crate ends at the still-concrete `AudioSource` plus `PreparedAudioLane` seam and
contains no playback scheduler, Warp renderer, or playback effects. **Off-RT
rebuild** — `RebuildPort::submit` → `spawn_blocking_on`
on the tokio handle captured during audio preparation. **Downloader** — owned
by `kithara-stream`; this crate never spawns it and never reconstructs HLS/file
protocol policy.

### Musical-coordinate ownership

`kithara-warp` is the canonical owner of beat maps, session coordinates, and
the synchronization protocol. This crate publishes decoded signal and analysis
facts only; it neither defines Warp coordinates nor converts between parallel
map representations. The R7 audio seam does not advance a `WarpMap`, report
runtime map progress, or acknowledge rendered/presented synchronization.

`kithara-signal` is the canonical owner of decoded-signal values
(`AudioSpec`, `AudioChunkInfo`, `AudioChunk`) and pure sample/time math;
`kithara-bufpool` owns `SamplePool` and `SampleBuffer`. This crate owns the
runtime `AudioReader`, `AudioSource`, and observer protocols that transport
those values. It does not mirror their fields or re-export them through a
decoder-specific compatibility layer.

Transport (`runtime/ports.rs`): SPSC `ringbuf::HeapRb` plus a one-slot overflow
(`Outlet`/`Inlet`).

- **Backpressure.** `Outlet::try_push` parks one item in the overflow slot when
  the ring is full, so a producer emitting ≤1 chunk per tick treats it as
  infallible. Each tick starts with `Outlet::flush()`; if still full the node
  returns `TickResult::Backpressured` *without* ticking the FSM — every internal
  transition, seeks included, pauses until the consumer drains.
- **Wake.** A producer→reader ring push arms a coalesced atomic wake;
  empty-to-non-empty also enqueues `on_data_available`. The produce core never
  calls `ThreadWake::wake` or enters the kernel. The scheduler shell delivers
  the pending `ThreadWake` after a node visit and before reporting or removing
  the slot. An unregistered or cancelled node gets one final shell-side flush,
  so EOF and failure output cannot strand a blocked reader. The consumer
  snapshots `ThreadWake` before re-checking `try_pop`; a signal between the
  snapshot and park advances the gate sequence, so `wait_timeout` returns
  immediately while retaining its timed backstop. Consumer→worker wakes are an
  explicit capability: `ConsumerWakeMode::RealtimeDeferred` (the production
  default) only arms the scheduler's coalesced level after a successful ring
  pop, while `ImmediateOffRt` signals its `ThreadGate` immediately from a
  consumer known to run off the real-time thread. A seek-epoch drain coalesces
  all discarded entries into one wake after the drain rather than signaling per
  item.
- **Trash ring.** The RT consumer must never `free`, so spent pooled `AudioChunk`s
  go to a second ring drained by the play-owned `DecoderNode::recycle` on the
  worker. Capacity
  `audio_buffer_chunks + 2` absorbs a full forward-ring seek drain, making the RT
  push infallible.
- **Off-RT deferral.** Signals the forbid-blocking core must not make are armed
  on-core and flushed by the shell from the play-owned
  `DecoderNode::recycle`. The outlet flush
  delivers the consumer output wake (`ReaderOutputWake`), while
  `AudioSource::prepare_deferred` resolves source format/state before the
  play-owned effect service, while `AudioSource::finish_deferred` owns FSM
  lifecycle events (`DeferredBus<Event>`), the reader→peer wake
  (`ReadinessGate::flush_peer_wake`), and retired state (`Retired::drain`).
  `StreamAudioSource::drop` keeps one teardown flush after the scheduler's final
  pass: terminal-slot removal performs no further pass.

### Playback scheduling boundary

The generic scheduler, its `run_loop`, task slots, service classes, and
playback hang-watchdog policy belong to `kithara-play::PlayWorker`. This crate
publishes worker-neutral `AudioSource` outcomes and immediate/deferred wake
capabilities only. Analysis keeps its separate single-node runner on the
existing analysis thread; it does not expose or share the playback scheduler.

### Preload gate

`PreloadGate` is the one-time startup signal releasing the async consumer's
`preload().await`. The worker is a plain OS thread and must never run a
cross-thread task `wake()`: it does a lock-free `signal_epoch(epoch)` (`Release`
stores of `ready_epoch` then `ready`) and the awaiter polls with `Acquire`,
re-arming its own runtime timer (`POLL_INTERVAL` = 2 ms) while closed.
The play-owned `DecoderNode` opens the gate at every preload terminal site —
preload-chunk threshold with an empty overflow slot, EOF, `Failed`, `on_cancel`
— from its cached runtime epoch, and `rearm()`s it in `sync_seek_epoch`
(`Audio::seek` rearms consumer-side) so a post-seek wait blocks until that epoch
refills.

**`block_on_underrun`.** The bool remains the independent empty-read policy;
`ConsumerWakeMode` controls only how a successful drain wakes the worker. With
`AudioConfig::block_on_underrun(true)` a `read()` on an empty ring PARKS the
caller until the worker produces and the effective wake mode is always
`ImmediateOffRt`, regardless of the explicitly configured mode. The consumer
must therefore live on a dedicated thread or `spawn_blocking`, never the audio
callback or a tokio runtime thread whose tasks feed the ring. On wasm32 reads
never block.

## Ownership map

`StreamAudioSource<T>` is a thin coordinator: it dispatches and is the sole
mutator of track state through `update_state`. Sub-owners never take
`&mut StreamAudioSource`; they take disjoint context borrows (`SeekApplyCtx`,
`DecodeCtx`, `RouteCtx`) and return decision values for the coordinator to apply.

- `SharedStream<T>` — byte-space ground truth (position, len, phase, byte map,
  anchors, init range). No other owner clones byte-range policy. Every RT
  byte-space call — `phase`/`phase_at`, `position`/`set_position`, `len`,
  `byte_map`, `probe_seek` — answers from the source's narrow `SourceProbe`
  handle, and the fixed-at-open handles (`abr_handle`,
  `format_change_segment_range`, `peer_wake`) from clones resolved once in
  `SharedStream::new`; none of them take the control mutex. Off-RT holders (a
  construction reader parked in `Stream::read`, a consumer query) hold that
  mutex across waits, and a contended acquire on the forbid-blocking produce
  core is an RTSan violation (`sched_yield` in `parking_lot`'s contended
  path). The probe answers from the source's self-synchronizing state and
  must not take locks a reader wait can hold. Three calls still lock on RT
  frames — `probe_read` (steady-state `Read`), `media_info`, and
  `seek_time_anchor` — which is safe only because the sole off-RT holder
  that parks under the mutex is the replacement rebuild reader, and it
  exists exclusively while the FSM sits in `RebuildingDecoder`, a phase
  whose tick touches none of those calls. The same window is the only time
  the off-RT reader moves the byte cursor, which keeps `probe_seek`'s
  non-atomic load→resolve→store single-writer.
- `ActiveDecode` — the authoritative active `DecoderGeneration`, the optional
  `IncomingDecode`, the always-on `GaplessBlender`, decoded-output accounting, and
  the source observer.
  Each `DecoderGeneration` owns its decoder facts, base offset, install epoch,
  per-generation `GaplessStage`, and staged chunks.
- `ReadinessGate` — the only owner of byte-range readiness calculations; gate and
  wait paths must resolve the same range for the same phase.
- `SeekEngine` — `resume_target`, and the only writer of the producer decode
  epoch (`commit_decode_epoch`). `ResumeCursor` — separate raw-decode and
  rendered-source heads plus host/decoder sample rates; the raw head owns ABR
  cuts, while the rendered head resolves recreate positions after final output
  admission. The same owner detects route changes.
- `RebuildPort<T>` — the two-phase rebuild boundary: `prepare` produces a pending
  job, `submit` (from `finish_deferred`) spawns it off-RT. The job constructs a
  complete `DecoderGeneration`; installation only moves it.
- `Retired` — off-RT drain for everything the produce core displaces but must not
  free: generations a rebuild replaced, and the chunks a seek flushed out of
  staging and the gapless buffers. On overflow the queue `mem::forget`s rather
  than freeing on-core, and warns on drain.
- Format and anchor decisions are pure functions in `decode::format` and
  `seek::anchor`.

`RecreateCause::RouteChange` enters the same recreate state machine as
`FormatBoundary` and `VariantSwitch`; it is not a separate lightweight path.

## Track FSM

Of the `CurrentFsm` phases only `Failed` is terminal (`is_terminal`); `AtEof`
stays alive so a later seek can re-arm the track. `track::dispatch` runs three
stages per step: seek preemption (`preempt_target`), skipped while
`RebuildingDecoder`, which records it into `RebuildState::superseded_seek`; route
change (`start_route_change_recreate_if_needed`), skipped in
recreate/rebuild/terminal phases; then the phase's own `step`.

`AtEof` has exactly two owners, both semantic (PCM/timeline), never byte-space:
the decode path's exhausted finalization (`decode/step.rs`) and a seek landing
at-or-past `duration` (`SeekTransition::AtEof`). Byte-space `SourcePhase::Eof`
is a readiness answer, not an end of track — the demuxer may still hold
buffered frames past the last byte (a seek into the final segment parks the
reader at the stream total with the tail undecoded) — so wait states resume
into their `WaitContext` on it, exactly like `Ready`.

`update_state` publishes the phase to the shared `Activity` PLAYING flag: every
non-terminal phase keeps it set (the downloader peer's `priority()` reads it, and
buffering / mid-seek / rebuild windows are still "listened to"); `AtEof` and
`Failed` clear it, and entering `Failed` enqueues `AudioEvent::TrackFailed`.
`TrackStep::Done` is reserved for real termination: `TrackStep::Failed` maps to
`TickResult::Done`, EOF does not.

## Readiness gating

What *kind* of bytes gate a rebuild is not guessed: `recreate_ready_range` asks
`DecoderFactory::reader_profile(media_info, byte_map)` for the demuxer's
`ReaderProfile` (kithara-decode/kithara-stream contract) and resolves the named
input **shape** into a virtual byte range, which only the stream can do (it owns
the ABR byte shift). The `DecoderFactory` first overlays the caller-configured
`MediaInfo` codec and container, so a user declaration selects the profile, not
the playlist guess. `ReaderInput::Incremental` gates on the read-ahead window
directly; `ReaderInput::InitOnly` gates on the init header in virtual byte space
(`format_change_segment_range`), falling back to
`[offset, offset + profile.read_ahead_bytes())` when the init is unaddressable or
larger than that window. The landing media segment is read by the rebuilt
demuxer's first `next_frame`, not gated up front.

Steady-state gating has the same gate-vs-read contract, or the worker hot-spins.
`DEFAULT_READ_AHEAD_BYTES` is 32 KiB.

- `source_is_ready` (the `Decoding` entry gate) clamps to `chunk_lookahead_range`:
  the read-ahead window truncated at the **next** segment boundary and at source
  length. On a boundary the range may be empty, deliberately: the demuxer then
  drains input buffered from the previous segment.
- The container parser reads *across* that boundary, so `DecodeStep::NotReady`
  parks in `WaitContext::Playback`, whose phase (`source_phase_for_wait_context`)
  uses the **unclamped** `source_phase_forward` window — the range the decoder
  actually reads through.
- A seek landing gates on `seek_landing_end`: the containing segment's end, or
  the standard look-ahead for flat sources, always clamped to source length so
  the gate never waits past EOF. `post_seek_anchor_offset` gates `AwaitingResume`
  on the anchor byte only while the active ABR variant still matches the anchor's
  variant index.

**Source-readiness parks must re-aim the producer**, or a peer aimed elsewhere
after a seek/switch never fetches the bytes and the park never ends.
`ReadinessGate::source_park` is the single helper turning a not-ready
`SourcePhase` into a parked `TrackStep::Blocked` with the peer wake armed, so
every gate re-aims by construction — including a rebuild, which parks before the
decoder exists and cannot trigger the wake by reading.

## Decoder rebuild

Recreation is two-phase and never builds a decoder on the produce core.
`RecreatingDecoder` gates on `source_ready_for_recreate`, then
`RebuildPort::prepare` `probe_seek`s to the recreate offset and stores a pending
job — only one may be pending at a time. `finish_deferred` → `RebuildPort::submit`
spawns it (`spawn_blocking_on`); the job builds the decoder, optionally seeks it
to its landing time, pushes a `DecoderBuildComplete` onto the replacement or
incoming completion queue (capacity 4 each), then wakes the worker. Shell-side
`prepare_deferred` drains the completion queues, retires stale completions, and caches the
replacement matching the current `BuildId`; `RebuildingDecoder` only takes that
cached replacement. A matching replacement first aborts any exact incoming
transition: its landing and prepared blend belong to the generation being
replaced, and a late incoming completion is retired as stale. Installation is a
`replace_active` plus a retire. A caught
factory panic becomes
`RecreateOutcome::SoftFailed`, failing the track with
`TrackFailure::RecreateFailed`; `NeedsSourceWait` parks (`classify` maps only
`ErrorClass::Interrupted` here).

### Recreate policy

- The decoder is **not** recreated on every seek. Only on a real format boundary
  (codec change, or a variant change in an init-bearing container other than WAV
  — `variant_boundary` / `needs_init`), on a host-rate route change, and on
  non-interrupted seek failures. A known same-codec HLS switch in a self-framing
  container is **not** a format change (the source retargets byte mapping at the
  segment boundary), so a variant-index-only change must never become a recreate.
- Init-bearing containers (fMP4/MP4/WAV/MKV/CAF) must recreate at the source's
  init range, never mid-segment (no ftyp/RIFF/EBML header there);
  mid-stream-decodable containers recreate at the offset directly.
  `recreate_offset` encodes exactly this.
- **Seek-epoch suppression**: `detect` returns `None` while a seek is pending and
  the active generation was installed at that same seek epoch.
- **Supersession retains seek ownership**: when a variant change or newer seek
  epoch makes an in-flight rebuild stale (`policy::superseded`), the recorded
  `superseded_seek`, else an observed newer seek, else the request carried by
  `RecreateNext::Seek`/`ApplySeek` returns to `SeekRequested`. Only a decode-only
  rebuild may continue into a fresh `FormatBoundary` recreate; dropping the
  carried request permanently starves the producer.
- **Mid-playback recreate resumes at the rendered source head, not raw decode
  progress or `committed`.** A `FormatBoundary` + `RecreateNext::Decode`
  rebuild bumps no seek epoch and flushes no outlet ring, so resuming at the
  lagging `committed_position` would replay already-queued chunks, while raw
  decode progress may skip source retained by Warp or a buffering effect.
  `Fetch::source_end` carries the decoded-source endpoint represented by
  rendered PCM; the play worker commits it only after final producer-port
  admission, and `ResumeCursor::resume_position` reads that epoch-scoped frame
  plus sample rate. `resume_target` wins only while
  `target > rendered_source_head`. Raw decode progress remains the ABR
  splice/promotion coordinate.
- A **route change** keeps its container and resolves its origin through
  `anchor::recreate_offset` seeded with the running generation's `base_offset` —
  never a seek anchor, which would root an init-bearing demuxer on a media byte.
  Equal-rate notifications recreate nothing.

## Seek

Two epoch atomics: the **seek-state** epoch, bumped by the consumer the instant
it requests a seek (`Audio::seek` → `SeekControl::begin`); and the **producer
decode epoch** (`SeekEngine::epoch`), advanced only when the worker applies a
seek, so it lags across the requested-but-not-applied window. Decoded chunks
*and* terminal markers (EOF / failure) are tagged with the decode epoch via
`AudioSource::decode_epoch()`, never the live seek-state epoch: a genuine
EOF reached after a newer seek bumped the seek-state epoch would otherwise pass
the consumer's `EpochValidator` as the new seek's terminal.

Consumer side splits in two, because the begin half takes locks and some
consumers sit on an audio device callback. **Begin** — `SeekBegin::begin`,
implemented by `SeekHandle` (`Audio::seek_handle`) — bumps the epoch, marks
pending, publishes `SeekLifecycle`, notifies the peer, rearms the preload gate
and wakes the worker. **Adopt** — `Audio::sync_seek` — runs
`RingConsumer::begin_seek_epoch` when that epoch differs from the ring's, draining
stale fetches inside the RT no-free boundary (stale chunks to the trash ring; the
first fetch at the new epoch is staged, not dropped). Adopting is lock-free, and
every read entry point (`read`, `read_planar`, `next_chunk`, `preload`) does it
first, so a new epoch reaches the reader without its caller touching the reader.

`Audio::seek` remains the one-call form for consumers off the audio thread.
A `SourceSeekAnchor` byte offset
is valid only in the variant byte space that resolved it, so `ApplyingSeek` (and
a wait carrying it) re-resolves the seek when the active ABR variant no longer
matches `anchor.variant_index`.

**Seek error recovery** (`SeekRecovery::resolve`) splits by `DecodeError`
variant, never by string match. `SeekOutOfRange` (past EOF, or outside the known
duration) → `Reject`, no recreate and no retry: a fresh decoder would reject the
same target forever. `Interrupted` → park in `WaitContext::ApplySeek` with the
peer re-aimed. Anything else → recreate. Missing `MediaInfo`, or an init-bearing
container with no available init range, fails the seek.

**Head trim.** A generation may carry a `pending_head_skip` `ResumeState`;
`seek::skip::apply` drops the leading frames between the chunk timestamp and the
seek target once, on the epoch that requested it, and clears the flag. A chunk
fully before the target is dropped whole.

## Variant transitions (gapless splice)

A variant switch can also be spliced gap-free by running a second generation to
overlap. `ActiveDecode::incoming` is an `IncomingDecode` FSM: `Preparing` →
`Building { build }` → `Priming` (or `Failed`).
`StreamAudioSource::progress_variant_transition` drives it from `prepare_deferred`
and only from `Decoding`, or `WaitingForSource` in `WaitContext::Playback` (the
starved reader an urgent down-switch exists to rescue). Seek and recreate phases
are excluded — they are about to replace the decoder a promotion would install.
A live transition holds the outgoing's EOF instead of dying to it (below); the
terminal phases abort only a transition that outlives them: `Failed`, and an
intent that first surfaces after `AtEof` has already latched.

- **Landing.** `landing_for` places the incoming at the outgoing's
  `OutgoingFrontier::Exact` frame, translated through each generation's timeline
  origin — never the audible playhead (behind the frontier the gap never closes)
  and never past it (the outgoing stops decoding on a full ring, so a stalled
  consumer would wedge the switch). The source derives this landing frontier
  only from `ResumeCursor`; neither `WaitingForSource` nor the outgoing
  disposition replaces a known exact landing. `Awaiting` carries no frame, so
  the source keeps its own seek-derived target.
- **Priming** is bounded to 8 decode steps per pass and only extends the staged
  span. The reader plan's exact or unavailable promotion frontier is latched in
  `Preparing` and carried through `Building` into the incoming generation;
  decoder build latency and later `ResumeCursor` movement cannot move that cut.
  Once latched, outgoing publication stops at the cut. A same-`AudioSpec` active
  generation may decode only until its bounded holdback covers the real 20 ms
  outgoing tail, while a cross-spec transition stops immediately. This lets the
  incoming catch one fixed cut instead of chasing an equal-rate outgoing stream.
  `IncomingPrime::Advanced` wakes the rebuild runtime; `Ready` means a proof
  exists, while EOF before the frontier is `Failed`.
  When the *outgoing* runs out of source with a live incoming, the generation is
  marked source-exhausted instead of finished: holdback and the staged tail stay
  promotable, `DecoderEvent::TransitionHold` announces the held state once per
  transition, and EOF finalization waits until the transition promotes or fails
  (the held pass reports `Waiting`, so the hang watchdog bounds it). Even with
  no slot, finalization defers by one tick so an intent that raced the last
  chunk still plants its incoming before `AtEof` can latch. An exhausted
  outgoing can never satisfy a wait-for-outgoing readiness answer, so those
  degrade to a hard cut at the final frontier.
  A finished active generation still uses the unheld EOF drain. Gap, mixed-spec,
  malformed, or over-capacity PCM fails the decode and stays owned for shell
  retirement. `ResumeCursor` records each
  post-skip, post-gapless chunk immediately before blending and before the
  decoded source crosses into the play-owned Warp/effect lane, so a buffering
  or frame-changing playback effect cannot move the raw cut. A generation marks
  EOF once, disables holdback, and drains staged then gapless PCM without
  reflushing over pending tail data.
- **Promotion proof** (`promotion_span`) is fail-closed and minted before
  `VariantControl::promote_variant`. A same-spec exact transition requires
  continuous active PCM from the rate-converted outgoing cut through the whole
  join and continuous incoming PCM from its corresponding cut through the same
  end. An installed transition with `OutgoingDisposition::Abandoned` maps only
  its priming and promotion proof to `OutgoingFrontier::Unavailable`, an explicit
  hard cut; `WaitingForSource` alone does not. A retained transition still needs
  real outgoing join PCM — except a source-exhausted outgoing, whose join PCM
  cannot exist past the final decode head and degrades the proof to a hard cut
  there. `Deferred` preserves the same latched cut. `Awaiting`,
  a previous active join, a discontinuous
  span, or a landed-late incoming mints no proof.
- **Promotion** takes the incoming generation into a non-copy
  `PreparedPromotion`, trims it to the proven cut, and copies exactly the proven
  active sample range before `VariantControl::promote_variant`. `Deferred`
  restores the already-trimmed generation to `Priming`; `Stale` returns it for
  shell retirement; `Promoted` performs only the infallible blender/generation
  state swap. Every displaced or aborted generation goes to `Retired` - never
  dropped on the produce core. Seek/reset cancels an active join; generation seek
  notification retires every staged chunk.

**`GaplessBlender`** is always on and owns the audible seam. It owns active and
prepared reusable buffers; profile growth and resizing happen in the shell before
`Priming`, while checked replacement and `process_active` only move or reuse
state. For exactly 20 ms it combines real outgoing sample `i` with incoming
sample `i` using gains `1 - i / frames` and `i / frames`, then returns to
`Steady`; identical samples remain bit-exact. Different specs hard-replace state.
Ramp counters are `u16`, so the per-frame gain is an exact `f32::from`.

## Construction reads

`Audio::prepare` builds the initial decoder **exactly once**
(`create_initial_decoder`, one `spawn_blocking`), with no retry loop and no
readiness gate. The construction read goes through the **blocking** off-RT
`Stream::read` adapter: every `OpenedReader` carries its own `ConstructionGate`,
shared only with that reader's `SharedStream` clone. The initial builder and
`RebuildPort` arm their reader-local gate around each off-RT factory call and
disarm it after a normal return, join error, or caught panic. A rebuild therefore
cannot switch an active decoder reader into blocking mode. Steady-state reads
use non-blocking `Stream::probe_read`; on-core seeks use `probe_seek` (position
math only, no `prime_seek_range` spin on the forbid path). The gate selects the
read mode and nothing else: `SharedStream`'s `Seek` is the blocking adapter in
both phases, because a decoder seeks past residual lateness in steady state as
well, and a probe seek there only reports not-ready to a caller that can do
nothing but ask again. Staying off the blocking path is `OffsetReader`'s own
choice, made by naming `probe_seek` — not a consequence of a disarmed gate.
Blocking makes a
slow-but-arriving prefix wait, off the RT worker, up to the stream's blocking-read
budget rather than error on the first not-ready probe. A construction-range byte
that never arrives surfaces the **stream layer's** typed terminal verbatim; the
audio layer mints no construction error type and there is no synthetic
`TimedOut`.

A `VariantChange`/`SeekPending` at construction is **not** a rebuild trigger: the
variant is settled before the build, construction always probes at offset 0, and
a concurrent play-then-seek is applied by the post-construction seek path — a
`VariantChange` surfacing here is a stream-layer state bug. Pinned by
`tests/tests/kithara_hls/probe_not_ready_at_creation.rs`.

## Prepared producer seam

`Audio::prepare` returns `PreparedAudio<Audio<Stream<T>>, StreamAudioSource<T>>`:
the reader plus a still-concrete decoded source and `PreparedAudioLane`. The lane
carries the source, ring ports, event publisher, playhead, preload gate, and
service-class capability required by its consumer. It contains no
`PlayWorker`, `DecoderNode`, playback effect, stretch processor, or engine-load
meter.

Decoded output remains in decoder/song coordinates
(`AudioChunkInfo.timestamp` / `end_timestamp` / `frame_offset`). A source
discontinuity publishes its revision and `AudioSpec` so the downstream owner can
reset its own state. `kithara-play` consumes this seam, owns terminal effect
drain and final output admission, and is the only layer that transforms frame
count for playback.

## Sample-rate conversion

Sample-rate conversion for playback is decoder-owned. `AudioConfig.decoder`
carries `AudioDecoderConfig<B>`, whose optional `DecoderResamplerSettings<B>`
holds the concrete `B: kithara_resampler::ResamplerBackend`, its
`ResamplerOptions`, and its `ResamplerQuality` (`High` by default), combined with
`AudioConfig.host_sample_rate` into `DecoderConfig.resampler`. A requested host
rate always resolves to a plan: absent settings fall back to `B::default()`, so
asking for a rate is never silently dropped — on the Apple fused placement the
codec converts with no standalone backend at all, and a backend that cannot
serve the ratio fails loudly at decoder construction. Without a host rate there
is no plan and the decoder emits source-rate PCM; route changes are then decided
by `ResumeCursor`'s rate guards, which recreate nothing at an unknown or equal
rate. Backend implementations belong to
`kithara-resampler`; this crate never picks a portable default.
`resample-rubato` / `resample-glide` enable backend types; `apple-fused-src`
forwards to `kithara-decode/apple-codec-embedded-resampler`. Selecting a backend
is a typed config decision, never a runtime fallback chain. Output capacity is a
correctness invariant, not a knob: the backend reports `output_frames_for_input`
in the ceil frame domain and the decoder adapter sizes buffers from that.

## Track analysis

`analysis/` owns the reusable per-track analysis engine, generic over
`B: ResamplerBackend`. `AnalyzerBuilder<B>` is the public, `Default`-constructed
selector (`with_waveform`, `with_beat` — which requires `B: Default` —
`with_beat_config`, `with_sample_pool`), and `is_empty()` lets callers skip
scheduling a pass entirely. `TrackAnalyzers` is the crate-private per-track set;
each analyzer is fed every decoded chunk once.
`TrackAnalysis` is the public snapshot: caller token, revision, rate axis,
extent, coverage, per-artifact fingerprint, waveform, and beat. It is
self-contained by contract - a consumer holding only a snapshot can render the
waveform, place markers on the source timeline, and tell how much of the track
it is based on. `source_frames()` is the denominator that turns a `BeatGrid`
frame into a fraction: the extent when known, covered frames otherwise, so a
repeated or overlapping range counts once.

A pass publishes many times. `TrackAnalyzers::snapshot` leaves the pass able to
accept further ranges and bumps a strictly increasing revision, so a consumer
discards anything that does not outrank what it holds. `AnalysisTask` publishes
every `PUBLISH_SECONDS` of newly covered source and once more at end of stream,
keyed to decoded frames rather than wall-clock time, so a run produces the same
revision sequence every time. Only that last publication pins the extent, to the
covered frontier. `GridState` is `Final` only once the whole known extent is one
covered run.

Identity is an opaque `AnalysisToken` the caller opens the pass with; this crate
echoes it and never interprets it. `AnalysisFingerprint` carries the beat tag and
the waveform tag separately, so a waveform resolution change cannot invalidate
stored beat results.

The rate axis is named when the pass opens, not discovered from the first chunk:
`AnalysisWorker::analyze` takes it, the caller opens its reader onto the same
one, and a range on another axis is refused rather than redefining what a frame
number means. Analyzers are still built lazily by whichever range arrives first,
so a pass that covers nothing allocates nothing.

`Coverage` is the canonical record of which source ranges a pass has observed,
kept as sorted, disjoint, non-adjacent runs; `TrackAnalyzers` owns it and every
consumer reads it, there is no second copy. A chunk's range comes from
`AudioChunkInfo::frame_offset` and `frames`, so position never depends on arrival
order. `TrackAnalysis::missing` is derived from that same record rather than
kept beside it; its horizon is the extent once known and the covered frontier
until then, the same rule `source_frames` uses.

### Decode scheduling

`AnalysisTask` does not read its reader in order. It picks where to decode next
by one rule: the middle of the largest uncovered range. That degenerates to
binary subdivision on an uncovered track, so an early snapshot describes the
whole track rather than its opening, and to refilling holes on one playback has
mostly covered; a range another producer covered is never decoded again.

- **Two extents.** The pass publishes the covered frontier at end of stream, or
  the length the schedule planned against when that is longer, so a range given
  up on past the last covered frame is still reported missing. The schedule
  works from the reader's stated length - available before anything is decoded -
  bounded by what that reader proved: end of stream, or a seek answered
  `PastEof`. That figure is never written into the pass, because
  `TrackAnalyzers::ingest` refuses a range reaching past the extent it holds and
  an under-reported duration would refuse the source's own tail.
- **Run bounds.** A run decodes at least one beat detector window, read off the
  beat configuration, before another position is chosen; a shorter run would
  leave every position contributing a partial window and the markers would move.
  It ends at covered audio or at the end of the extent. Where it starts comes
  from its first decoded chunk, not from `landed_at`: a seek is begun rather
  than completed when it answers, so the decoder resumes at its own boundary.
- **End of pass.** No position is left when the extent is covered, or when every
  position still uncovered has proved unreachable. A run waiting on its reader
  is never asked to reschedule, so the covered extent is checked before it too -
  that is how a pass whose last uncovered range a producer filled ends without
  its reader reaching end of stream.
- **Retirement.** A position whose run decoded nothing new is never chosen
  again, which keeps a source with coarse seeking from being asked for it
  forever. The test is what that run decoded, not what the pass covered while it
  ran, since a producer folding ranges from anywhere would keep an unreachable
  position eligible for as long as playback feeds the pass. The cost is stated
  rather than hidden: retiring the middle of a gap drops the whole gap from the
  schedule, so a source that snaps out of every gap leaves a hole, reported as
  missing like any other, and refilling a hole costs a seek per halving.
- **Decode error.** It ends the pass without discarding it: the ranges already
  delivered are published and the rest reported missing.

A source that reports no duration has no middle to seek to, so the task decodes
it in order and never repositions the reader. This is a degraded mode rather
than a fallback over a missing answer: "where is the middle of this track"
genuinely has no answer for a live stream.

## Producer ingest

`analysis/producer/` is the seam a component that must not be slowed down uses
to contribute ranges it has already decoded, so a track being played is not
decoded a second time. `AnalysisProducer` names its pass once, when `analyze`
hands it back, so offering costs no lookup and two producers never contend; a
track with no open pass has no handle at all.

`offer` downmixes to mono by the channel mean - the same reduction both
analyzers apply - and copies into a bounded transport allocated when the pass
opens: a sample ring plus a ring of `(start, frames)` descriptors. It never
blocks, never allocates and never retains the caller's buffer, so a caller under
a forbid-blocking policy may recycle its buffer as soon as the call returns. A
range that does not fit is refused whole and reported untaken; it stays
uncovered, so it stays missing and eligible to be produced again. `Outlet` is
deliberately not the transport here: it parks a failed push in a one-slot
overflow and still reports `Ok`, so it cannot report the first refusal.

That the offer neither blocks nor allocates is asserted, not just stated: the
`rtsan` lane calls it inside a forbid-blocking region, where RTSan aborts on a
malloc, free, lock, or syscall. The taken path is the heaviest one - the refused
and closed paths return before writing - so proving it covers them. A pass that
ends drops the reading half; the next offer reports the pass closed and the
caller lets its handle go.

`AnalysisProducer` implements the neutral `AudioObserver` contract. Playback
can attach it to a queued track without `kithara-audio`, `kithara-play`, or
`kithara-queue` knowing which analyzer consumes the decoded ranges. Rejection is
best-effort and never changes playback; a refused range remains uncovered and
eligible for the analysis reader to decode later.

The analysis worker drains the transport on its own tick, where the DSP is
allowed to happen, and folds **one block per descriptor**. Contiguous
descriptors are never joined, for correctness rather than as a missed
optimisation: `Runs::merge` finishes the frontier `MonoStream` and `Runs::open`
starts a fresh one at every push boundary, so the beat resampler's segmentation
is a pure function of the block boundaries. Per descriptor those are the
producer's own chunk boundaries; joined, they would depend on how many
descriptors happened to be waiting for the tick.

`BeatAnalysisConfig<B>` carries the implementation-affecting beat tunables and a
standalone resampler backend handle. Defaults: 1024-frame mono resampler blocks,
22 050 Hz detector input, 30-second detector windows with 2 seconds of overlap,
`ResamplerQuality::High`. The analyzer never stores whole-track source PCM: it
downmixes to mono and keeps each covered span at the detector rate, a quarter of
the source cost and outside the playback pool.

The contiguous run, not the pass, owns the resampler. `MonoStream` is
sequential, so a range decoded later cannot be pushed through the stream that
produced an earlier one: a run keeps its stream only while it is the frontier,
and the moment another segment is appended behind it the stream is flushed into
the run's mono and dropped. Every join is pinned to the detector frame its
source position implies, so per-segment rounding cannot accumulate into marker
drift. A detector window is a fixed span of the absolute detector-rate timeline,
keyed by index: detected once, wherever in the arrival order its span completes,
with its markers at the source position they were found at. Markers therefore
agree across arrival orders within the resampler's splice tolerance rather than
byte for byte - byte equality would mean re-detecting everything after every
filled gap.

A run that reaches `detector_min_window_seconds` is detected as it stands rather
than waiting for a full window plus overlap, which is what makes a track usable
from its first covered piece; that estimate is re-detected once the window
fills. Once the extent is known the grid is spread across it at its own tempo,
keeping every detected marker where it was found, so a later revision replaces
filled positions with detected ones as coverage arrives.

Run mono comes from the same injected `SamplePool` as everything else in the pass,
and the runs additionally share a mono budget of four detector windows.
Detection consumes the front of a run and never its back, so the budget is spent
from the earliest run forward; a reclaimed span stays covered for every other
consumer but beat cannot analyse it again, and reclaimed ranges are recorded for
the snapshot to report. Marker equality across arrival orders therefore holds
below the budget: past it, a span reordered far behind the frontier can be
reclaimed before its window completes. Grid-cleanup scratch comes from that same
pool, injected through `AnalyzerBuilder::with_sample_pool`; cleanup does not
construct a second one.

`AnalysisWorker<B>` is the public handle over the private `AnalysisRunner` and
its single long-lived `AnalysisNode<B>` on the existing `kithara-analysis`
thread (absent on wasm32). It neither constructs nor shares the playback
scheduler. Jobs carry caller-owned cancel tokens; `child_token()`
hands out children of the worker's own job scope, so there is one cancel
hierarchy, and the caller keeps at most one job in flight, cancelling the
previous token to preempt. Results arrive on a `watch` channel: waveform first,
then waveform+beat when a beat pass is configured; on failure or cancel the
sender drops without a value. The node owns the job receiver, the task FSM, and
the single `Box<dyn BeatDetector>` taken at construction — detector ownership is
never shared or locked. `Decode` consumes at most one chunk per tick. The
runner park is flash-visible and `analyze` wakes it after enqueueing; no
sleep, backoff loop, or poll watcher. `AnalysisObserver` keeps the normal
no-progress watchdog and separately classifies returned heavy ticks against a
120-second budget; a detector call is indivisible, so an over-budget call can
only be reported after it returns.

**Feature seams.** There is no single `analysis` feature. Artifact types are
unconditional because analysis and cache keys use them even when a pass is
absent. `analysis-waveform` gates the `realfft` analyzer (and
`with_waveform`), `analysis-beat` gates the beat analyzer path, `beat-nn` is a
detector backend on top of `analysis-beat`. Without `analysis-beat`,
`with_beat()` is a compile-time no-op — `is_empty()` is the runtime signal.

## Waveform

Pure synchronous DSP turning decoded PCM into a `Waveform` for display. No async,
I/O, cancel, or colour types here: band → colour mapping and orchestration belong
to consumer crates. Tunables live in `AnalysisParams`.

- **Source-only invariant**: analysis runs on the decoded SOURCE signal, never
  post-EQ / post-timestretch / post-resample output. Playback-rate and mixer
  transforms remap only the time axis and never re-run analysis.
- **Playback observation is separate**: the optional `AudioObserver` sees the
  playback decoder output before playback effects. Its `AudioChunk::meta.spec` is
  authoritative and may already reflect decoder-side sample-rate conversion;
  source-rate offline analysis does not consume this feed.
- **`Bucket { low, mid, high }`** are three independent band heights per bucket,
  each normalized to `[0, 1]` on one shared scale — not a single bar plus a
  colour. The deck paints them as concentric mirrored bars, so the tallest is the
  outer hull. All-zero is silence, renders as nothing, never `NaN`.
- **Position-addressed windows**: `WaveformAnalyzer::push` takes the source frame
  the block starts at, scatters the block into every window it touches, and
  reduces a window once every one of its own frames has been written. Blocks may
  arrive in any order, twice, or overlapping. A window's completeness is its own
  written set, never the pass's `Coverage`: the two diverge exactly when a
  window is evicted, and reducing on coverage would then publish a half-silent
  window instead of leaving the span unanalysed. Windows still waiting are
  capped, and the oldest is evicted: its span then reads as uncovered, which is
  what a gap already looks like.
- **Normalized-position index**: buckets are indexed by normalized track position
  `[0, 1]`, never wall-clock seconds. `bucketize` is the single home of that
  mapping — bucket `b` folds the raw range `[b*R/N, (b+1)*R/N)` and always returns
  exactly `N` values, filling an empty range with the supplied `empty`. An
  uncovered span therefore renders as silence for now; the sparse series keeps
  which windows exist, so per-bucket coverage can surface later.
  `WaveformAnalyzer::snapshot` spreads the buckets over the window count the
  source extent implies, so bucket boundaries stay put as coverage grows, and
  falls back to the highest reduced window when the extent is unknown. It clamps
  the request to that count, so a short track yields fewer buckets rather than
  fabricated ones, and it does not consume the pass: a pass publishes many times.
- **PCM ↔ frequency boundary**: `WaveformAnalyzer::new` takes the track
  `sample_rate` because band crossovers map to FFT bins via
  `bin_hz = sample_rate / fft_size`. A constant sample rate per track is assumed;
  build the analyzer once the first chunk's `AudioSpec` is known.
- **Reduction**: per Hann window, band energy is summed into low/mid/high (DC bin
  zeroed; windows below `energy_floor` RMS contribute nothing) and each band is
  divided by its bin count — an energy DENSITY, without which the wide mid/high
  bands outweigh the narrow low band by bin count alone. Windows hop by
  `fft_size / 4` (75% overlap), so every source frame feeds four windows; only
  sources shorter than one window fall back to a single zero-padded window, and
  only once the extent is known, so no snapshot publishes a padded frontier
  window. `snapshot` keeps each bucket's loudest window
  (component-wise max), takes `sqrt` to magnitude, applies the per-band
  perceptual `band_gain`, then divides all three by one shared global max —
  shared, not per-band, so the loudness tilt survives.

## Blob codec

Analysis artifacts persisted to the on-disk cache (`Waveform`, `BeatGrid`) share
one versioned little-endian encoding via the crate-internal, domain-agnostic
`blob` module: the `Blob` trait owns the frame (a `u32` `Blob::VERSION` header
then the body), each artifact implements only its body, and decoding requires the
cursor to consume the blob exactly — trailing bytes are corruption.

Each artifact owns its `VERSION`. A mismatch is `BlobError::Version`; a
truncated, mis-sized, or out-of-range body is `BlobError::Corrupt`. Both are cache
misses — the caller re-analyses and overwrites; there is no in-place migration.
Speculative allocation from an untrusted length prefix is capped at
`MAX_PREALLOC`. `BlobError` is the only piece crossing the crate boundary (the
public `TryFrom<&[u8]>` error); `Blob`, `Reader`, `Writer` stay internal. The
composite track-analysis blob (version + config fingerprint + per-artifact
sections) is an app-layer concern owned by `kithara-app`.

## Agent guardrails

- `kithara-audio` owns decoder lifecycle, seek/session state, source
  discontinuity publication, and stale decoded-chunk invalidation. It consumes
  source contracts and must not reconstruct HLS or file policy from
  protocol-specific heuristics.
- Playback cancellation and scheduling policy belong to `kithara-play`; audio
  sources observe only their scoped cancellation and wake contracts. Each
  queued analysis job owns its own scoped token, while the private analysis
  runner owns cancellation of its single long-lived node.
- Prefer explicit FSM or session objects for multi-step control flow; do not
  scatter new `pending_*` or shadow flags across source and consumer layers.
  Playback effects and their reset/drain policy belong to `kithara-play`, not
  to this source pipeline.
