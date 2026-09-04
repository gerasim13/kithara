---
name: run-kithara-app
description: Build and run Kithara. Default target is the desktop app (kithara-app, GUI). Use the design-system gallery, iOS, or Android sections ONLY when the user explicitly asks for that target.
---

## Desktop (default)

```bash
cargo run -p kithara-app
```

Add track paths/URLs as extra args to play specific tracks; without any, it
uses the built-in defaults.

## Design-system gallery — only if explicitly requested

```bash
cargo run -p kithara-ui --example gallery --features capture
```

The retained host, and everything the gallery can be asked to do instead of
opening — photographing its pages, or one control of one page, comparing two
sets — are flags:

```bash
cargo run -p kithara-ui --features capture,masonry --example gallery -- --host retained
```

```bash
cargo run -p kithara-ui --example gallery --features capture -- --help
```

## iOS — only if explicitly requested

```bash
just platform apple demo
```

## Android — only if explicitly requested

```bash
just platform android build
cd android && ./gradlew :example:installDebug
```
