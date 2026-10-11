#![cfg(feature = "embed-small-model")]

#[cfg(all(test, target_os = "android"))]
use kithara_test_dylib as _;

mod common;

use common::{WINDOW, f_measure, fixture, load_golden, load_pcm_fixture, report};
use kithara_beat::{BEAT_MODEL_BYTES, BeatThis, MEL_MODEL_BYTES};
#[cfg(feature = "cuda")]
use kithara_beat::{BeatBackend, BeatError};
#[cfg(feature = "cuda")]
use kithara_platform::time::Instant;
use kithara_test_utils::{bufpool::pools, kithara};

const SMALL_MIN_F: f64 = 0.99;

#[kithara::test(native, flash(false))]
fn python_parity_small_model(beat_pcm: Vec<f32>) {
    let pcm = beat_pcm;
    let bt = BeatThis::builder()
        .mel_model(MEL_MODEL_BYTES)
        .beat_model(BEAT_MODEL_BYTES)
        .pools(pools())
        .build()
        .unwrap_or_else(|e| panic!("BeatThis::builder failed: {e}"));
    let raw = bt
        .analyze(&pcm)
        .unwrap_or_else(|e| panic!("analyze failed: {e}"));

    let golden = load_golden(&fixture("golden_small.json"));

    let detected: Vec<f32> = raw.beats.iter().map(|mark| mark.at).collect();
    let detected_downbeats: Vec<f32> = raw.downbeats.iter().map(|mark| mark.at).collect();
    assert!(
        raw.beats
            .iter()
            .chain(raw.downbeats.iter())
            .all(|mark| mark.confidence > 0.0 && mark.confidence < 1.0),
        "every detected mark carries a probability, never a certainty"
    );

    let beats = f_measure(&golden.beats, &detected, WINDOW);
    let downbeats = f_measure(&golden.downbeats, &detected_downbeats, WINDOW);
    report("beats", &beats);
    report("downbeats", &downbeats);

    assert!(
        beats.f_measure >= SMALL_MIN_F,
        "beat F-measure {:.4} < {SMALL_MIN_F} @ {WINDOW}s vs golden_small.json",
        beats.f_measure
    );
    assert!(
        downbeats.f_measure >= SMALL_MIN_F,
        "downbeat F-measure {:.4} < {SMALL_MIN_F} @ {WINDOW}s vs golden_small.json",
        downbeats.f_measure
    );
}

#[kithara::fixture]
fn beat_pcm() -> Vec<f32> {
    load_pcm_fixture("beat_test_mono_22050.f32le")
}

#[cfg(feature = "cuda")]
#[kithara::test(native, flash(false))]
#[ignore = "requires native GPU ONNX Runtime and NVIDIA CUDA/cuDNN"]
fn cuda_matches_cpu_and_python_small_model(beat_pcm: Vec<f32>) {
    let cpu = BeatThis::builder()
        .mel_model(MEL_MODEL_BYTES)
        .beat_model(BEAT_MODEL_BYTES)
        .pools(pools())
        .build()
        .expect("CPU detector");
    let load_start = Instant::now();
    let cuda = BeatThis::builder()
        .mel_model(MEL_MODEL_BYTES)
        .beat_model(BEAT_MODEL_BYTES)
        .backend(BeatBackend::Cuda { device_id: 0 })
        .pools(pools())
        .build()
        .expect("CUDA detector");
    let load = load_start.elapsed();
    let start = Instant::now();
    let reference = cpu.analyze(&beat_pcm).expect("CPU analysis");
    let cpu_time = start.elapsed();
    let start = Instant::now();
    let cpu_warm = cpu.analyze(&beat_pcm).expect("warm CPU analysis");
    let cpu_warm_time = start.elapsed();
    assert_eq!(reference, cpu_warm);
    let start = Instant::now();
    let detected = cuda.analyze(&beat_pcm).expect("CUDA analysis");
    let cold = start.elapsed();
    let start = Instant::now();
    let warm = cuda.analyze(&beat_pcm).expect("warm CUDA analysis");
    let warm_time = start.elapsed();
    assert!(
        warm.beats
            .iter()
            .chain(warm.downbeats.iter())
            .all(|mark| mark.at.is_finite() && mark.confidence > 0.0 && mark.confidence < 1.0)
    );
    let golden = load_golden(&fixture("golden_small.json"));
    for (kind, cpu_marks, gpu_marks, python_marks) in [
        ("beats", reference.beats, detected.beats, golden.beats),
        (
            "downbeats",
            reference.downbeats,
            detected.downbeats,
            golden.downbeats,
        ),
    ] {
        assert!(
            gpu_marks
                .iter()
                .all(|mark| mark.at.is_finite() && mark.confidence > 0.0 && mark.confidence < 1.0)
        );
        let cpu_times: Vec<_> = cpu_marks.iter().map(|mark| mark.at).collect();
        let gpu_times: Vec<_> = gpu_marks.iter().map(|mark| mark.at).collect();
        let cpu_score = f_measure(&cpu_times, &gpu_times, WINDOW);
        let python_score = f_measure(&python_marks, &gpu_times, WINDOW);
        report(kind, &cpu_score);
        assert!(
            cpu_score.f_measure >= SMALL_MIN_F,
            "CUDA differs from CPU for {kind}"
        );
        assert!(
            python_score.f_measure >= SMALL_MIN_F,
            "CUDA differs from Python for {kind}"
        );
    }
    eprintln!(
        "complete analysis incl CPU mel: CPU cold={cpu_time:?} warm={cpu_warm_time:?} CUDA load={load:?} cold={cold:?} warm={warm_time:?}"
    );
}

#[cfg(feature = "cuda")]
#[kithara::test(native, flash(false))]
#[ignore = "requires native GPU ONNX Runtime"]
fn an_invalid_cuda_device_never_selects_cpu_inference() {
    let result = BeatThis::builder()
        .mel_model(MEL_MODEL_BYTES)
        .beat_model(BEAT_MODEL_BYTES)
        .backend(BeatBackend::Cuda {
            device_id: i32::MAX,
        })
        .pools(pools())
        .build();
    assert!(
        matches!(result, Err(BeatError::Cuda(_))),
        "invalid CUDA device must be a typed runtime failure"
    );
}
