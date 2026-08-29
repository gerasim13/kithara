<div align="center">

<img src="../../logo.svg" alt="kithara" width="300">

</div>

<div align="center">

[![License](https://img.shields.io/badge/license-MIT%2FApache--2.0-blue.svg)](../../LICENSE-MIT)

</div>

# kithara-fixtures

Audio test assets produced at build time and served from a content-addressed
store on disk. A test asks for bytes and gets them; nothing is synthesized or
encoded inside a test's wall-clock deadline.

## Layout

- `src/defs/` — generator bodies, one function per asset, each carrying its
  cases. These compile into the build script only, never into the library.
- `build.rs` — resolves every declared case against the store, produces what is
  missing, and writes the accessor module.
- `src/store.rs` — the store itself: identity, namespace, atomic writes, and the
  cross-process lock that keeps two producers off one entry.

An asset declared `#[kithara::asset(..., embed)]` is baked into the binary with
`include_bytes!` instead of being read from disk at run time. It is still
generated once, into the store, like every other asset.

## Usage

```rust
use kithara_fixtures::store;

let root = store::root_from_env();
let namespace = store::namespace(&root, "0123456789abcdef");
let id = store::asset_id("sine_wav", "a440_6s");

if let Some(bytes) = store::read_entry(&namespace, &id, "wav") {
    // serve the entry
}
```

## Key items

- `store::STORE_ENV` — `KITHARA_FIXTURE_CACHE`, the store root override. CI
  points it at a persisted directory so a fresh job starts warm.
- `store::asset_id` — stable identity of one case.
- `store::read_entry` / `store::write_entry` — a hit-or-miss read and an atomic
  write; an empty file counts as a miss.
- `store::lock_entry` — the exclusive producer lock for one entry.

See [CONTEXT.md](CONTEXT.md) for the store layout, the invalidation contract,
and why the generators stay out of the library.
