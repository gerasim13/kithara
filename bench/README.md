# Superpowered vs kithara Bench

Local benchmark for faster-than-realtime HLS AAC reads on macOS arm64.

## Prerequisites

- Superpowered SDK 2.8.1 checked out outside the repo:
  `export SUPERPOWERED_SDK_DIR=~/code/Superpowered`
- Xcode command line tools, `clang++`, Rust, Cargo, Python 3, and `curl`.
- `BENCH_RATE` defaults to `44100` from the spike. Override only when using
  different fixtures.
- `ffmpeg` is only needed to regenerate the TS fixture.

Regenerate the Superpowered TS fixture from the worktree root:

```bash
cd bench/fixtures-ts/shq
ffmpeg -allowed_extensions ALL -i ../../../assets/hls/index-shq-a1.m3u8 -map 0:a -c copy -f hls -hls_time 6 -hls_list_size 0 -hls_playlist_type vod -hls_segment_type mpegts -hls_segment_filename 'seg-%d.ts' index.m3u8
```

## Run

```bash
export SUPERPOWERED_SDK_DIR=~/code/Superpowered
BENCH_RATE=44100 bench/run.sh 5
```

One measured smoke run:

```bash
BENCH_RATE=44100 bench/run.sh 1
```

External URLs must be provided as a pair because kithara and Superpowered use
different container support paths:

```bash
bench/run.sh 1 --kit-url "https://example/fmp4-master.m3u8" --sp-url "https://example/ts/index.m3u8" --duration-range 219.5 220.5
```

`bench/run.sh` builds both kit-bench configs and sp-bench, starts one Python
`http.server` rooted at the worktree root, performs one warm-up per engine, then
runs rotated measured repetitions.

## JSON Contract

Each CLI prints exactly one JSON object on success.

| Field | Meaning |
|---|---|
| `engine` | `kithara` or `superpowered` |
| `decoder` | `symphonia`, `apple`, or `superpowered` |
| `ttfa_ms` | Baseline to first PCM |
| `wall_ms` | Baseline to EOF |
| `cpu_user_s` | `getrusage(RUSAGE_SELF)` user CPU delta |
| `cpu_sys_s` | `getrusage(RUSAGE_SELF)` system CPU delta |
| `cpu_total_user_s` | Whole-process user CPU at EOF |
| `cpu_total_sys_s` | Whole-process system CPU at EOF |
| `max_rss_bytes` | `ru_maxrss` at EOF; bytes on macOS |
| `samples` | Interleaved PCM samples |
| `pcm_frames` | `samples / channels` |
| `samplerate` | Output sample rate |
| `channels` | Output channel count |

Failures print stderr and no JSON line.

## Fixtures And Equivalence

Superpowered does not parse the repo fMP4 HLS fixture (`.m4s` +
`EXT-X-MAP`), so the primary comparison uses plan B:

- kithara opens `/bench/fixtures/master-shq.m3u8`, a single-variant master
  pointing at the original fMP4 media playlist.
- Superpowered opens `/bench/fixtures-ts/shq/index.m3u8`, a stream-copy MPEG-TS
  remux of the same AAC payload.

The AAC payload is the same, but frame counts are not exact because
Superpowered reports 1024-frame process blocks and the TS remux trims/pads
container edges slightly. The equivalence gate requires matching sample rate
and channel count, `max(pcm_frames) - min(pcm_frames) <= 44100`, and each run's
duration within `219.5..220.5` seconds unless overridden with
`--duration-range`.

Do not push this branch.
