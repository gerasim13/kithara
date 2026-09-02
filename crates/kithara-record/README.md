<div align="center">

<img src="https://raw.githubusercontent.com/zvuk/kithara/main/logo.svg" alt="kithara" width="300">

</div>

<div align="center">

[![crates.io](https://img.shields.io/crates/v/kithara-record.svg)](https://crates.io/crates/kithara-record)
[![docs.rs](https://docs.rs/kithara-record/badge.svg)](https://docs.rs/kithara-record)
[![License](https://img.shields.io/badge/license-MIT%2FApache--2.0-blue.svg)](https://github.com/zvuk/kithara/blob/main/LICENSE-MIT)

</div>

# kithara-record

Storage-neutral recording over Kithara's continuous encoder and container
sessions.

`RecordingCore` converts interleaved `f32` PCM into one independently playable
configured part. `RecordingSink` is the transactional byte boundary: write at an
offset, commit the final length, or abort. Application adapters decide where
those bytes live.

See [CONTEXT.md](CONTEXT.md) for transaction and failure invariants.
