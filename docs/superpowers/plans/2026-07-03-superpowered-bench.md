# Superpowered vs kithara HLS Bench — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.
> **Process override (user directive):** implementer = Codex (MCP, sandbox workspace-write); Claude orchestrates, executes everything that runs (clones, builds, spike, benchmark runs) and reviews between tasks. Steps are labeled **[Codex]** / **[Claude]**.

**Goal:** Two CLI binaries + runner that measure CPU / wall / TTFA / peak-RSS of a full faster-than-realtime HLS AAC read on Superpowered SDK vs kithara, per spec `docs/superpowers/specs/2026-07-03-superpowered-bench-design.md`.

**Architecture:** `bench/sp-bench` (C++, AdvancedAudioPlayer offline pump, Decoder fallback), `bench/kit-bench` (Rust, facade `Audio<Stream<Hls>>` + `block_on_underrun(true)`), `bench/run.sh` (build → preflight → ephemeral-port test_server → warm-up → N rotated runs → hard equivalence gate → median table). Stage-0 spike settles all unverified Superpowered facts before harness code.

**Tech Stack:** Rust edition 2024 (standalone crate, path-dep on `crates/kithara`), C++17 + clang, `$SUPERPOWERED_SDK_DIR` static lib, bash + python3 (stdlib only) for stats, existing `test_server` bin from `kithara-integration-tests`.

## Global Constraints

- Worktree: `/Volumes/Render/dev/worktrees/kithara-bench`, branch `bench/superpowered-compare`. **NEVER `git push`.**
- Only `bench/**` and `docs/superpowers/**` may change (plan-B exception: `bench/fixtures-ts/`).
- `bench/kit-bench` is NOT a workspace member: its `Cargo.toml` ends with an empty `[workspace]` table. Root `Cargo.toml`, workspace-hack, lints of the main repo stay untouched.
- Superpowered SDK is never committed. It lives at `$SUPERPOWERED_SDK_DIR` (default `~/code/Superpowered`), cloned from `github.com/superpoweredSDK/Low-Latency-Android-iOS-Linux-Windows-tvOS-macOS-Interactive-Audio-Platform`.
- macOS arm64 only. Benchmark URL: media playlist `/assets/hls/index-shq-a1.m3u8` (AAC-LC ~270 kbps, 37 segs, ≈220.2 s) — never `master.m3u8` (contains FLAC variant, SP can't pin variants).
- Metrics JSON contract (single stdout line, both CLIs, exact keys): `{"engine":..., "decoder":..., "ttfa_ms":..., "wall_ms":..., "cpu_user_s":..., "cpu_sys_s":..., "cpu_total_user_s":..., "cpu_total_sys_s":..., "max_rss_bytes":..., "samples":..., "pcm_frames":..., "samplerate":..., "channels":...}`. Baseline (rusage + monotonic snapshot) is taken immediately before the open call; `cpu_*_s` are deltas baseline→EOF; `cpu_total_*` = whole process (diagnostic); `pcm_frames = samples / channels`.
- Errors: any failure → non-zero exit, message to stderr, NO JSON line. Runner: any failed run or equivalence mismatch = whole benchmark run fails (no excluded samples).
- Commit gate caveat: pre-commit (`prek` → `just lint-fast`) runs workspace clippy; on a fresh target dir boring-sys2 bindings break under feature unification. Recipe if it fires: `cargo clean -p boring-sys2 -p boring2 && just lint-fast` run DIRECTLY to green, then commit. Do not `--no-verify` code commits.
- Commit messages end with: `Claude-Session: https://claude.ai/code/session_01HkW3jvjLY9K4MKEMWLPXfG`

---

### Task 1: Stage-0 spike — settle Superpowered facts

**Files:**
- Create: `bench/spike/spike.cpp`
- Create: `bench/spike/Makefile`
- Modify: `docs/superpowers/specs/2026-07-03-superpowered-bench-design.md` (append `## Spike findings`)

**Interfaces:**
- Produces: verified facts consumed by Tasks 2–5: (a) SP fMP4-HLS plays or plan B; (b) offline faster-than-realtime pumping works or Decoder/`--paced` path; (c) exact `Superpowered::Initialize` call + working key; (d) temp-folder API owner + exact names; (e) header/lib names + frameworks for the Makefile; (f) fixture `samplerate`/`channels`.
- Decision gates recorded in spec: `SPIKE-A` (container), `SPIKE-B` (pump speed), `SPIKE-C` (init/license), `SPIKE-D` (temp folder), `SPIKE-E` (build recipe), `SPIKE-F` (fixture rate).

- [ ] **Step 1 [Claude]: Clone the SDK and inventory it**

```bash
git clone --depth 1 https://github.com/superpoweredSDK/Low-Latency-Android-iOS-Linux-Windows-tvOS-macOS-Interactive-Audio-Platform.git ~/code/Superpowered
export SUPERPOWERED_SDK_DIR=~/code/Superpowered
ls "$SUPERPOWERED_SDK_DIR/Superpowered"          # headers + libs
ls "$SUPERPOWERED_SDK_DIR/Superpowered"/lib* 2>/dev/null; find "$SUPERPOWERED_SDK_DIR" -name "*.a" -maxdepth 3
grep -rn "Initialize" "$SUPERPOWERED_SDK_DIR/Superpowered/Superpowered.h" | head
grep -rn "ExampleLicenseKey\|Initialize(" "$SUPERPOWERED_SDK_DIR"/Examples* -r --include=*.cpp --include=*.mm 2>/dev/null | head -5
grep -n "setTempFolder\|TempFolder" "$SUPERPOWERED_SDK_DIR/Superpowered/"*.h
grep -n "HLSDownload\|HLSBufferingSeconds\|InternalBufferSizeSeconds\|internalBufferSizeSeconds" "$SUPERPOWERED_SDK_DIR/Superpowered/SuperpoweredAdvancedAudioPlayer.h" | head -20
```

Expected: header names, exact `Initialize` signature, the example license key used by SDK samples, `Superpowered::AdvancedAudioPlayer::setTempFolder` (or free function) signature, macOS static lib path (e.g. `libSuperpoweredmacOS.a`). Paste raw findings into the spec `## Spike findings`.

- [ ] **Step 2 [Claude]: Fixture sample rate (SPIKE-F)**

```bash
ls /Users/litvinenko-pv/code/kithara/assets/hls/ | grep shq | head -4
cat /Users/litvinenko-pv/code/kithara/assets/hls/init-shq-a1.mp4 \
    "$(ls /Users/litvinenko-pv/code/kithara/assets/hls/segment-shq*.m4s | head -1)" > /tmp/probe-shq.mp4
afinfo /tmp/probe-shq.mp4
```

Expected: `afinfo` prints data format incl. sample rate (e.g. `44100 Hz`) and channel count. Record as SPIKE-F.

- [ ] **Step 3 [Codex]: Write the spike program**

Adjust ONLY include names / `Initialize` call / temp-folder call to what Step 1 found; the control flow below is fixed. `bench/spike/spike.cpp`:

```cpp
#include <chrono>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <sys/resource.h>
#include "Superpowered.h"
#include "SuperpoweredAdvancedAudioPlayer.h"

// Usage: spike <media-playlist-url> <samplerate> <tempdir>
// Exit 0 = opened + decoded to EOF; prints timing facts for SPIKE-A/B.
int main(int argc, char **argv) {
    if (argc < 4) { fprintf(stderr, "usage: spike <url> <rate> <tmpdir>\n"); return 2; }
    const char *url = argv[1];
    const unsigned int rate = (unsigned int)atoi(argv[2]);

    Superpowered::Initialize("ExampleLicenseKey-WillExpire-OnNextUpdate"); // fix per Step 1
    Superpowered::AdvancedAudioPlayer::setTempFolder(argv[3]);             // fix per Step 1

    auto *player = new Superpowered::AdvancedAudioPlayer(rate, 0, 0 /* offline */);
    player->timeStretching = false;
    player->HLSAutomaticAlternativeSwitching = false;
    // Step 1 tells the real download-everything constant name:
    player->HLSBufferingSeconds = Superpowered::AdvancedAudioPlayer::HLSDownloadRemaining;

    const auto t0 = std::chrono::steady_clock::now();
    player->openHLS(url);

    float buf[1024 * 2 + 64];
    bool playing = false, gotAudio = false;
    unsigned long long frames = 0;
    double ttfaMs = -1;
    while (true) {
        auto ev = player->getLatestEvent();
        if (ev == Superpowered::AdvancedAudioPlayer::PlayerEvent_OpenFailed) {
            fprintf(stderr, "SPIKE-A FAIL: open error %i (%s)\n", player->getOpenErrorCode(),
                    Superpowered::AdvancedAudioPlayer::statusCodeToString(player->getOpenErrorCode()));
            return 1;
        }
        if (!playing && ev == Superpowered::AdvancedAudioPlayer::PlayerEvent_Opened) {
            player->play();
            playing = true;
        }
        player->outputSamplerate = rate;
        bool has = player->processStereo(buf, false, 1024);
        if (has) {
            frames += 1024;
            if (!gotAudio) {
                gotAudio = true;
                ttfaMs = std::chrono::duration<double, std::milli>(std::chrono::steady_clock::now() - t0).count();
            }
        }
        if (player->eofRecently()) break;
        const double elapsedS = std::chrono::duration<double>(std::chrono::steady_clock::now() - t0).count();
        if (elapsedS > 300.0) { fprintf(stderr, "SPIKE-B: 300s timeout, frames=%llu\n", frames); return 1; }
    }
    const double wallS = std::chrono::duration<double>(std::chrono::steady_clock::now() - t0).count();
    printf("SPIKE OK: ttfa_ms=%.1f wall_s=%.2f frames=%llu (track=220.2s; wall<<220 => faster-than-realtime)\n",
           ttfaMs, wallS, frames);
    delete player;
    return 0;
}
```

- [ ] **Step 4 [Codex]: Spike Makefile**

`bench/spike/Makefile` (lib/framework names corrected per Step 1):

```makefile
SDK ?= $(SUPERPOWERED_SDK_DIR)
CXX ?= clang++
CXXFLAGS = -std=c++17 -O2 -arch arm64 -I$(SDK)/Superpowered
# Step 1 fixes the exact .a name and framework list:
LIBS = $(SDK)/Superpowered/libSuperpoweredmacOS.a \
       -framework AudioToolbox -framework CoreAudio -framework CoreFoundation \
       -framework Security -framework SystemConfiguration

spike: spike.cpp
	$(CXX) $(CXXFLAGS) spike.cpp $(LIBS) -o spike
```

- [ ] **Step 5 [Claude]: Build and run the spike**

```bash
cd /Volumes/Render/dev/worktrees/kithara-bench
(cd /Users/litvinenko-pv/code/kithara && TEST_SERVER_PORT=0 cargo run --release -p kithara-integration-tests --bin test_server &) # note printed "test server listening on <base>"
cd bench/spike && SUPERPOWERED_SDK_DIR=~/code/Superpowered make
mkdir -p /tmp/sp-spike-tmp && ./spike "<base>/assets/hls/index-shq-a1.m3u8" <rate-from-SPIKE-F> /tmp/sp-spike-tmp
```

Expected: `SPIKE OK` with `wall_s` ≪ 220 (SPIKE-A pass + SPIKE-B pass). On `SPIKE-A FAIL` → record, switch Task 4 to plan B (TS remux) and re-verify; on 300 s timeout with growing frames at ~realtime → SPIKE-B fail, Task 4 uses `Superpowered::Decoder` path (retest with a 30-line decoder variant of the spike before deciding `--paced`).

- [ ] **Step 6 [Codex]: Record findings + commit**

Append `## Spike findings` (SPIKE-A…F verdicts, exact API names, lib/framework list, license-key line, fixture rate/channels) to the spec. Then:

```bash
git add bench/spike docs/superpowers/specs/2026-07-03-superpowered-bench-design.md
git commit -m "bench(spike): stage-0 superpowered facts settled"
```

---

### Task 2: kit-bench skeleton + metrics module (TDD)

**Files:**
- Create: `bench/kit-bench/Cargo.toml`
- Create: `bench/kit-bench/src/main.rs`
- Create: `bench/kit-bench/src/metrics.rs`

**Interfaces:**
- Produces: `metrics::Baseline::take()`, `metrics::Report` with `fn finish(baseline, counters) -> Report` and `fn to_json_line(&self) -> String` emitting the Global-Constraints JSON contract; consumed by Task 3 pump and Task 5 runner.

- [ ] **Step 1 [Codex]: Cargo.toml**

```toml
[package]
name = "kit-bench"
version = "0.1.0"
edition = "2024"
publish = false

[features]
default = ["symphonia"]
symphonia = ["kithara/symphonia"]
apple = ["kithara/apple"]

[dependencies]
kithara = { path = "../../crates/kithara", default-features = false, features = [
  "hls", "client-reqwest", "tls-rustls", "tokio-net", "tokio-rt-multi-thread",
] }
libc = "0.2"
tempfile = "3"

[workspace]
```

(If `cargo check` shows the facade needs an extra feature for `Audio`/decode wiring, add the minimal one and note it in the commit message; do NOT enable `file`.)

- [ ] **Step 2 [Codex]: Failing unit tests for metrics**

`bench/kit-bench/src/metrics.rs` (tests first, at the bottom of the new file; module skeleton only so the test compiles then fails):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pcm_frames_is_samples_over_channels() {
        let r = Report::example(88_200, 2, 44_100);
        assert_eq!(r.pcm_frames, 44_100);
        assert!((r.duration_secs() - 1.0).abs() < 1e-9);
    }

    #[test]
    fn json_line_has_contract_keys() {
        let line = Report::example(88_200, 2, 44_100).to_json_line();
        for key in [
            "engine", "decoder", "ttfa_ms", "wall_ms", "cpu_user_s", "cpu_sys_s",
            "cpu_total_user_s", "cpu_total_sys_s", "max_rss_bytes", "samples",
            "pcm_frames", "samplerate", "channels",
        ] {
            assert!(line.contains(&format!("\"{key}\"")), "missing {key} in {line}");
        }
        assert!(!line.contains('\n'));
    }

    #[test]
    fn baseline_delta_nonnegative() {
        let b = Baseline::take();
        let (u, s) = b.cpu_delta();
        assert!(u >= 0.0 && s >= 0.0);
    }
}
```

- [ ] **Step 3 [Codex]: Run tests, verify failure**

Run: `cd bench/kit-bench && cargo test`
Expected: FAIL — `Report`/`Baseline` not defined.

- [ ] **Step 4 [Codex]: Implement metrics.rs**

```rust
use std::time::Instant;

fn rusage_self() -> libc::rusage {
    let mut ru: libc::rusage = unsafe { std::mem::zeroed() };
    // SAFETY: RUSAGE_SELF with a zeroed out-param is the documented calling convention.
    unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut ru) };
    ru
}

fn tv_secs(tv: libc::timeval) -> f64 {
    tv.tv_sec as f64 + tv.tv_usec as f64 / 1e6
}

pub(crate) struct Baseline {
    pub(crate) t0: Instant,
    ru0_user: f64,
    ru0_sys: f64,
}

impl Baseline {
    pub(crate) fn take() -> Self {
        let ru = rusage_self();
        Self { t0: Instant::now(), ru0_user: tv_secs(ru.ru_utime), ru0_sys: tv_secs(ru.ru_stime) }
    }

    pub(crate) fn cpu_delta(&self) -> (f64, f64) {
        let ru = rusage_self();
        (tv_secs(ru.ru_utime) - self.ru0_user, tv_secs(ru.ru_stime) - self.ru0_sys)
    }
}

pub(crate) struct Report {
    pub(crate) engine: &'static str,
    pub(crate) decoder: String,
    pub(crate) ttfa_ms: f64,
    pub(crate) wall_ms: f64,
    pub(crate) cpu_user_s: f64,
    pub(crate) cpu_sys_s: f64,
    pub(crate) cpu_total_user_s: f64,
    pub(crate) cpu_total_sys_s: f64,
    pub(crate) max_rss_bytes: i64,
    pub(crate) samples: u64,
    pub(crate) pcm_frames: u64,
    pub(crate) samplerate: u32,
    pub(crate) channels: u16,
}

impl Report {
    pub(crate) fn finish(
        b: &Baseline, decoder: String, ttfa_ms: f64, samples: u64, samplerate: u32, channels: u16,
    ) -> Self {
        let (du, ds) = b.cpu_delta();
        let ru = rusage_self();
        Self {
            engine: "kithara",
            decoder,
            ttfa_ms,
            wall_ms: b.t0.elapsed().as_secs_f64() * 1e3,
            cpu_user_s: du,
            cpu_sys_s: ds,
            cpu_total_user_s: tv_secs(ru.ru_utime),
            cpu_total_sys_s: tv_secs(ru.ru_stime),
            max_rss_bytes: ru.ru_maxrss, // bytes on macOS
            samples,
            pcm_frames: samples / channels.max(1) as u64,
            samplerate,
            channels,
        }
    }

    pub(crate) fn duration_secs(&self) -> f64 {
        self.pcm_frames as f64 / self.samplerate as f64
    }

    pub(crate) fn to_json_line(&self) -> String {
        format!(
            concat!(
                "{{\"engine\":\"{}\",\"decoder\":\"{}\",\"ttfa_ms\":{:.2},\"wall_ms\":{:.2},",
                "\"cpu_user_s\":{:.4},\"cpu_sys_s\":{:.4},\"cpu_total_user_s\":{:.4},",
                "\"cpu_total_sys_s\":{:.4},\"max_rss_bytes\":{},\"samples\":{},",
                "\"pcm_frames\":{},\"samplerate\":{},\"channels\":{}}}"
            ),
            self.engine, self.decoder, self.ttfa_ms, self.wall_ms, self.cpu_user_s,
            self.cpu_sys_s, self.cpu_total_user_s, self.cpu_total_sys_s, self.max_rss_bytes,
            self.samples, self.pcm_frames, self.samplerate, self.channels
        )
    }

    #[cfg(test)]
    pub(crate) fn example(samples: u64, channels: u16, samplerate: u32) -> Self {
        Self {
            engine: "kithara", decoder: "symphonia".into(), ttfa_ms: 1.0, wall_ms: 2.0,
            cpu_user_s: 0.1, cpu_sys_s: 0.1, cpu_total_user_s: 0.2, cpu_total_sys_s: 0.2,
            max_rss_bytes: 1, samples, pcm_frames: samples / channels as u64, samplerate, channels,
        }
    }
}
```

`bench/kit-bench/src/main.rs` for now:

```rust
mod metrics;

fn main() {
    eprintln!("kit-bench: pump not implemented yet (Task 3)");
    std::process::exit(2);
}
```

- [ ] **Step 5 [Codex]: Run tests, verify pass**

Run: `cd bench/kit-bench && cargo test`
Expected: 3 passed. (First build compiles the kithara stack — minutes.)

- [ ] **Step 6 [Codex]: Commit**

```bash
git add bench/kit-bench
git commit -m "bench(kit): crate skeleton + metrics module with unit tests"
```

---

### Task 3: kit-bench pump

**Files:**
- Modify: `bench/kit-bench/src/main.rs`
- Create: `bench/kit-bench/src/pump.rs`

**Interfaces:**
- Consumes: `metrics::{Baseline, Report}` from Task 2; kithara facade API (verified shapes: `HlsConfig::for_url(url).store(StoreOptions::new(dir)).cancel(token).initial_abr_mode(mode).build()`; `AudioConfig::<Hls>::for_stream(hls_config).block_on_underrun(true).decoder_backend(b).build()`; `Audio::<Stream<Hls>>::new(config).await`; `audio.read(&mut [f32]) -> Result<ReadOutcome, _>`; mirror `tests/tests/multi_instance/concurrent_hls.rs:44-68` and `tests/src/reads.rs:27-44`).
- Produces: `kit-bench <url> [--paced]` binary printing the contract JSON line; decoder name in JSON = `"symphonia"` or `"apple"` by compiled feature.

- [ ] **Step 1 [Codex]: Implement pump.rs**

```rust
use kithara::{
    assets::StoreOptions,
    audio::{Audio, AudioConfig, PcmReader, ReadOutcome},
    hls::{AbrMode, Hls, HlsConfig},
    platform::CancelToken,
    stream::Stream,
};

use crate::metrics::{Baseline, Report};

const READ_BUF_SAMPLES: usize = 4096;

#[cfg(feature = "apple")]
const DECODER: &str = "apple";
#[cfg(not(feature = "apple"))]
const DECODER: &str = "symphonia";

fn decoder_backend() -> kithara::decode::DecoderBackend {
    #[cfg(feature = "apple")]
    { kithara::decode::DecoderBackend::Apple }
    #[cfg(not(feature = "apple"))]
    { kithara::decode::DecoderBackend::Symphonia }
}

pub(crate) fn run(url: &str, paced: bool) -> Result<Report, Box<dyn std::error::Error>> {
    let temp = tempfile::TempDir::new()?; // fresh cold client cache per run
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()?;

    // Baseline immediately before open: Audio::new (open+probe+decoder) is inside the window.
    let baseline = Baseline::take();

    let hls_config = HlsConfig::for_url(url.parse()?)
        .store(StoreOptions::new(temp.path()))
        .cancel(CancelToken::never()) // CLI process is the root owner
        .initial_abr_mode(AbrMode::manual(0)) // media playlist ⇒ single variant
        .build();
    let config = AudioConfig::<Hls>::for_stream(hls_config)
        .block_on_underrun(true)
        .decoder_backend(decoder_backend())
        .build();
    let mut audio = rt.block_on(Audio::<Stream<Hls>>::new(config))?;

    let spec = audio.spec();
    let (samplerate, channels) = (spec.sample_rate, spec.channels as u16);

    let mut buf = vec![0.0f32; READ_BUF_SAMPLES];
    let mut samples: u64 = 0;
    let mut ttfa_ms: f64 = -1.0;
    loop {
        match audio.read(&mut buf)? {
            ReadOutcome::Pending { .. } => continue, // engine-aware parked read
            ReadOutcome::Frames { count, .. } => {
                if ttfa_ms < 0.0 {
                    ttfa_ms = baseline.t0.elapsed().as_secs_f64() * 1e3;
                }
                samples += count.get() as u64;
                if paced {
                    let audio_pos = samples as f64 / (samplerate as f64 * channels as f64);
                    let wall = baseline.t0.elapsed().as_secs_f64();
                    if audio_pos > wall {
                        std::thread::sleep(std::time::Duration::from_secs_f64(audio_pos - wall));
                    }
                }
            }
            ReadOutcome::Eof { .. } => break,
        }
    }
    drop(audio);
    drop(rt);
    Ok(Report::finish(&baseline, DECODER.into(), ttfa_ms, samples, samplerate, channels))
}
```

(Field names on `PcmSpec` — `sample_rate`/`channels` — and the exact `spec()` accessor: verify against `kithara::decode::PcmSpec` when compiling; adjust locally, no other deviation.)

- [ ] **Step 2 [Codex]: main.rs wiring**

```rust
mod metrics;
mod pump;

fn main() {
    let mut args = std::env::args().skip(1);
    let url = match args.next() {
        Some(u) => u,
        None => {
            eprintln!("usage: kit-bench <media-playlist-url> [--paced]");
            std::process::exit(2);
        }
    };
    let paced = args.any(|a| a == "--paced");
    match pump::run(&url, paced) {
        Ok(report) => println!("{}", report.to_json_line()),
        Err(e) => {
            eprintln!("kit-bench failed: {e}");
            std::process::exit(1);
        }
    }
}
```

- [ ] **Step 3 [Claude]: Compile both feature configs**

```bash
cd /Volumes/Render/dev/worktrees/kithara-bench/bench/kit-bench
cargo build --release                                        # symphonia
cargo build --release --no-default-features --features apple # apple
```

Expected: both compile. If apple-without-symphonia fails inside kithara features → fall back per spec: single dual-backend build + `--decoder` CLI flag + RSS caveat recorded in spec; Codex implements the flag, Claude re-verifies.

- [ ] **Step 4 [Claude]: Smoke run against the fixture server**

```bash
TEST_SERVER_PORT=0 cargo run --release -p kithara-integration-tests --bin test_server & # in main checkout; note <base>
./target/release/kit-bench "<base>/assets/hls/index-shq-a1.m3u8"
```

Expected: one JSON line; `pcm_frames/samplerate ≈ 220.2`; `wall_ms` ≪ 220_000. If `Audio` needs a master playlist (media-playlist root rejected): create `bench/fixtures/master-aac.m3u8` (copy of `assets/hls/master.m3u8` minus the FLAC variant, served via `run.sh` from `bench/fixtures/`), keep `AbrMode::manual(2)` (shq idx), record in spec.

- [ ] **Step 5 [Codex]: Commit**

```bash
git add bench/kit-bench
git commit -m "bench(kit): faster-than-realtime HLS pump with metrics"
```

---

### Task 4: sp-bench

**Files:**
- Create: `bench/sp-bench/main.cpp`
- Create: `bench/sp-bench/metrics.h`
- Create: `bench/sp-bench/Makefile`

**Interfaces:**
- Consumes: SPIKE-A…F verdicts (exact names from spec `## Spike findings`); spike Makefile recipe.
- Produces: `sp-bench <url> <samplerate> [--paced] [--engine player|decoder]` printing the same contract JSON line with `"engine":"superpowered"`, `"decoder":"superpowered"`.

- [ ] **Step 1 [Codex]: metrics.h — same measurement contract as Rust side**

```cpp
#pragma once
#include <chrono>
#include <cstdio>
#include <sys/resource.h>

struct Baseline {
    std::chrono::steady_clock::time_point t0;
    double u0, s0;
    static double tv(const timeval &t) { return t.tv_sec + t.tv_usec / 1e6; }
    static Baseline take() {
        rusage ru{}; getrusage(RUSAGE_SELF, &ru);
        return {std::chrono::steady_clock::now(), tv(ru.ru_utime), tv(ru.ru_stime)};
    }
    double elapsedMs() const {
        return std::chrono::duration<double, std::milli>(std::chrono::steady_clock::now() - t0).count();
    }
};

// Prints the shared JSON contract line (keys must match kit-bench exactly).
inline void printReport(const Baseline &b, double ttfaMs, unsigned long long samples,
                        unsigned int samplerate, unsigned int channels) {
    rusage ru{}; getrusage(RUSAGE_SELF, &ru);
    const double du = Baseline::tv(ru.ru_utime) - b.u0, ds = Baseline::tv(ru.ru_stime) - b.s0;
    printf("{\"engine\":\"superpowered\",\"decoder\":\"superpowered\","
           "\"ttfa_ms\":%.2f,\"wall_ms\":%.2f,\"cpu_user_s\":%.4f,\"cpu_sys_s\":%.4f,"
           "\"cpu_total_user_s\":%.4f,\"cpu_total_sys_s\":%.4f,\"max_rss_bytes\":%ld,"
           "\"samples\":%llu,\"pcm_frames\":%llu,\"samplerate\":%u,\"channels\":%u}\n",
           ttfaMs, b.elapsedMs(), du, ds, Baseline::tv(ru.ru_utime), Baseline::tv(ru.ru_stime),
           ru.ru_maxrss, samples, samples / channels, samplerate, channels);
}
```

- [ ] **Step 2 [Codex]: main.cpp — player path (+ decoder path only if SPIKE-B failed)**

Structure (exact SDK names per spike findings; stereo ⇒ `channels = 2`, `samples = frames * 2`):

```cpp
#include <cstring>
#include <cstdlib>
#include <string>
#include <thread>
#include "metrics.h"
#include "Superpowered.h"
#include "SuperpoweredAdvancedAudioPlayer.h"

// usage: sp-bench <url> <samplerate> [--paced] [--engine player|decoder] [--tmp <dir>]
int main(int argc, char **argv) {
    if (argc < 3) { fprintf(stderr, "usage: sp-bench <url> <rate> [--paced] [--tmp <dir>]\n"); return 2; }
    const char *url = argv[1];
    const unsigned int rate = (unsigned int)atoi(argv[2]);
    bool paced = false; const char *tmp = "/tmp/sp-bench-tmp";
    for (int i = 3; i < argc; i++) {
        if (!strcmp(argv[i], "--paced")) paced = true;
        else if (!strcmp(argv[i], "--tmp") && i + 1 < argc) tmp = argv[++i];
    }

    Superpowered::Initialize("<key from SPIKE-C>");
    Superpowered::AdvancedAudioPlayer::setTempFolder(tmp); // exact API from SPIKE-D

    auto *player = new Superpowered::AdvancedAudioPlayer(rate, 0, 0);
    player->timeStretching = false;
    player->HLSAutomaticAlternativeSwitching = false;
    player->HLSBufferingSeconds = Superpowered::AdvancedAudioPlayer::HLSDownloadRemaining; // per SPIKE

    Baseline base = Baseline::take();
    player->openHLS(url);

    float buf[1024 * 2 + 64];
    bool playing = false;
    unsigned long long frames = 0;
    double ttfaMs = -1;
    while (true) {
        auto ev = player->getLatestEvent();
        if (ev == Superpowered::AdvancedAudioPlayer::PlayerEvent_OpenFailed) {
            fprintf(stderr, "sp-bench open failed: %i (%s)\n", player->getOpenErrorCode(),
                    Superpowered::AdvancedAudioPlayer::statusCodeToString(player->getOpenErrorCode()));
            return 1;
        }
        if (!playing && ev == Superpowered::AdvancedAudioPlayer::PlayerEvent_Opened) {
            player->play();
            playing = true;
        }
        player->outputSamplerate = rate;
        if (player->processStereo(buf, false, 1024)) {
            if (ttfaMs < 0) ttfaMs = base.elapsedMs();
            frames += 1024;
            if (paced) {
                const double audioPos = (double)frames / rate;
                const double wall = base.elapsedMs() / 1e3;
                if (audioPos > wall)
                    std::this_thread::sleep_for(std::chrono::duration<double>(audioPos - wall));
            }
        }
        if (player->eofRecently()) break;
        if (base.elapsedMs() > 900'000) { fprintf(stderr, "sp-bench timeout\n"); return 1; }
    }
    printReport(base, ttfaMs, frames * 2, rate, 2);
    delete player;
    return 0;
}
```

- [ ] **Step 3 [Codex]: Makefile (copy the working spike recipe)**

```makefile
SDK ?= $(SUPERPOWERED_SDK_DIR)
CXX ?= clang++
CXXFLAGS = -std=c++17 -O2 -arch arm64 -I$(SDK)/Superpowered
LIBS = <exact .a + frameworks from SPIKE-E>

sp-bench: main.cpp metrics.h
	$(CXX) $(CXXFLAGS) main.cpp $(LIBS) -o sp-bench
```

- [ ] **Step 4 [Claude]: Build + smoke run**

```bash
cd bench/sp-bench && SUPERPOWERED_SDK_DIR=~/code/Superpowered make
mkdir -p /tmp/sp-bench-tmp
./sp-bench "<base>/assets/hls/index-shq-a1.m3u8" <rate> --tmp /tmp/sp-bench-tmp
```

Expected: one JSON line; `pcm_frames` within ±1 segment of kit-bench's (exact match asserted later by runner with tolerance 0 — if SP pads/trims edges, record the delta in spec and set the runner tolerance accordingly, documented).

- [ ] **Step 5 [Codex]: Commit**

```bash
git add bench/sp-bench
git commit -m "bench(sp): superpowered HLS pump CLI"
```

---

### Task 5: run.sh + stats + README

**Files:**
- Create: `bench/run.sh`
- Create: `bench/stats.py`
- Create: `bench/README.md`

**Interfaces:**
- Consumes: `kit-bench` (two builds), `sp-bench`, `test_server` listening line `test server listening on <base_url>` (`tests/src/test_server/native.rs:375`).
- Produces: `bench/run.sh [N] [--paced] [--url <external-url>]` → median/IQR table; exit non-zero on any failed run or equivalence mismatch.

- [ ] **Step 1 [Codex]: stats.py with self-test**

```python
#!/usr/bin/env python3
"""Aggregate bench JSON lines: median + IQR per (engine,decoder) per metric.

Usage: stats.py <results.jsonl>   # one JSON object per line
Self-test: stats.py --self-test
"""
import json
import statistics
import sys

METRICS = ["ttfa_ms", "wall_ms", "cpu_user_s", "cpu_sys_s", "max_rss_bytes"]


def aggregate(lines):
    groups = {}
    for line in lines:
        r = json.loads(line)
        groups.setdefault((r["engine"], r["decoder"]), []).append(r)
    frames = {(e, d): {x["pcm_frames"] for x in rs} for (e, d), rs in groups.items()}
    all_frames = set().union(*frames.values()) if frames else set()
    if len(all_frames) != 1:
        raise SystemExit(f"EQUIVALENCE FAIL: pcm_frames differ across runs/engines: {frames}")
    out = {}
    for key, rs in groups.items():
        out[key] = {}
        for m in METRICS:
            vals = sorted(x[m] for x in rs)
            q = statistics.quantiles(vals, n=4) if len(vals) >= 2 else [vals[0]] * 3
            out[key][m] = (statistics.median(vals), q[2] - q[0])
    return out


def self_test():
    lines = [
        json.dumps({"engine": "a", "decoder": "x", "pcm_frames": 100, "ttfa_ms": t,
                    "wall_ms": 1, "cpu_user_s": 1, "cpu_sys_s": 1, "max_rss_bytes": 1})
        for t in (10, 20, 30)
    ]
    agg = aggregate(lines)
    assert agg[("a", "x")]["ttfa_ms"][0] == 20, agg
    bad = lines + [json.dumps({"engine": "b", "decoder": "y", "pcm_frames": 99, "ttfa_ms": 1,
                               "wall_ms": 1, "cpu_user_s": 1, "cpu_sys_s": 1, "max_rss_bytes": 1})]
    try:
        aggregate(bad)
    except SystemExit:
        print("self-test OK")
        return 0
    raise AssertionError("equivalence gate did not fire")


if __name__ == "__main__":
    if sys.argv[1:] == ["--self-test"]:
        sys.exit(self_test())
    with open(sys.argv[1]) as f:
        agg = aggregate([ln for ln in f if ln.strip()])
    for (e, d), ms in sorted(agg.items()):
        row = "  ".join(f"{m}={med:.2f}±{iqr:.2f}" for m, (med, iqr) in ms.items())
        print(f"{e:12s} {d:10s} {row}")
```

- [ ] **Step 2 [Codex]: Run self-test**

Run: `python3 bench/stats.py --self-test`
Expected: `self-test OK`.

- [ ] **Step 3 [Codex]: run.sh**

```bash
#!/usr/bin/env bash
set -euo pipefail
# bench/run.sh [N] [--paced] [--url <external-url>]
cd "$(dirname "$0")/.."   # repo root of the bench worktree
N=5; PACED=""; EXT_URL=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --paced) PACED="--paced"; shift ;;
    --url) EXT_URL="$2"; shift 2 ;;
    *) N="$1"; shift ;;
  esac
done
RATE="${BENCH_RATE:?set BENCH_RATE to the fixture sample rate from SPIKE-F}"
BENCH_TMPDIR="$(mktemp -d /tmp/kithara-bench.XXXXXX)"

echo "== build phase (unmeasured) =="
( cd bench/kit-bench && cargo build --release )
( cd bench/kit-bench && cargo build --release --no-default-features --features apple \
    --target-dir target-apple )
( cd bench/sp-bench && make )
cargo build --release -p kithara-integration-tests --bin test_server

echo "== preflight =="
{
  echo "date: $(date -u +%FT%TZ)"; sw_vers; sysctl -n machdep.cpu.brand_string
  rustc --version; clang++ --version | head -1
  echo "git: $(git rev-parse --short HEAD)"; echo "BENCH_TMPDIR: $BENCH_TMPDIR"
  echo "profiles: kit=release sp=-O2 server=release"
  [[ -n "${RUSTFLAGS:-}" ]] && echo "WARN: RUSTFLAGS=$RUSTFLAGS"
  [[ -n "${CXXFLAGS:-}" ]] && echo "WARN: CXXFLAGS=$CXXFLAGS"
  [[ -n "${MallocStackLogging:-}${MallocScribble:-}" ]] && echo "WARN: Malloc debug env set"
  uptime
} | tee "$BENCH_TMPDIR/preflight.txt"

echo "== server =="
TEST_SERVER_PORT="${TEST_SERVER_PORT:-0}" ./target/release/test_server > "$BENCH_TMPDIR/server.log" 2>&1 &
SERVER_PID=$!
trap 'kill "$SERVER_PID" 2>/dev/null || true; rm -rf "$BENCH_TMPDIR"' EXIT
for _ in $(seq 1 100); do
  BASE=$(sed -n 's/^test server listening on //p' "$BENCH_TMPDIR/server.log" | head -1)
  [[ -n "$BASE" ]] && curl -fsS "$BASE/health" >/dev/null 2>&1 && break
  sleep 0.1
done
[[ -n "$BASE" ]] || { echo "server did not start"; exit 1; }
URL="${EXT_URL:-$BASE/assets/hls/index-shq-a1.m3u8}"
echo "url: $URL"

KIT_SYM=bench/kit-bench/target/release/kit-bench
KIT_APL=bench/kit-bench/target-apple/release/kit-bench
SP=bench/sp-bench/sp-bench

run_one() { # $1=tag $2...=cmd
  local tag="$1"; shift
  local tmp; tmp=$(mktemp -d "$BENCH_TMPDIR/run.XXXXXX")
  local out
  if ! out=$(SP_TMP="$tmp" "$@" 2>>"$BENCH_TMPDIR/errors.log"); then
    echo "RUN FAILED: $tag (see $BENCH_TMPDIR/errors.log)"; exit 1
  fi
  echo "$out" >> "$BENCH_TMPDIR/results.jsonl"
  echo "  $tag: $out"
  rm -rf "$tmp"
}

echo "== warm-up (unmeasured) =="
TMPD=$(mktemp -d "$BENCH_TMPDIR/warm.XXXXXX")
"$SP" "$URL" "$RATE" $PACED --tmp "$TMPD/sp" >/dev/null
"$KIT_SYM" "$URL" $PACED >/dev/null
"$KIT_APL" "$URL" $PACED >/dev/null
rm -rf "$TMPD"

echo "== measured: $N reps, rotated order =="
for i in $(seq 1 "$N"); do
  case $((i % 3)) in
    1) ORDER=(sp sym apl) ;;
    2) ORDER=(sym apl sp) ;;
    0) ORDER=(apl sp sym) ;;
  esac
  for side in "${ORDER[@]}"; do
    case $side in
      sp)  run_one "sp[$i]"  "$SP" "$URL" "$RATE" $PACED --tmp "$BENCH_TMPDIR/sp-$i" ;;
      sym) run_one "sym[$i]" "$KIT_SYM" "$URL" $PACED ;;
      apl) run_one "apl[$i]" "$KIT_APL" "$URL" $PACED ;;
    esac
  done
done

echo "== results =="
cat "$BENCH_TMPDIR/preflight.txt"
python3 bench/stats.py "$BENCH_TMPDIR/results.jsonl"
cp "$BENCH_TMPDIR/results.jsonl" bench/last-results.jsonl
echo "raw: bench/last-results.jsonl"
```

(kit-bench создаёт свой свежий client-cache сам через `tempfile::TempDir`; `--tmp` у sp-bench — его аналог. `bench/last-results.jsonl` добавить в `bench/.gitignore`.)

- [ ] **Step 4 [Codex]: README.md**

Short: prerequisites (`SUPERPOWERED_SDK_DIR`, `BENCH_RATE` from spike), build+run one-liner, JSON contract table (copy from spec), plan-B note, `--paced`/`--url` flags, "never push this branch".

- [ ] **Step 5 [Claude]: End-to-end validation + commit**

```bash
chmod +x bench/run.sh bench/stats.py
BENCH_RATE=<rate> bench/run.sh 2
```

Expected: full pass, table printed, exit 0. Then Codex commits:

```bash
git add bench/run.sh bench/stats.py bench/README.md bench/.gitignore
git commit -m "bench: runner with preflight, warm-up, rotation, equivalence gate"
```

---

### Task 6 [Claude]: Final benchmark runs + report

**Files:**
- Modify: `docs/superpowers/specs/2026-07-03-superpowered-bench-design.md` (append `## Results`)

- [ ] **Step 1: Two full runs, repeatability check**

```bash
BENCH_RATE=<rate> bench/run.sh 5   # run A
BENCH_RATE=<rate> bench/run.sh 5   # run B
```

Expected: both exit 0; per-config CPU medians of A vs B within ~5% (spec sanity). If not — quiesce the machine (other agents' builds!) and rerun.

- [ ] **Step 2: Optional internet control run**

```bash
BENCH_RATE=<rate> bench/run.sh 1 --url "<user-provided or Apple public AAC HLS stream matching SPIKE-A container>"
```

- [ ] **Step 3: Append `## Results` to the spec and commit**

Table of medians±IQR for sp / kit-symphonia / kit-apple × {cpu_user+sys, wall, ttfa, rss}, ratios kit/sp per metric, environment header from preflight, spike verdicts summary, honest caveats (SP decoder unknown internals, kithara streaming overhead, plan-B container note if fired). Commit; report the comparison table + interpretation to the user in chat.

---

## Self-Review (done at write time)

- **Spec coverage:** spike→T1; kit two configs→T2/T3; sp player+fallback→T4; runner/preflight/warm-up/rotation/ephemeral port/equivalence/stats→T5; final runs+5%+internet+Results→T6; plan B→T1/T4 gates; `--paced`→T3/T4 flags. Covered.
- **Placeholders:** `<key from SPIKE-C>`, `<rate>`, `<base>`, `<exact .a + frameworks from SPIKE-E>` are deliberate spike-resolved inputs recorded in the spec before Tasks 2-6 run — not TBDs; every other step has complete code/commands.
- **Type consistency:** JSON keys identical across `metrics.rs`, `metrics.h`, `stats.py` (checked key-by-key); `Report::finish` signature matches pump call; `pcm_frames = samples/channels` everywhere; `samples = frames*2` on SP side matches stereo contract.
