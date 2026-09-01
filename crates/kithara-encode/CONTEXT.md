# kithara-encode — Context

Contracts and invariants for the kithara-encode crate; the README is the overview.

The crate owns three roles: portable continuous PCM/WAV encoding, streaming AAC-LC encoding for the live broadcast path, and offline encoding that `kithara-integration-tests` uses to generate encoded fixtures and packaged tracks. It consumes canonical `AudioCodec`, `ContainerFormat`, and `MediaInfo` from `kithara-stream`.

## Backends

The portable PCM/WAV sessions are always compiled and need no native backend. Two optional native encoders also live here and the caller names the one it wants. `ffmpeg` links the system `FFmpeg` and owns byte encoding, FLAC, and the offline AAC-LC fixtures; `fdk-aac` builds from vendored sources into the binary and owns HE-AAC v1/v2 offline plus the streaming AAC-LC a live broadcast runs on. Both native backends are default features, but a build with neither still has the portable sessions.

`StreamBackend` follows `kithara-decode`'s `DecoderBackend`: a variant exists only where its feature is compiled in, so asking for a backend this build does not carry is a compile error rather than a runtime miss, and a backend that fails is terminal — neither one stands in for the other. The offline routes answer per codec instead: a codec whose backend is configured out is `UnsupportedCodec`, and byte encoding without `ffmpeg` is `InvalidInput`.

## Continuous portable encoding

`EncodeConfig` is the shared `bon` configuration for a continuous encoder/container pair. `sample_rate` and `channels` are required; `codec` defaults to `Pcm`, `container` defaults to `Wav`, and `packet_frames` defaults to 1024. The current portable profile is WAV with little-endian IEEE float32 samples. Other codec/container pairs return typed unsupported-profile errors and are never replaced with the default.

`EncoderSession` accepts complete interleaved f32 frames. It emits the same `EncodedAccessUnit` sequence for the same samples regardless of input chunking, and `finish` emits the final partial packet. Access-unit timestamps are PCM frame positions and start at zero.

`ContainerSession` accepts those access units in timestamp order and returns absolute `ContainerWrite` operations instead of owning a file or byte store. WAV payload writes start after the fixed header; `finish` returns the final header rewrite and exact committed length. A caller with a known frame count must call `validate_frame_count` before doing work. RIFF's hard size boundary returns `ContainerLimitExceeded` rather than truncating or switching formats.

## Streaming encoding

`StreamEncoder` is the canonical AAC-LC encode path. Its builder names the backend, sample rate, channels, bit rate, and timescale and opens one encoder for one continuous stream; `push` takes **interleaved f32** and returns the access units that audio completed; `finish` flushes the encoder and returns the rest. The instance lives for the whole stream — a per-segment encoder would restart the priming frame and click at every boundary.

- `push` takes full frames: a slice whose length is not a multiple of the channel count is rejected. Samples are expected in `[-1.0, 1.0]`; `FFmpeg` takes larger magnitudes through to the linked encoder, where the outcome depends on its sample format, and fdk clamps them onto full scale.
- The filter graph holds up to `FRAME_SAMPLES - 1` samples between calls and hands the encoder whole frames, so chunk size does not reach the encoder: the same audio pushed in any chunking yields byte-identical access units with identical timestamps. `finish` is what releases the tail.
- A `push` that returns a backend error leaves the encoder mid-frame — drop it and open a new one rather than pushing again.
- Timestamps start at 0 and access-unit boundaries are what gets rescaled into `1/timescale`, so durations tile the pts timeline exactly even when the ratio is fractional. Each access unit carries `FRAME_SAMPLES` (1024) samples per channel and is a sync point, and `pts == dts`.
- The encoder takes the source's own channel layout: finishing the builder fails when AAC-LC has no layout for the requested channel count rather than silently downmixing or upmixing. fdk carries 1 to 6 channels and audio from 8 kHz to 96 kHz.
- Priming is measured per backend, not assumed: past the pushed audio `FFmpeg` hands back one `FRAME_SAMPLES` frame and fdk two. Whoever judges the decoded signal skips the larger of the two.
- fdk takes the pushed f32 down to i16, which is the input libfdk reads; `FFmpeg` encodes the f32 as it comes. Its access units carry no transport header, and its stream ends on libfdk's own end-of-input signal — that signal is what puts the tail of a broadcast into the last segment.

## PCM input contract

`PcmSource::read_pcm_at` must yield **interleaved packed i16** bytes; every backend derives its frame stride as `channels * size_of::<i16>()`. A source that hands back any other layout produces silent garbage, not an error. The AAC-LC offline path reads those bytes, scales them by `1/32768` into f32, and pushes them through `StreamEncoder`; the FLAC and bytes paths hand the i16 bytes to `FFmpeg` directly, and the HE-AAC path reads them into the in-tree fdk encoder.

## Packaged encoding

`PackagedEncodeRequest::validate()` is shared by every packaged backend and rejects a zero `timescale`, a zero `packets_per_segment`, or a `PcmSource` without a finite `total_byte_len()` (packaged encoding is offline-only).

Routing inside the single `InnerEncoder` implementation:

- `AacLc` — `StreamEncoder` on the `FFmpeg` backend fed the whole source, natural frame 1024 samples.
- `AacHe` / `AacHeV2` — in-tree fdk-aac `AacHeEncoder` (AOT SBR / PS), natural frame 2048 samples, **stereo input only**.
- `Flac` — FFmpeg FLAC, natural frame 4608 samples.
- Any other codec — `EncodeError::UnsupportedCodec`.

`EncoderFactory::frame_samples(codec)` returns those same natural frame sizes; segmenters must use it rather than assuming 1024.

### Scratch ownership

`EncoderFactory::encode_packaged` takes the caller's typed `PoolRegion` and reuses its byte and sample pools for temporary byte and converted-sample storage. Production encoding must not construct a component-local region. A scratch lease ends inside the encode call and is returned before the resulting track can outlive the request.

### `EncodedTrack` output contract

- `EncodedAccessUnit::bytes` is a long-lived owned `Vec<u8>`, not pooled scratch. It must not retain a pool lease: access units can outlive the encoder request and move independently through muxing, caching, and transport.
- `codec_config` is codec-dependent: FLAC yields the 34-byte STREAMINFO body, HE-AAC yields the AudioSpecificConfig, and **AAC-LC yields an empty blob** — the muxer has to synthesize the ASC itself.
- `media_info` is the request's info with `codec`, `sample_rate`, and `channels` overridden from the actual encode. The FLAC and HE-AAC paths additionally force `container = Fmp4`; the AAC-LC path leaves the requested container untouched.
- FFmpeg backends rescale packets from `1/sample_rate` to `1/timescale` and normalize every timestamp against the first packet's `min(pts, dts)`, so a track always starts at 0; `is_sync` comes from the packet's key flag. The fdk HE-AAC path synthesizes `pts = dts` itself in timescale units from the encoder frame length and marks every access unit `is_sync`.
- `encoder_delay` and `trailing_delay` are pass-through values from the request — this crate never measures priming or padding.

## Byte encoding

`create_bytes` covers `Mp3`, `Flac`, `Aac`, and `M4a`. `BytesEncodeTarget` owns the codec/container/extension mapping and the 128 kbps default for the lossy targets; the FFmpeg bytes backend adds the MIME type and forces `compression_level=5` with no bit rate for FLAC. An explicit `bit_rate` on the request wins over the target default. Byte encoding muxes through a real file in a `tempfile::tempdir()`, so it needs a writable temp dir and is not usable from a read-only sandbox.

`normalize_flac_codec_config` is the public helper for callers holding a raw FLAC config blob: it accepts a bare 34-byte STREAMINFO, a metadata block carrying STREAMINFO, or `fLaC`-prefixed bytes, and errors otherwise.

## Platform

FFmpeg is initialized once per process behind a `OnceLock`. On `wasm32` the ffmpeg and fdk modules are not compiled at all and `StreamEncoder` is absent. `EncodeConfig`, `EncoderSession`, and `ContainerSession` still provide PCM/WAV float32; native-only `EncoderFactory` entry points return `EncodeError::InvalidInput("encoding is not supported on wasm32")`.

libfdk's vendored `FDK_archdef.h` has no branch for MSVC on ARM64, so the `fdk-aac` feature does not build for `aarch64-pc-windows-msvc`.
