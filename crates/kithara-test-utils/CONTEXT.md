# kithara-test-utils — Context

Contracts and invariants for the kithara-test-utils crate; the README is the overview.

## WASM test serialization

`#[kithara::test(serial)]` expansions acquire the hidden test-binary-wide async
mutex before entering the generated body. The guard is deliberately outside the
per-test timeout: serialization controls overlap, while each timeout measures
only its own test body.

## Probe capture

While at least one `Recorder` is alive, the capture layer records every `tracing::event!` whose target ends with `_probe` (all `#[kithara::probe]` expansions emit to `<crate_name>_probe`, e.g. `kithara_stream_probe`) into a process-wide `Vec`, so a test can snapshot the full sequence and assert on it.

- **Why a tracing layer, not `EventBus`**: `kithara_events::EventBus` is a `tokio::sync::broadcast` — under load, lagged subscribers drop events. Probes fire at the decision site and the tracing layer records every emission without a bounded channel.
- **Why a process-wide subscriber**: `tracing::subscriber::set_default` is thread-local, but probes fire on tokio worker threads (e.g. those spawned by `Downloader::run`) that do not inherit a per-test default. Because `#[kithara::test]` initialises a global subscriber via `setup_tracing_with_filter`, the probe layer must be composed inside that init path — `test::init_tracing` attaches it alongside the fmt layer. A separate `set_global_default` would fail with `SetGlobalDefault`.
- **Activation**: probe sites compile to no-ops unless the crate's `probe` feature is enabled in the test build. The `capture` module itself is gated on `cfg(any(test, feature = "probe"))` and is absent on `wasm32`.
- Under `--cfg rtsan` `init_tracing` installs **no** subscriber at all: a capturing/formatting subscriber allocates on the forbid-blocking audio worker, so that lane deliberately has no probe capture.

### Cross-test isolation

Isolation is by **install id**, not by serializing tests:

- `#[kithara::test]` calls `bump_install_id()` once and enters the `OWNED_INSTALL_ID` task-local scope before the test body runs.
- Every probe firing stamps `current_install_id()` into its event; `Recorder::snapshot` keeps only events whose `install_id` matches the recorder's and whose timestamp is `>= start_at`.
- The task-local is what makes this correct: `tokio::spawn` inherits it, so orphan tasks from a just-finished test (downloader on-complete, audio worker draining its last buffer) freeze the *previous* id and drop out of the next test's snapshot. `spawn_blocking` and non-tokio threads do not inherit it and fall back to the global atomic.
- Probe capture is lease-bound. Each `install()` acquires a lease, cloned recorders share that lease, and independently installed recorders share the global log. Dropping the last lease clears the log and releases its backing allocation; dropping one overlapping recorder leaves the log intact for its live siblings. The lease is what bounds the log's lifetime: the layer is composed into every test binary's subscriber, HLS playback fires ~10k probes/second, and a retained `ProbeEvent` costs ~700 bytes — unleased capture added ~70 MB of RSS per playback session and never gave it back.

### Driving tests off probes

`Recorder::wait_for_probe` / `wait_for_probe_async` are the sanctioned way to advance a test: they block until a recorded event matches the predicate or the budget elapses, including events that arrived before the call. Tests should use probe arrival as their clock instead of polling `Audio::read` / `Stream::len()` on wall time, and should fail when the budget elapses rather than relaxing it. Use the async variant on a `current_thread` runtime — the blocking one starves the tasks the test is waiting on.

Decode packed probe arguments with `T::from_probe_arg(event.u64("field")?)` rather than hand-written decoders next to the `IntoProbeArg` impls.
