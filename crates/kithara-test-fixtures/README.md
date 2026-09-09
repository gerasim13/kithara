<div align="center">

<img src="https://raw.githubusercontent.com/zvuk/kithara/main/logo.svg" alt="kithara" width="300">

</div>

<div align="center">

[![License](https://img.shields.io/badge/license-MIT%2FApache--2.0-blue.svg)](https://github.com/zvuk/kithara/blob/main/LICENSE-MIT)

</div>

# kithara-test-fixtures

Audio test assets produced at build time and served from a persistent store
on disk. A test asks for bytes and gets them; nothing is synthesized or
encoded inside a test's wall-clock deadline.

Source edits, dependency updates and commits do not invalidate prepared assets.
The explicit `cache-version` file selects the shared cache revision. Change it
only when intentionally replacing the cached fixture set; use a new case name
for an individual replacement. Rebuilds reuse existing entries.

Set `KITHARA_FIXTURE_CACHE` to an absolute persistent directory before building.
There is no temporary-directory default. For all local worktrees, configure it
once in your user Cargo configuration (`~/.cargo/config.toml`):

```toml
[env]
KITHARA_FIXTURE_CACHE = "/absolute/persistent/path/kithara-fixtures"
```

The environment can override this value. CI supplies a persistent directory
shared across branches and platforms within each trust boundary. When changing
the root, copy the existing version directory to preserve prepared assets.

## Usage

```rust
use kithara_test_fixtures::fixtures::tone_mp3;

#[kithara::test]
fn decode_prepared_audio(tone_mp3: &'static [u8]) {
    // Pass the prepared bytes to the decoder under test.
    assert!(!tone_mp3.is_empty());
}
```

Fixture providers read already-built assets. Signal generation, including tiny
PCM inputs, belongs in `src/defs/` and runs through `build.rs`. Async providers
may own local servers and return them with the prepared input; test parameters
use `#[future(awt)]` to receive those resources after preparation.

## Key Types

- `store::STORE_ENV` — `KITHARA_FIXTURE_CACHE`, the required store root. CI
  points it at a persisted directory so a fresh job starts warm.
- `store::asset_id` — stable identity of one case.
- `store::read_entry` / `store::write_entry` — a hit-or-miss read and an atomic
  write; an empty file counts as a miss.
- `store::lock_entry` — the exclusive producer lock for one entry.

- `signal::Wave` — the waveform vocabulary.
- `signal::Pcm` — interleaved 16-bit PCM in memory.
- `signal::wav` / `signal::header` — the RIFF writer.

### Layout

- `src/defs/` — generator bodies, one function per asset, each carrying its
  cases. These compile into the build script only, never into the library.
- `src/signal/` — waveforms, PCM buffers, and the RIFF writer. The workspace's
  one waveform implementation for build-time inputs and signal assertions.
- `src/fmp4/` — the fMP4 mux: an `EncodedTrack` in, init and media segments out.
  The build script packages both embedded bodies and registered HLS variants.
- `build.rs` — resolves every declared case against the store, produces what is
  missing, and writes the accessor module.
- `src/store.rs` — the store itself: identity, namespace, atomic writes, and the
  cross-process lock that keeps two producers off one entry.

An asset declared `#[kithara::asset(..., embed)]` is baked into the binary with
`include_bytes!` instead of being read from disk at run time. It is still
generated once, into the store, like every other asset.

See [crate contracts](https://github.com/zvuk/kithara/wiki/kithara-test-fixtures)
for the store layout, invalidation, and build-time preparation contracts.
