# kithara-file — Context

Contracts and invariants for kithara-file; the README is the overview.

## Architecture

`FileConfig::for_src(src)` → `File` (StreamType marker) → internal `FileCoord`, which splits by
source: `FileSrc::Local` reads through `AssetStore` with an absolute `ResourceKey`;
`FileSrc::Remote` runs an internal pull-driven `FilePeer` emitting `FetchCmd` batches to the
shared `dl::Downloader`, which writes (writer / `on_complete`) into `AssetStore` under the `File`
layout scope. `FileSource` (impl `kithara_stream::Source`) wraps `FileCoord`; `Stream<File>`
(`Read + Seek`) wraps `FileSource`. `FileSource` is synchronous: every async concern (HTTP fetch,
body streaming, finalization) belongs to the `Downloader` through `FilePeer`, and it holds the
`PeerHandle` from `Downloader::register` for its whole lifetime — dropping the last handle cancels
in-flight fetches.

## Reader contract

`Stream<File>::Read + Seek` goes through `FileSource::wait_range` / `read_at`.

- `wait_range(_, Some(_))` is the audio-worker probe: checks phase once, returns
  `SourceError::WaitBudgetExceeded` for missing in-range bytes instead of blocking.
  `wait_range(_, None)` is the off-RT adapter path, delegating to the storage wait until bytes,
  EOF, failure, or cancel resolves. A flushing seek short-circuits to `WaitOutcome::Interrupted`
  before any demand update.
- The probe clamps oversized read-ahead at a known length, so a fully cached file queried with
  `0..read_ahead` where `read_ahead > len` is `Ready`, not need-data.
- `Eof` requires **both** that the range starts at or past the known length **and** that the
  resource is `Committed`. An announced-but-uncommitted length yields `Waiting`; an in-range range
  not yet written returns `WaitBudgetExceeded` (→ `Pending` / need-data) so the reader holds
  rather than terminating. See `crates/kithara-stream/CONTEXT.md` "End-of-stream contract".
- Known length blends the announced total (`FileCoord::total_bytes`, seeded from
  `Content-Length`) with the committed `AssetReader::len()`, announced first. On `206 Partial Content` the total is `resume_from + Content-Length`.

## Sources

`FileSrc::Local(path)` requires an existing absolute path, opens it through `AssetStore` with an
absolute `ResourceKey`, skips all network activity, and yields a `FileSource` with no peer and no
downloader. Media bytes are read in place: never copied under the cache directory, and no layout
callback applies to that absolute key. Derived resources differ —
`kithara_play::ResourceConfig::asset_key` describes the same path as `AssetSource::Local`, selects
the `File` layout, and mints an ordinary relative key such as the default
`analysis/track.analysis`, so a custom `File` layout governs local-track analysis and other
derived artifacts while the original media file stays untouched. `FileSrc::Remote`:

- Opening returns as soon as the asset claim succeeds; `Content-Length` / `Content-Type` arrive
  later with the first response, so `len()` is `None` until then. `AcquisitionResult::Ready` means
  already committed (no download); `Pending` hands over the single non-`Clone` commit-owning
  `AssetWriter`.
- If a sibling `AssetStore` instance holds the atomic-chunked tmp for the same canonical path,
  `create` polls every 10 ms until that sibling commits or drops, or returns cancellation when
  its own work token fires. The loop is wrapped in
  `#[kithara::hang_watchdog]` and ticks the watchdog only while the tmp's length is *unchanged*,
  so a live sibling never panics and only a stale tmp from a crashed process does.
- Downloading is pull-driven and gap-driven: `Peer::poll_next` fetches from `next_gap(0, upper)`,
  the first missing byte from the start and never the seek position, so the landed prefix stays
  contiguous and `FileCoord::set_download_pos` is a true cached-prefix cursor. Backpressure runs
  through the read-demand cell shared between `FileCoord::read_pos_handle()` and the demand lease
  from `AssetStore::attach_demand`, bounded by `look_ahead_bytes`. Each chunk write wakes the
  audio worker through `WorkerWake`, so a parked probe re-runs on arrival rather than on the
  scheduler backstop.
- Only the **elected producer** issues GETs. A peer with no demand lease (standalone store, single
  consumer) always drives; with a lease, a non-producer tries `try_take_producer` to claim an
  abandoned slot, else yields to the next downloader tick. Two consumers of one URL share one GET.
- A transient error with bytes already received leaves the resource active, so the next
  `poll_next` issues a Range GET for the remaining gap. A fatal error, or a hard failure with no
  bytes at offset 0, fails and evicts the resource and publishes `FileEvent::Error`.

Cache identity: naming is owned by the layout registered for the `File` marker in the shared
`AssetStore`. The stream binds `AssetSource::Remote { url, discriminator }` through
`store.scope::<File>()` and mints one `AssetResource::Source`; its extension comes from the
explicit `config.extension` hint, then the final URL-path extension, and finally `bin`, accepting
only a short all-ASCII-alphanumeric candidate, lower-cased. The default layout stores the file at
`<asset_root>/track/track.<ext>`, root hash over the canonical URL without query or fragment,
folding in the explicit discriminator when present. Query parameters are never an implicit
identity for direct files. A higher layer may register any `AssetLayout` for `File`;
`kithara_play::policy::QueryIdentityLayout` is the built-in domain-aware option selecting stable
identity parameters while ignoring rotating signatures and expiry values. Layouts are registered
once through `AssetStore::builder().layouts`, and every `FileConfig` holds a cheap clone of that
same store handle.
