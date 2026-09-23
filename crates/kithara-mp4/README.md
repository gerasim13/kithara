<div align="center">

<img src="https://raw.githubusercontent.com/zvuk/kithara/main/logo.svg" alt="kithara" width="300">

</div>

<div align="center">

[![crates.io](https://img.shields.io/crates/v/kithara-mp4.svg)](https://crates.io/crates/kithara-mp4)
[![docs.rs](https://docs.rs/kithara-mp4/badge.svg)](https://docs.rs/kithara-mp4)
[![License](https://img.shields.io/badge/license-MPL--2.0-blue.svg)](LICENSE)

</div>

# kithara-mp4

Fragmented-mp4 box walk over a random-access byte source.

An mp4 index needs box headers, not payload. The walk reads each header and seeks over the box it does not need, so `mdat` — every byte of the track — is never transferred. Peak memory tracks the layout rather than the track length: an eight-megabyte file yields its layout from a hundred and twenty-eight kilobytes of reads, and a longer track costs the same.

## Usage

```rust
use std::io;

use kithara_mp4::{Fmp4Layout, ReadAt};

// Any random-access source: a cached file, a mapped region, a byte slice.
struct Bytes(Vec<u8>);

impl ReadAt for Bytes {
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
        let start = usize::try_from(offset).map_err(io::Error::other)?;
        let Some(tail) = self.0.get(start..) else {
            return Ok(0);
        };
        let n = tail.len().min(buf.len());
        buf[..n].copy_from_slice(&tail[..n]);
        Ok(n)
    }
}

let source = Bytes(std::fs::read("track.m4a")?);
let total = u64::try_from(source.0.len())?;

if let Some(layout) = Fmp4Layout::read(&source, total) {
    let timescale = u64::from(layout.timescale());
    for fragment in layout.fragments() {
        let seconds = fragment.decode_ticks / timescale;
        println!("{:?} starts at {seconds}s", fragment.byte_range);
    }
}
# Ok::<(), Box<dyn std::error::Error>>(())
```

A media segment carries no `moov`, so it has no layout to read; `read_samples` walks its `(moof, mdat)` pairs instead and enumerates the individual samples a decoder feeds its codec.

```rust
# use kithara_mp4::ReadAt;
# fn demo<S: ReadAt>(segment: &S, total: u64, track_id: u32) -> Result<(), kithara_mp4::Mp4Error> {
for sample in kithara_mp4::read_samples(segment, total, track_id)? {
    let _access_unit = sample.byte_range;
    let _pts = sample.decode_ticks;
}
# Ok(())
# }
```

`Fmp4Layout::read` returns `None` for anything it cannot index: bytes that do not parse as mp4, a classic non-fragmented file with no `moof` chain, a track whose timescale is unavailable, or a malformed fragment.

Times come out as media ticks against the track's timescale. Converting them to a clock type, and projecting fragments onto whatever per-segment descriptor a protocol speaks, belongs to the caller — a file source fills in its own single-variant descriptor, an HLS variant has no use for this crate at all because its segment boundaries come from the playlist.

## Key Types

<table>

<tr><th>Type</th><th>Kind</th><th>Role</th></tr>

<tr><td><code>ReadAt</code></td><td>trait</td><td>Random-access byte source the walk pulls box headers from; one method, <code>read_at(offset, buf)</code></td></tr>

<tr><td><code>Fmp4Layout</code></td><td>struct</td><td>Fragment layout of one file: the init range, the audio timescale, and every fragment in file order</td></tr>

<tr><td><code>Fragment</code></td><td>struct</td><td>One <code>moof</code> + <code>mdat</code> fragment — byte range, decode ticks, duration ticks</td></tr>

<tr><td><code>read_samples</code></td><td>fn</td><td>Walks a <code>moov</code>-less media segment and enumerates one track's samples</td></tr>

<tr><td><code>Sample</code></td><td>struct</td><td>One access unit — byte range, absolute decode ticks, duration ticks</td></tr>

<tr><td><code>Mp4Error</code></td><td>struct</td><td>Why a walk could not deliver; carries a fixed phrase a caller wraps in its own error vocabulary</td></tr>

</table>

## Features

The crate has no cargo features. It carries no decoder, no allocator policy, and no protocol vocabulary: it depends on `re_mp4` for box shapes and `tracing` for the reasons a walk gave nothing back, and a fixed sixteen-kilobyte window is the whole of its scratch.

## Integration

`kithara-file` builds its seek index from a fully cached track through `Fmp4Layout`, and `kithara-decode` enumerates each media segment's frames through `read_samples`. Consumers implement `ReadAt` over whatever storage they already hold; nothing here reads a path or owns a handle.

See [crate contracts](https://github.com/zvuk/kithara/wiki/kithara-mp4) for the walk's guarantees and the layout it promises.
