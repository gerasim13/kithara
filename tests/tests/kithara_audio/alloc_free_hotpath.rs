use std::num::{NonZeroU32, NonZeroUsize};

use assert_no_alloc::*;
use kithara::{
    self,
    bufpool::SamplePool,
    resampler::{
        Resampler, ResamplerConfig, ResamplerMode, ResamplerOptions, ResamplerQuality,
        ResamplerSettings, create_resampler, rubato::RubatoBackend,
    },
    signal::{AudioChunk, AudioChunkInfo, AudioSpec},
    warp::{StretchControls, StretchKind, Warp, WarpConfig, WarpRenderer},
};

#[cfg(debug_assertions)]
#[global_allocator]
static A: AllocDisabler = AllocDisabler;

fn make_pool() -> SamplePool {
    SamplePool::new(128, 200_000)
}

fn warp_renderer(
    controls: kithara::platform::sync::Arc<StretchControls>,
    spec: AudioSpec,
    pool: SamplePool,
) -> WarpRenderer {
    let config = WarpConfig::builder().stretch(controls).build();
    Warp::new((), &config).renderer(spec, pool)
}

fn make_chunk(pool: &SamplePool, frames: usize, channels: u16) -> AudioChunk {
    make_chunk_at(pool, frames, channels, 44100)
}

fn make_chunk_at(pool: &SamplePool, frames: usize, channels: u16, sample_rate: u32) -> AudioChunk {
    let samples = frames * channels as usize;
    let mut pcm = pool.get_with(|v| v.resize(samples, 0.0));
    for (i, s) in pcm.iter_mut().enumerate() {
        #[expect(
            clippy::cast_precision_loss,
            reason = "test data, precision irrelevant"
        )]
        let val = (i as f32) * 0.001;
        *s = val;
    }
    let meta = AudioChunkInfo {
        spec: AudioSpec::new(channels, NonZeroU32::new(sample_rate).expect("test rate")),
        ..Default::default()
    };
    AudioChunk::new(meta, pcm)
}

#[kithara::test]
fn test_pool_get_put_allocation_free() {
    let pool = make_pool();

    permit_alloc(|| {
        pool.pre_warm(16, |v| v.resize(4096, 0.0));
        for _ in 0..20 {
            let _buf = pool.get();
        }
    });

    assert_no_alloc(|| {
        for _ in 0..10 {
            let _buf = pool.get();
        }
    });
}

#[kithara::test]
fn test_pcm_chunk_access_allocation_free() {
    let pool = make_pool();

    let chunk = permit_alloc(|| {
        pool.pre_warm(16, |v| v.resize(4096, 0.0));
        make_chunk(&pool, 1024, 2)
    });

    assert_no_alloc(|| {
        let _samples: &[f32] = &chunk.samples;
        let _frames = chunk.frames();
        let _spec = chunk.spec();
        if !chunk.samples.is_empty() {
            let _ = chunk.samples[0];
        }
    });

    permit_alloc(|| drop(chunk));
}

fn build_resampler(pool: &SamplePool, source_rate: u32, target_rate: u32) -> impl Resampler {
    let settings = ResamplerSettings::builder()
        .channels(NonZeroUsize::new(2).unwrap_or_else(|| panic!("test channels")))
        .mode(ResamplerMode::FixedRatio {
            source_sample_rate: NonZeroU32::new(source_rate)
                .unwrap_or_else(|| panic!("test source rate")),
            target_sample_rate: NonZeroU32::new(target_rate)
                .unwrap_or_else(|| panic!("test target rate")),
        })
        .quality(ResamplerQuality::High)
        .options(ResamplerOptions::builder().chunk_size(4_096).build())
        .sample_pool(pool.clone())
        .build();
    let config = ResamplerConfig::builder()
        .backend(RubatoBackend::new())
        .settings(settings)
        .build();
    create_resampler(&config).unwrap_or_else(|err| panic!("resampler should build: {err}"))
}

fn planar_block(pool: &SamplePool, frames: usize) -> [kithara::bufpool::SampleBuffer; 2] {
    let mut left = pool.get();
    let mut right = pool.get();
    left.ensure_len(frames)
        .unwrap_or_else(|err| panic!("left channel buffer should fit: {err}"));
    right
        .ensure_len(frames)
        .unwrap_or_else(|err| panic!("right channel buffer should fit: {err}"));
    for frame in 0..frames {
        #[expect(
            clippy::cast_precision_loss,
            reason = "test data, precision irrelevant"
        )]
        let phase = frame as f32 * 0.001;
        left[frame] = phase.sin();
        right[frame] = phase.cos();
    }
    [left, right]
}

fn output_block(pool: &SamplePool, frames: usize) -> [kithara::bufpool::SampleBuffer; 2] {
    let mut left = pool.get();
    let mut right = pool.get();
    left.ensure_len(frames)
        .unwrap_or_else(|err| panic!("left output buffer should fit: {err}"));
    right
        .ensure_len(frames)
        .unwrap_or_else(|err| panic!("right output buffer should fit: {err}"));
    [left, right]
}

fn process_planar(
    resampler: &mut dyn Resampler,
    input: &[kithara::bufpool::SampleBuffer; 2],
    output: &mut [kithara::bufpool::SampleBuffer; 2],
) -> usize {
    let input_refs = [&input[0][..], &input[1][..]];
    let (left, right) = output.split_at_mut(1);
    let mut output_refs = [&mut left[0][..], &mut right[0][..]];
    resampler
        .process_into_buffer(&input_refs, &mut output_refs)
        .unwrap_or_else(|err| panic!("resampler process should succeed: {err}"))
        .output_frames
}

/// Active fixed-ratio resampler construction pre-allocates its scratch, so the
/// first process call after construction is allocation-free.
#[kithara::test]
fn resampler_active_first_chunk_alloc_free() {
    let pool = make_pool();

    let (mut resampler, input, mut output) = permit_alloc(|| {
        pool.pre_warm(64, |v| v.resize(16384, 0.0));
        let resampler = build_resampler(&pool, 48_000, 44_100);
        let input = planar_block(&pool, 4_096);
        let output = output_block(&pool, resampler.output_frames_next());
        (resampler, input, output)
    });

    assert_no_alloc(|| {
        let frames = process_planar(&mut resampler, &input, &mut output);
        assert!(frames > 0);
    });
}

/// Active fixed-ratio resampler stays allocation-free after warmup.
#[kithara::test]
fn resampler_active_steady_state_alloc_free() {
    let pool = make_pool();

    let (mut resampler, input, mut output) = permit_alloc(|| {
        pool.pre_warm(64, |v| v.resize(16384, 0.0));
        let mut resampler = build_resampler(&pool, 48_000, 44_100);
        for _ in 0..16 {
            let warm = planar_block(&pool, 4_096);
            let mut warm_output = output_block(&pool, resampler.output_frames_next());
            let _ = process_planar(&mut resampler, &warm, &mut warm_output);
        }
        let input = planar_block(&pool, 4_096);
        let output = output_block(&pool, resampler.output_frames_next());
        (resampler, input, output)
    });

    assert_no_alloc(|| {
        let frames = process_planar(&mut resampler, &input, &mut output);
        assert!(frames > 0);
    });
}

/// Pre-sizing scratch only changes capacity; two independent resamplers fed
/// the same input must emit byte-identical output.
#[kithara::test]
fn resampler_presize_keeps_output_bit_exact() {
    let pool = make_pool();
    pool.pre_warm(64, |v| v.resize(16384, 0.0));

    let render = || -> Vec<f32> {
        let mut resampler = build_resampler(&pool, 48_000, 44_100);
        let mut out = Vec::new();
        for n in 0..12 {
            let mut input = planar_block(&pool, 4_096);
            for (i, s) in input[0].iter_mut().enumerate() {
                #[expect(
                    clippy::cast_precision_loss,
                    reason = "test waveform, precision irrelevant"
                )]
                let v = ((n * 4096 + i) as f32 * 0.0007).sin();
                *s = v;
            }
            let mut output = output_block(&pool, resampler.output_frames_next());
            let frames = process_planar(&mut resampler, &input, &mut output);
            out.extend_from_slice(&output[0][..frames]);
            out.extend_from_slice(&output[1][..frames]);
        }
        out
    };

    let a = render();
    let b = render();
    assert_eq!(a, b, "resampler output must be deterministic and bit-exact");
    assert!(!a.is_empty(), "active resampler must emit output");
}

#[kithara::test]
fn test_resampler_passthrough_allocation_free() {
    let pool = make_pool();

    let (mut resampler, input, mut output) = permit_alloc(|| {
        pool.pre_warm(32, |v| v.resize(8192, 0.0));
        let mut resampler = build_resampler(&pool, 44_100, 44_100);
        let warmup = planar_block(&pool, 4_096);
        let mut warmup_output = output_block(&pool, resampler.output_frames_next());
        let _ = process_planar(&mut resampler, &warmup, &mut warmup_output);
        let input = planar_block(&pool, 4_096);
        let output = output_block(&pool, resampler.output_frames_next());
        (resampler, input, output)
    });

    assert_no_alloc(|| {
        let frames = process_planar(&mut resampler, &input, &mut output);
        assert!(frames > 0);
    });
}

#[kithara::test]
#[case(StretchKind::Signalsmith)]
#[cfg_attr(
    not(all(target_os = "windows", target_env = "msvc")),
    case(StretchKind::Bungee)
)]
fn timestretch_active_process_and_terminal_flush_are_allocation_free(#[case] kind: StretchKind) {
    const FRAMES: usize = 8_192;
    let pool = make_pool();
    let spec = AudioSpec::new(2, NonZeroU32::new(44_100).expect("test rate"));
    let (mut effect, first, second) = permit_alloc(|| {
        let controls = StretchControls::new(0.5);
        controls.set_keylock(true);
        controls.set_backend(kind);
        let mut effect = warp_renderer(controls, spec, pool.clone());
        effect.prepare(spec);
        let first = make_chunk(&pool, FRAMES, 2);
        let second = make_chunk(&pool, FRAMES, 2);
        (effect, first, second)
    });

    let misses = pool.stats().alloc_misses;
    let first_output = assert_no_alloc(|| {
        effect
            .render(first)
            .unwrap_or_else(|| panic!("active stretch must render"))
    });
    assert_eq!(pool.stats().alloc_misses, misses);
    permit_alloc(|| {
        effect.prepare(spec);
        drop(first_output);
    });

    let misses = pool.stats().alloc_misses;
    let second_output = assert_no_alloc(|| {
        effect
            .render(second)
            .unwrap_or_else(|| panic!("serviced stretch must render again"))
    });
    assert_eq!(pool.stats().alloc_misses, misses);
    permit_alloc(|| {
        effect.prepare(spec);
        drop(second_output);
    });

    let misses = pool.stats().alloc_misses;
    let terminal = assert_no_alloc(|| effect.flush());
    assert_eq!(pool.stats().alloc_misses, misses);
    permit_alloc(|| {
        effect.prepare(spec);
        drop(terminal);
    });
}

#[kithara::test]
#[case(StretchKind::Signalsmith)]
#[cfg_attr(
    not(all(target_os = "windows", target_env = "msvc")),
    case(StretchKind::Bungee)
)]
fn timestretch_pending_and_maximum_output_are_allocation_free(#[case] kind: StretchKind) {
    const FRAMES: usize = 8_192;
    let pool = make_pool();
    let spec = AudioSpec::new(2, NonZeroU32::new(44_100).expect("test rate"));
    let (mut maximum, input) = permit_alloc(|| {
        let controls = StretchControls::new(0.05);
        controls.set_keylock(true);
        controls.set_backend(kind);
        let mut maximum = warp_renderer(controls, spec, pool.clone());
        maximum.prepare(spec);
        let input = make_chunk(&pool, FRAMES, 2);
        (maximum, input)
    });
    let misses = pool.stats().alloc_misses;
    let maximum_output = assert_no_alloc(|| {
        maximum
            .render(input)
            .unwrap_or_else(|| panic!("maximum prepared output must render"))
    });
    assert_eq!(maximum_output.frames(), 163_840);
    assert_eq!(pool.stats().alloc_misses, misses);
    permit_alloc(|| {
        maximum.prepare(spec);
        drop(maximum_output);
    });

    let (mut pending, input) = permit_alloc(|| {
        let controls = StretchControls::new(2.0);
        controls.set_keylock(true);
        controls.set_backend(kind);
        let mut pending = warp_renderer(controls, spec, pool.clone());
        pending.prepare(spec);
        let input = make_chunk(&pool, 1, 2);
        (pending, input)
    });
    let misses = pool.stats().alloc_misses;
    assert_no_alloc(|| {
        assert!(pending.render(input).is_none());
    });
    assert_eq!(pool.stats().alloc_misses, misses);
    permit_alloc(|| pending.prepare(spec));

    let misses = pool.stats().alloc_misses;
    let terminal = assert_no_alloc(|| {
        pending
            .flush()
            .unwrap_or_else(|| panic!("pending frame plus terminal tail must render"))
    });
    assert_eq!(pool.stats().alloc_misses, misses);
    permit_alloc(|| {
        pending.prepare(spec);
        drop(terminal);
    });
}
