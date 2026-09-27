#![forbid(unsafe_code)]

use std::{hint::black_box, num::NonZeroUsize};

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};

mod consts {
    use super::NonZeroUsize;

    pub(super) const SIZES: [usize; 6] = [64, 128, 256, 512, 1024, 4096];
    pub(super) const SIX: NonZeroUsize = NonZeroUsize::MIN.saturating_add(5);
    pub(super) const TWO: NonZeroUsize = NonZeroUsize::MIN.saturating_add(1);
}

fn kernels(c: &mut Criterion) {
    let mut group = c.benchmark_group("layout");
    for frames in consts::SIZES {
        group.throughput(Throughput::Elements(
            u64::try_from(frames).expect("frame count fits u64"),
        ));
        let planes = vec![vec![0.25_f32; frames]; consts::SIX.get()];
        let mut restored = planes.clone();
        for channels in [consts::TWO, consts::SIX] {
            let mut interleaved = vec![0.0_f32; channels.get() * frames];
            group.bench_with_input(
                BenchmarkId::new(format!("fi_interleave_{channels}ch"), frames),
                &frames,
                |b, _| {
                    b.iter(|| {
                        fast_interleave::interleave_variable(
                            black_box(&planes[..channels.get()]),
                            0..frames,
                            &mut interleaved,
                            channels,
                        );
                    });
                },
            );
            group.bench_with_input(
                BenchmarkId::new(format!("fi_deinterleave_{channels}ch"), frames),
                &frames,
                |b, _| {
                    b.iter(|| {
                        fast_interleave::deinterleave_variable(
                            black_box(&interleaved),
                            channels,
                            &mut restored,
                            0..frames,
                        );
                    });
                },
            );
            group.bench_with_input(
                BenchmarkId::new(format!("interleave_{channels}ch"), frames),
                &frames,
                |b, _| {
                    b.iter(|| {
                        kithara_dsp::interleave_variable(
                            black_box(&planes[..channels.get()]),
                            0..frames,
                            &mut interleaved,
                            channels,
                        );
                    });
                },
            );
            group.bench_with_input(
                BenchmarkId::new(format!("deinterleave_{channels}ch"), frames),
                &frames,
                |b, _| {
                    b.iter(|| {
                        kithara_dsp::deinterleave_variable(
                            black_box(&interleaved),
                            channels,
                            &mut restored,
                            0..frames,
                        );
                    });
                },
            );
        }
        let mut noisy = vec![f32::from_bits(1); frames];
        group.bench_with_input(BenchmarkId::new("sanitize", frames), &frames, |b, _| {
            b.iter(|| kithara_dsp::sanitize(black_box(&mut noisy)));
        });
    }
    group.finish();
}

criterion_group!(benches, kernels);
criterion_main!(benches);
