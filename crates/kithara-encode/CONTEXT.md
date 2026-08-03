# kithara-encode — Context

Contracts and invariants for the kithara-encode crate; the README is the overview.

Test-only infrastructure (`publish = false`), consumed by `kithara-integration-tests` to generate encoded fixtures and packaged tracks. It consumes canonical `AudioCodec`, `ContainerFormat`, and `MediaInfo` from `kithara-stream`.

## PCM input contract

`PcmSource::read_pcm_at` must yield **interleaved packed i16** bytes; every backend derives its frame stride as `channels * size_of::<i16>()`. A source that hands back any other layout produces silent garbage, not an error.

## Packaged encoding

`PackagedEncodeRequest::validate()` is shared by every packaged backend and rejects a zero `timescale`, a zero `packets_per_segment`, or a `PcmSource` without a finite `total_byte_len()` (packaged encoding is offline-only).

Routing inside the single `InnerEncoder` implementation:

- `AacLc` — FFmpeg AAC, natural frame 1024 samples.
- `AacHe` / `AacHeV2` — in-tree fdk-aac `AacHeEncoder` (AOT SBR / PS), natural frame 2048 samples, **stereo input only**.
- `Flac` — FFmpeg FLAC, natural frame 4608 samples.
- Any other codec — `EncodeError::UnsupportedCodec`.

`EncoderFactory::frame_samples(codec)` returns those same natural frame sizes; segmenters must use it rather than assuming 1024.

### `EncodedTrack` output contract

- `codec_config` is codec-dependent: FLAC yields the 34-byte STREAMINFO body, HE-AAC yields the AudioSpecificConfig, and **AAC-LC yields an empty blob** — the muxer has to synthesize the ASC itself.
- `media_info` is the request's info with `codec`, `sample_rate`, and `channels` overridden from the actual encode. The FLAC and HE-AAC paths additionally force `container = Fmp4`; the AAC-LC path leaves the requested container untouched.
- FFmpeg backends rescale packets from `1/sample_rate` to `1/timescale` and normalize every timestamp against the first packet's `min(pts, dts)`, so a track always starts at 0; `is_sync` comes from the packet's key flag. The fdk HE-AAC path synthesizes `pts = dts` itself in timescale units from the encoder frame length and marks every access unit `is_sync`.
- `encoder_delay` and `trailing_delay` are pass-through values from the request — this crate never measures priming or padding.

## Byte encoding

`create_bytes` covers `Mp3`, `Flac`, `Aac`, and `M4a`. `BytesEncodeTarget` owns the codec/container/extension mapping and the 128 kbps default for the lossy targets; the FFmpeg bytes backend adds the MIME type and forces `compression_level=5` with no bit rate for FLAC. An explicit `bit_rate` on the request wins over the target default. Byte encoding muxes through a real file in a `tempfile::tempdir()`, so it needs a writable temp dir and is not usable from a read-only sandbox.

`normalize_flac_codec_config` is the public helper for callers holding a raw FLAC config blob: it accepts a bare 34-byte STREAMINFO, a metadata block carrying STREAMINFO, or `fLaC`-prefixed bytes, and errors otherwise.

## Platform

FFmpeg is initialized once per process behind a `OnceLock`. On `wasm32` the ffmpeg and fdk modules are not compiled at all and every factory entry point returns `EncodeError::InvalidInput("encoding is not supported on wasm32")`.
