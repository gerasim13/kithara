<div align="center">

<img src="../logo.svg" alt="kithara" width="300">

[![License](https://img.shields.io/badge/license-MIT%2FApache--2.0-blue.svg)](../LICENSE-MIT)

</div>

# Kithara for Android

Kotlin bindings for the Kithara audio engine: queue-based playback with seek,
adaptive bitrate, and reactive state through `StateFlow`. The Rust core is
exposed through UniFFI and ships as a Gradle module carrying JNI slices for
`arm64-v8a` and `x86_64`.

## Build

```bash
just platform android                          # JNI libraries + Kotlin bindings, debug
just platform android build --profile release
just platform android aar                      # release AARs
just platform android run                      # boot an emulator, install and launch the demo
just platform android test                     # instrumented tests on an emulator
```

The recipe checks `cargo-ndk`, `rustup`, and the installed Rust targets itself
and prints the exact command for whatever is missing. The NDK comes from
`ANDROID_NDK_HOME`, `ANDROID_NDK_ROOT`, or `NDK_HOME`, else the newest version
installed under the Android SDK. Toolchain versions are pinned in
`.config/ci-pins.toml`.

Generated output lands in `android/lib/build/generated/`: `jniLibs/` per ABI and
`uniffi/kotlin/`. The AAR export writes `android/lib/build/outputs/aar/` —
`kithara.aar` plus `rust-tls.aar`, the rustls platform verifier, which must be
distributed alongside it. A file-dependency integration also needs `jna` and
`kotlinx-coroutines-core`.

Traps:

- The Android target graph excludes `kithara-workspace-hack`; host feature
  unification must not leak into NDK builds, and the build must leave
  `Cargo.lock` and the generated workspace-hack manifest untouched.
- Gradle *configuration* — not just the build — shells out to `cargo metadata`
  from `settings.gradle.kts` and `lib/build.gradle.kts` to locate the rustls
  verifier AAR, so a sync fails without a reachable cargo. It looks at `CARGO`,
  then `~/.cargo/bin/cargo`, then `PATH`; Android Studio does not always
  inherit a shell `PATH`.
- The generated Kotlin is a source directory of the `lib` module. The IDE
  cannot resolve it until it exists, so run the build once before the first
  sync.

## Quick Start

```kotlin
// Application.onCreate:
Kithara.initialize(applicationContext)

val player = KitharaPlayer()
val item = KitharaPlayerItem("https://example.com/track.mp3")

lifecycleScope.launch {
    player.insert(item)
    player.play()
}
```

## Usage

### Playback and queue

```kotlin
player.play()
player.pause()
player.playingRate = 1.5f            // target playback speed

player.insert(second, after = first)
player.remove(first)
player.removeAllItems()

try {
    player.seek(30.0)
} catch (e: KitharaError) { /* seek failed */ }
```

### State

```kotlin
lifecycleScope.launch {
    player.state.collect { println("${it.status} ${it.currentTime}s / ${it.duration}s rate ${it.rate}") }
}
lifecycleScope.launch { player.currentItemChanges.collect { /* item switched */ } }
lifecycleScope.launch { item.state.collect { it.error?.let(::println) } }

// Explicit preload is optional; insert can auto-load with player config.
lifecycleScope.launch { item.load() }
```

### Per-item options

```kotlin
val item = KitharaPlayerItem(
    url = "https://example.com/stream.m3u8",
    preferredPeakBitrate = 256_000.0,
    preferredPeakBitrateForExpensiveNetworks = 128_000.0,
    additionalHeaders = mapOf("Authorization" to "Bearer <token>"),
)
```

### Cache location and layout

`Kithara.initialize` creates one process-wide `AssetStore` rooted at
`<application cacheDir>/kithara`, shared by every default-configured player. A
different root or custom path layout means constructing another store:

```kotlin
val layouts = AssetLayoutRegistry().apply {
    register(MyFileAssetLayout(), AssetLayoutTarget.File)
    register(MyHlsAssetLayout(), AssetLayoutTarget.Hls)
}
val store = AssetStore(
    root = application.filesDir.resolve("kithara-cache").absolutePath,
    layouts = layouts,
)
val player = KitharaPlayer(config = KitharaPlayer.Config(store = store))
```

Ownership: `AssetLayoutRegistry` is the native Rust registry, so `register`
routes the layout into Rust immediately and Kotlin keeps no second copy. A store
captures a registry snapshot at construction — later registrations reach only
later stores — and one store can then be shared by any number of players. An
empty registry uses Kithara's defaults. `MyFileAssetLayout` and
`MyHlsAssetLayout` implement `AssetLayout`; their `root(source)` and
`path(resource)` callbacks choose paths below the outer cache directory, and
invalid callback output is rejected rather than rewritten or replaced with a
default. The `AssetLayout` API contract owns the portable component rules.

For signed media URLs whose path is shared by several tracks or variants, use
the built-in query-identity layout, registered once per protocol that serves
those URLs:

```kotlin
val queryIdentity = AssetLayouts.queryIdentity(
    rules = listOf(
        CacheIdentityRule(
            domains = listOf("media.example.com", "*.cdn.example.com"),
            queryParameters = listOf("track_id", "variant"),
        ),
    ),
)
```

Rules are checked in order. Domain patterns are exact hosts, `*.example.com`
for subdomains only, or `*` for every host. Only the named parameters
contribute to cache identity, so rotating signatures and expiry timestamps do
not split the cache; selected values are hashed into safe path components and
the raw query is never written to disk.

## Architecture

| Layer | Contract |
|-------|----------|
| `com.kithara` | Public Kotlin API, `StateFlow`-based reactive state |
| `com.kithara.ffi` | Generated UniFFI bindings — not for direct use |
| `libkithara_ffi.so` | Rust core (kithara-play, kithara-ffi) |

The release AAR decodes the AAC family, MP3, and FLAC through the Android
`MediaCodec` backend over `MediaExtractor`.

## Demo App

[`example`](example) is a minimal player: URL or local file, play/pause/stop,
reactive status. `just platform android run` boots an emulator, installs it, and
launches it.

## License

Licensed under either of [Apache License, Version 2.0](../LICENSE-APACHE) or
[MIT license](../LICENSE-MIT) at your option.
