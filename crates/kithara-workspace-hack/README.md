<div align="center">

<img src="https://raw.githubusercontent.com/zvuk/kithara/main/logo.svg" alt="kithara" width="300">

</div>

<div align="center">

[![License](https://img.shields.io/badge/license-MIT%2FApache--2.0-blue.svg)](https://github.com/zvuk/kithara/blob/main/LICENSE-MIT)

</div>

# kithara-workspace-hack

Internal workspace-hack crate managed by [cargo-hakari](https://docs.rs/cargo-hakari). Not published; not for direct use. Hakari produces a unified feature set for third-party dependencies so Cargo unifies them into a single build per dep, avoiding duplicate compilation across participating workspace members. Target platforms and exclusions are owned by [the Hakari configuration](../../.config/hakari.toml).

## Usage

Use the cargo-hakari version pinned in [`.config/ci-pins.toml`](../../.config/ci-pins.toml). The dependency section in `Cargo.toml` is generated; do not edit it manually. To regenerate and synchronize member dependencies:

```bash
just deps hakari generate
just deps hakari manage-deps --yes
just deps hakari generate
just deps hakari-check
```

Run after adding or removing workspace dependencies, or when CI flags hakari drift.

### Configuration

The hakari config lives in `.config/hakari.toml` (or workspace root if present). It controls platform/feature combinations and which workspace crates participate.
