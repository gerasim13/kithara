#![forbid(unsafe_code)]

use std::{f64::consts::FRAC_1_SQRT_2, hint::black_box, num::NonZeroUsize};

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use kithara_dsp::filter::{Biquad, rbj};

const SIZES: [usize; 6] = [64, 128, 256, 512, 1024, 4096];
const TWO: NonZeroUsize = NonZeroUsize::MIN.saturating_add(1);
const SIX: NonZeroUsize = NonZeroUsize::MIN.saturating_add(5);

fn kernels(c: &mut Criterion) {
    let mut group = c.benchmark_group("layout");
    for frames in SIZES {
        group.throughput(Throughput::Elements(
            u64::try_from(frames).expect("frame count fits u64"),
        ));
        let planes = vec![vec![0.25_f32; frames]; SIX.get()];
        let planar = planes.concat();
        let stride = NonZeroUsize::new(frames).expect("bench sizes are non-zero");
        let mut restored = planes.clone();
        for channels in [TWO, SIX] {
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
                        kithara_dsp::interleave_channel_major(
                            black_box(&planar[..channels.get() * frames]),
                            stride,
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

fn biquad(c: &mut Criterion) {
    const FRAMES: usize = 1_024;
    let low_pass = rbj::low_pass(48_000.0, 4_000.0, FRAC_1_SQRT_2).expect("valid low-pass");
    let mut group = c.benchmark_group("biquad");
    group.throughput(Throughput::Elements(
        u64::try_from(FRAMES).expect("frame count fits u64"),
    ));
    for channels in [1_usize, 2, 8] {
        for sections in [1_usize, 4] {
            let mut filter = Biquad::new(
                NonZeroUsize::new(channels).expect("bench channels are non-zero"),
                NonZeroUsize::new(sections).expect("bench sections are non-zero"),
            )
            .expect("filter builds");
            for section in 0..sections {
                filter.retune(section, low_pass).expect("section in range");
            }
            let mut planes = vec![vec![0.25_f32; FRAMES]; channels];
            group.bench_with_input(
                BenchmarkId::new(format!("{channels}ch"), sections),
                &sections,
                |b, _| {
                    b.iter(|| filter.process(black_box(&mut planes), 0..FRAMES).is_ok());
                },
            );
        }
    }
    group.finish();

    let mut group = c.benchmark_group("biquad_decay");
    group.throughput(Throughput::Elements(
        u64::try_from(FRAMES).expect("frame count fits u64"),
    ));
    let mut filter = Biquad::new(TWO, NonZeroUsize::MIN).expect("filter builds");
    filter.retune(0, low_pass).expect("section 0 exists");
    let burst: Vec<f32> = (0..FRAMES)
        .map(|frame| if frame < 64 { 0.5 } else { 0.0 })
        .collect();
    let template = vec![burst; TWO.get()];
    let mut planes = template.clone();
    group.bench_function("burst_into_silence_2ch", |b| {
        b.iter(|| {
            planes.clone_from(&template);
            for start in (0..FRAMES).step_by(64) {
                let _ = black_box(filter.process(&mut planes, start..start + 64));
            }
        });
    });
    group.finish();
}

criterion_group!(benches, kernels, biquad);
criterion_main!(benches);
