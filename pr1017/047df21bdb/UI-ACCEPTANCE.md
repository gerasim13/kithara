# PR1017 task 5.2 offscreen real-track acceptance

Source head used for capture: `8a7acdab111f137f90b255f6231a63b3eafbc392` plus the uncommitted test-only capture harness patch recorded below.

## Input

- Analysis JSON: [338566717eec24d307187f2e3d716c19.json](/Volumes/Render/dev/evidence/waveform-peak-envelope-fold-checkpoint/draft-delivery/full-readiness-e6df-bc4-20261125-01/integration-5da8-20261127-01/ui-acceptance-8a7/offscreen/analysis/338566717eec24d307187f2e3d716c19.json)
- Status: `complete`
- Source: 48 kHz, 13,260,432 frames, 276.259 s
- Waveform: v3, `span_frames=64`, 207,195 native buckets, four bands per side
- Beat grid: 640 beats, 165 downbeats, BPM 138.996141
- Full-zoom anchor: downbeat frame 7,704,960, position `0.581049`, zoom `0.015`
- Overview: position `0.5`, zoom `0.5`

## Captures

Full zoom, iced:

![full-zoom iced](/Volumes/Render/dev/evidence/waveform-peak-envelope-fold-checkpoint/draft-delivery/full-readiness-e6df-bc4-20261125-01/integration-5da8-20261127-01/ui-acceptance-8a7/offscreen/ready-8a7-07/full-zoom/iced/studio-single.png)

Full zoom, masonry:

![full-zoom masonry](/Volumes/Render/dev/evidence/waveform-peak-envelope-fold-checkpoint/draft-delivery/full-readiness-e6df-bc4-20261125-01/integration-5da8-20261127-01/ui-acceptance-8a7/offscreen/ready-8a7-07/full-zoom/masonry/studio-single.png)

Overview, iced:

![overview iced](/Volumes/Render/dev/evidence/waveform-peak-envelope-fold-checkpoint/draft-delivery/full-readiness-e6df-bc4-20261125-01/integration-5da8-20261127-01/ui-acceptance-8a7/offscreen/ready-8a7-07/overview/iced/studio-single.png)

Overview, masonry:

![overview masonry](/Volumes/Render/dev/evidence/waveform-peak-envelope-fold-checkpoint/draft-delivery/full-readiness-e6df-bc4-20261125-01/integration-5da8-20261127-01/ui-acceptance-8a7/offscreen/ready-8a7-07/overview/masonry/studio-single.png)

SHA256:

- full-zoom iced: `c340864ff82ecd3c30694c214df33d6bb25ce7cdae3f97a3e27b75832903f398`
- overview iced: `1561a94690eb2d89fa536826ad581cb9cabf2215b524da71efe9e31daf7927ec`

## Harness

The capture used the existing `studio_capture_writes_both_hosts` test with:

```sh
PKG_CONFIG_PATH=/opt/homebrew/opt/ffmpeg@8/lib/pkgconfig \
KITHARA_STUDIO_CAPTURE=<out> \
KITHARA_CAPTURE_ANALYSIS=<analysis.json> \
KITHARA_CAPTURE_POSITION=<position> \
KITHARA_CAPTURE_ZOOM=<zoom> \
CARGO_BUILD_JOBS=2 \
cargo test -p kithara-app --lib --no-default-features \
  --features backend-cpal,masonry,stretch-signalsmith \
  studio_capture_writes_both_hosts -- --nocapture
```

Both full-zoom and overview runs passed: `1 passed; 0 failed`.

Pool stability after the real-track draw-pool sizing fix:

- full-zoom iced: `first_misses=64 second_misses=64 first_drops=0 second_drops=0`
- full-zoom masonry: `first_misses=112 second_misses=112 first_drops=0 second_drops=0`
- overview iced: `first_misses=64 second_misses=64 first_drops=0 second_drops=0`
- overview masonry: `first_misses=112 second_misses=112 first_drops=0 second_drops=0`

The uncommitted test-only harness patch is preserved at
[capture-harness-8a7.patch](/Volumes/Render/dev/evidence/waveform-peak-envelope-fold-checkpoint/draft-delivery/full-readiness-e6df-bc4-20261125-01/integration-5da8-20261127-01/ui-acceptance-8a7/offscreen/capture-harness-8a7.patch), SHA256 `56d55dd8e5ada78a55d2ad851a5bf5c098132a2f90efcc0caa9c45ed700ff345`.
It is evidence tooling only and is not part of the PR source diff.
