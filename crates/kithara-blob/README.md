<div align="center">

<img src="https://raw.githubusercontent.com/zvuk/kithara/main/logo.svg" alt="kithara" width="300">

</div>

<div align="center">

[![crates.io](https://img.shields.io/crates/v/kithara-blob.svg)](https://crates.io/crates/kithara-blob)
[![docs.rs](https://docs.rs/kithara-blob/badge.svg)](https://docs.rs/kithara-blob)
[![License](https://img.shields.io/badge/license-MIT%2FApache--2.0-blue.svg)](https://github.com/zvuk/kithara/blob/main/LICENSE-MIT)

</div>

# kithara-blob

Versioned little-endian byte framing for stored artifacts. An artifact owner
writes and reads only its own body; this crate frames the version header and
hands the body a cursor that never trusts a length prefix.

## Usage

```rust
use kithara_blob::{Blob, BlobError, Reader, Writer, from_bytes, to_bytes};

#[derive(Debug, PartialEq)]
struct Tempo {
    bpm: f64,
    beats: u64,
}

impl Blob for Tempo {
    const VERSION: u32 = 1;

    fn decode(reader: &mut Reader<'_>) -> Result<Self, BlobError> {
        Ok(Self {
            bpm: reader.read_f64()?,
            beats: reader.read_u64()?,
        })
    }

    fn encode(&self, writer: &mut Writer<'_>) {
        writer.write_f64(self.bpm);
        writer.write_u64(self.beats);
    }
}

fn round_trip() -> Result<(), BlobError> {
    let tempo = Tempo { bpm: 128.0, beats: 512 };
    let bytes = to_bytes(&tempo);
    assert_eq!(from_bytes::<Tempo>(&bytes)?, tempo);
    Ok(())
}
```

## Key Types

<table>

<tr><th>Type</th><th>Role</th></tr>

<tr><td><code>Blob</code></td><td>An artifact with a versioned body encoding</td></tr>

<tr><td><code>Reader</code></td><td>Cursor that reads a body and refuses an untrusted length</td></tr>

<tr><td><code>Writer</code></td><td>Append-only little-endian writer over caller storage</td></tr>

<tr><td><code>BlobError</code></td><td>Version, fingerprint, size, allocation, or corruption failure</td></tr>

<tr><td><code>MAX_PREALLOC</code></td><td>Cap on speculative preallocation from a length prefix</td></tr>

</table>

## Integration

Artifact owners implement `Blob` beside the value they encode; the domain
meaning of what the bytes carry stays with that owner, and this crate stays
free of any domain.

See [crate contracts](https://github.com/zvuk/kithara/wiki/kithara-blob) for detailed contracts, invariants, and internals.
