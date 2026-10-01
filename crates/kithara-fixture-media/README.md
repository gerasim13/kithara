<div align="center">

<img src="https://raw.githubusercontent.com/zvuk/kithara/main/logo.svg" alt="kithara" width="300">

</div>

<div align="center">

[![License](https://img.shields.io/badge/license-MIT%2FApache--2.0-blue.svg)](https://github.com/zvuk/kithara/blob/main/LICENSE-MIT)

</div>

# kithara-fixture-media

Workspace crate (`publish = false`) that owns the media both halves of the
fixture pipeline agree on: the synthetic signals a fixture is rendered from,
the fMP4 and HLS shapes it is packaged in, and the content-addressed store it
is kept in. `kithara-fixture-gen` writes fixtures with these types at build
time; `kithara-test-fixtures` reads them back with the same types at run time.

The `cache-version` file here selects the shared store revision.

## Usage

```rust
use kithara_fixture_media::signal::{Wave, wav};

let body = wav(44_100, 2, 4_410, Wave::sine(440.0));
assert_eq!(body.len(), 44 + 4_410 * 2 * 2);
```

## Key Types

<table>

<tr><th>Type</th><th>Role</th></tr>

<tr><td><code>signal::Wave</code> / <code>signal::Pcm</code></td><td>The waveform vocabulary and interleaved 16-bit PCM rendered from it</td></tr>

<tr><td><code>fmp4::Fmp4Package</code></td><td>One track muxed into an init segment and media segments</td></tr>

<tr><td><code>store::asset_id</code> / <code>store::formatted_asset_id</code></td><td>The stable identity of one case, the second keyed by its format sample</td></tr>

<tr><td><code>store::read_entry</code> / <code>store::write_entry</code></td><td>A hit-or-miss read and an atomic write dated to one fixed instant; an empty file counts as a miss</td></tr>

<tr><td><code>store::write_stamp</code></td><td>The namespace stamp a build script watches to notice the namespace's removal</td></tr>

<tr><td><code>store::lock_entry</code></td><td>The exclusive producer lock for one entry</td></tr>

</table>

## Features

- `native` — the store, the fMP4 muxer and the packaged-variant shapes:
  everything that touches the host filesystem or the encoder. Without it only
  the portable signals compile, which is what a wasm build reads.

## Integration

`kithara-fixture-gen` depends on this crate with `native` at build time, and
`kithara-test-fixtures` re-exports `signal`, `fmp4` and `store` from it, so a
test names them through the fixture crate.
