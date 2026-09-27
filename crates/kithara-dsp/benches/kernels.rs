#![forbid(unsafe_code)]

use std::{f64::consts::FRAC_1_SQRT_2, hint::black_box, num::NonZeroUsize};

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use kithara_dsp::{
    filter::{Biquad, Coefficients, Hertz, Type},
    interp::{Interpolation, interpolate},
};
use num_traits::ToPrimitive;

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
        let planar = planes.concat();
        let stride = NonZeroUsize::new(frames).expect("bench sizes are non-zero");
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
    let low_pass = Coefficients::from_params(
        Type::LowPass,
        Hertz::from_hz(48_000.0).expect("positive rate"),
        Hertz::from_hz(4_000.0).expect("positive cutoff"),
        FRAC_1_SQRT_2,
    )
    .expect("valid low-pass");
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
    let mut filter = Biquad::new(consts::TWO, NonZeroUsize::MIN).expect("filter builds");
    filter.retune(0, low_pass).expect("section 0 exists");
    let burst: Vec<f32> = (0..FRAMES)
        .map(|frame| if frame < 64 { 0.5 } else { 0.0 })
        .collect();
    let template = vec![burst; consts::TWO.get()];
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

fn interp(c: &mut Criterion) {
    const FRAMES: usize = 1_024;
    let window: Vec<f32> = std::iter::successors(Some(0.0_f32), |phase| Some(phase + 0.05))
        .map(f32::sin)
        .take(FRAMES + 4)
        .collect();
    let mut group = c.benchmark_group("interp");
    for ratio in [0.5_f64, 1.001, 2.0] {
        let positions: Vec<f32> =
            std::iter::successors(Some(1.0_f64), |position| Some(position + ratio))
                .take_while(|position| *position < 1_024.0)
                .filter_map(|position| position.to_f32())
                .collect();
        let mut output = vec![0.0_f32; positions.len()];
        group.throughput(Throughput::Elements(
            u64::try_from(positions.len()).expect("position count fits u64"),
        ));
        for method in [
            Interpolation::Linear,
            Interpolation::Quadratic,
            Interpolation::Hermite,
            Interpolation::Watte,
        ] {
            group.bench_with_input(
                BenchmarkId::new(format!("{method:?}"), ratio),
                &ratio,
                |b, _| {
                    b.iter(|| {
                        interpolate(
                            method,
                            black_box(&window),
                            black_box(&positions),
                            &mut output,
                        )
                        .is_ok()
                    });
                },
            );
        }
    }
    group.finish();
}

criterion_group!(benches, kernels, biquad, interp);
criterion_main!(benches);
