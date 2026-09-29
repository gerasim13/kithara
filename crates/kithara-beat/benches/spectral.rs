#![forbid(unsafe_code)]

use std::{hint::black_box, path::Path};

use criterion::{Criterion, criterion_group, criterion_main};
use kithara_beat::{SpectralBeats, Tempo};
use kithara_test_utils::bufpool::pools;

mod consts {
    /// The rate the crate contract fixes.
    pub(super) const RATE: usize = 22_050;
    /// Length of the excerpt one analysis runs over.
    pub(super) const SECONDS: usize = 30;
}

fn excerpt() -> Vec<f32> {
    let path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/track_excerpt_mono_22050.f32le");
    let bytes = std::fs::read(&path)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
    bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|word| f32::from_le_bytes(*word))
        .take(consts::RATE * consts::SECONDS)
        .collect()
}

fn spectral_beats(c: &mut Criterion) {
    let pcm = excerpt();
    let detector = SpectralBeats::new(pools(), Tempo::default()).expect("the detector builds");
    let mut group = c.benchmark_group("spectral_beats");
    group.sample_size(10);
    group.bench_function("analyze/30s", |b| {
        b.iter(|| black_box(detector.analyze(black_box(&pcm))));
    });
    group.finish();
}

criterion_group!(benches, spectral_beats);
criterion_main!(benches);
