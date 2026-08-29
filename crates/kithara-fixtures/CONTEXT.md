# kithara-fixtures — Context

Contracts and invariants for kithara-fixtures; the README is the overview.

## Store Layout

```
<root>/<fingerprint>/<id>.<ext>
<root>/<fingerprint>/<id>.lock
```

- **root** — `KITHARA_FIXTURE_CACHE` when set, otherwise
  `<temp_dir>/kithara-fixture-cache`. CI points it at a persisted directory, so
  a fresh job finds every entry already there.
- **fingerprint** — 16 hex characters identifying the build that produced the
  entries below it. See *Invalidation*.
- **id** — `sha2-256(func ‖ 0x00 ‖ case)`, first 16 bytes, hex. The separator is
  what keeps `("sine", "wav_6s")` and `("sine_wav", "6s")` apart.

An entry is one file. A **zero-length file is a miss**, not an empty asset: a
producer that dies mid-write must not leave something a reader will serve.
Writes go to `<id>.<ext>.tmp.<pid>`, are `sync_all`ed, then renamed — a reader
sees the entry whole or not at all.

## Identity Without `module_path!()`

The id hashes the function name and the case name, and nothing else. It
deliberately omits the module path, which inside a build script expands to
`build_script_build::defs::…` — an artefact of how the generators are compiled,
not a property of the asset. Uniqueness holds by construction: every accessor
lands in one flat generated module, so two cases sharing both halves could not
coexist there. The build script asserts it anyway.

## Invalidation

Content-addressing over the accessor name alone cannot notice that a generator
changed its output. The fingerprint is what stands between a changed generator
and the bytes the previous one produced: it hashes the generator sources and the
inputs they encode with. A changed generator lands in a new namespace and
regenerates; the old namespace stays until the store is pruned, which is what
lets a branch switch back and forth without paying twice.

The fingerprint is a build-script concern; the store only receives it as an
opaque directory name.

## Two Producers, One Entry

Every consumer of the store — parallel build scripts, several checkouts sharing
one root — races on the same entries. The protocol is double-checked:

1. Read the entry. A hit ends it.
2. Take `lock_entry`, which blocks until the entry is this process's to produce.
3. Read again. Another producer may have finished while this one waited.
4. Produce, write atomically, release.

`EntryLock` releases with its file handle, so a producer that panics or is
killed does not wedge the store.

## Generators Stay Out Of The Library

`src/defs/` is reached only through `#[path]` from `build.rs`. Two consequences,
both load-bearing:

- The encoding stack (`kithara-encode` and the FFmpeg / fdk-aac it links) is a
  **build-dependency**. It never enters a target build, so it costs nothing on
  wasm, on iOS, or in any product binary.
- Generation happens exactly once per fingerprint, in the build script. Nothing
  in the library can synthesize an asset, which is the whole point: a test's
  deadline never contains an encode.

## Transitional Coupling

`tests/src/fixture_cache.rs` is the disk cache this store replaces. It is frozen
for the duration of the migration and dies once its last consumer moves over.
Nothing new should be built on it.
