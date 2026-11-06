//! Fixed limiter numerical inputs and checks shared by Host and benchmark consumers.

use std::num::{NonZeroU32, NonZeroUsize};

use firewheel::{
    StreamInfo,
    clock::InstantSamples,
    dsp::{buffer::ConstSequentialBuffer, declick::DeclickValues},
    log::{RealtimeLoggerConfig, realtime_logger},
    mask::{ConnectedMask, ConstantMask, SilenceMask},
    node::{
        AudioNode, AudioNodeProcessor, ConstructProcessorContext, EmptyConfig, NUM_SCRATCH_BUFFERS,
        NodeID, ProcBuffers, ProcExtra, ProcInfo, ProcStore, StreamStatus,
    },
};
use kithara::{
    effects::{Ceiling, LimiterConfig, LookaheadMs, PeakLimiter, ReleaseMs, node::LimiterNode},
    platform::time::Duration,
};
use kithara_dsp::filter::Fir;
use kithara_fixture_media::reconstruction::reconstructed_peak;
use sha2::{Digest, Sha256};

use crate::bufpool_ext::pools;

/// Check fixed numerical inputs and sanitization/recovery through real consumers.
pub fn check() {
    let cases: [(&str, &str, &str); 3] = [
        (
            "intersample",
            include_str!("intersample.f32le.hex"),
            "fa0f255d8dc2c88f98752c6b8ed4fdc4226dc6c37e94447b975202cffa939831",
        ),
        (
            "sine",
            include_str!("sine.f32le.hex"),
            "309ec16a4a2dee3011992c2495a109bd2c1b6611f56cf499f544b33910503ae6",
        ),
        (
            "one_channel_transient",
            include_str!("one_channel_transient.f32le.hex"),
            "9d23f66ecefade5e4973e82f4c501e96f41d2a66e2a9bacb55d9763d3ca31d75",
        ),
    ];
    let cases = cases.map(|(name, source, digest)| {
        let bytes = hex::decode(source.split_ascii_whitespace().collect::<String>())
            .expect("fixed PCM hex");
        assert_eq!(bytes.len(), 2048);
        assert_eq!(hex::encode(Sha256::digest(&bytes)), digest, "{name}");
        println!("fixed numerical input: case={name} planar_sha256={digest}");
        (name, bytes)
    });
    check_dense_support_boundary();
    check_sanitization();
    for (name, bytes) in &cases {
        let samples: Vec<_> = bytes
            .as_chunks::<4>()
            .0
            .iter()
            .copied()
            .map(f32::from_le_bytes)
            .collect();
        let input = [samples[..256].to_vec(), samples[256..].to_vec()];
        for frames in [1, 7, 128, 1024] {
            numerical_case(name, &input, frames);
        }
    }
}

fn check_dense_support_boundary() {
    // Isolate dense zero-tail IEEE behavior, not a non-finite true-peak guarantee.
    let mut taps = [[0.0; 25]; 1];
    taps[0][0] = 1.0;
    for future in [0.0, f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        let mut detector = Fir::<25, 1, 1, 17>::new(taps);
        let mut history = [0.0; 17];
        history[1] = 1_048_576.0;
        let mut discarded = [0.0; 17];
        detector.begin_block(&[&mut history], 0.98);
        detector.process(0, &history, &history, &mut discarded);
        let mut raw = [0.0; 9];
        raw[1] = future;
        let mut peak = [0.0];
        detector.begin_block(&[&mut raw], 0.98);
        detector.process(0, &raw, &[0.0], &mut peak);
        let expected: f32 = if future.is_finite() { 1_048_576.0 } else { 0.0 };
        assert_eq!(peak[0].to_bits(), expected.to_bits());
    }
    println!(
        "dense support boundary: zero-tail raw nonfinite discards past sum; not a TP guarantee"
    );
}

fn check_sanitization() {
    let rate = NonZeroU32::new(44_100).expect("sanitization rate");
    let config = LimiterConfig::builder()
        .ceiling(Ceiling::try_from(0.98).expect("sanitization ceiling"))
        .release_ms(ReleaseMs::try_from(50.0).expect("sanitization release"))
        .lookahead_ms(LookaheadMs::try_from(5.0).expect("5 ms lookahead"))
        .build();
    let latency = config.latency_frames(rate);
    for poison in [
        f32::NAN,
        f32::INFINITY,
        f32::NEG_INFINITY,
        f32::MIN_POSITIVE * 0.5,
    ] {
        let mut input = [vec![0.0; 20_064], vec![0.0; 20_064]];
        input[0][32] = poison;
        input[1][32] = poison;
        input[0][64..].fill(0.25);
        input[1][64..].fill(0.125);
        for partition in [1, 128] {
            let total = (input[0].len() + latency + 64).next_multiple_of(partition);
            let owner = numerical_owner(&input, rate, config, partition, total);
            let output = numerical_callback(&input, rate, config, partition, total);
            for (actual, expected) in output.iter().zip(&owner) {
                assert_eq!(
                    actual.iter().copied().map(f32::to_bits).collect::<Vec<_>>(),
                    expected
                        .iter()
                        .copied()
                        .map(f32::to_bits)
                        .collect::<Vec<_>>()
                );
                assert!(
                    actual
                        .iter()
                        .all(|v| v.is_finite() && v.abs() <= f32::from(config.ceiling()))
                );
                assert!(actual[..64 + latency].iter().all(|v| *v == 0.0));
                assert!(actual[input[0].len() + latency..].iter().all(|v| *v == 0.0));
            }
            assert_eq!(
                output[1]
                    .iter()
                    .copied()
                    .map(f32::to_bits)
                    .collect::<Vec<_>>(),
                output[0]
                    .iter()
                    .map(|v| (v * 0.5).to_bits())
                    .collect::<Vec<_>>()
            );
            let quiet = &output[0][64 + latency..20_064 + latency];
            if poison.is_infinite() && partition == 128 {
                assert!(quiet[0] < 0.25);
            } else {
                assert_eq!(quiet[0].to_bits(), 0.25_f32.to_bits());
            }
            assert!(quiet.windows(2).all(|pair| pair[1] >= pair[0]));
            let final_gain = quiet.last().copied().expect("quiet recovery") / 0.25;
            assert!(final_gain > 0.99 && final_gain <= 1.0);
            println!(
                "sanitization recovery: poison_bits={} partition={partition} L={latency} quiet_frames=20000 final_gain={final_gain}",
                poison.to_bits()
            );
        }
    }
}

fn highest_rate() -> NonZeroU32 {
    NonZeroU32::new(768_000).expect("Host rate bound")
}

fn info(frames: usize, rate: NonZeroU32) -> ProcInfo {
    ProcInfo {
        sample_rate: rate,
        frames,
        in_silence_mask: SilenceMask::default(),
        out_silence_mask: SilenceMask::default(),
        in_constant_mask: ConstantMask::default(),
        out_constant_mask: ConstantMask::default(),
        in_connected_mask: ConnectedMask::default(),
        out_connected_mask: ConnectedMask::default(),
        total_cpu_seconds_recip: 1.0,
        process_to_playback_delay: None,
        did_just_unbypass: false,
        last_marker_instant: InstantSamples(0),
        sample_rate_recip: f64::from(rate.get()).recip(),
        clock_samples: InstantSamples(0),
        duration_since_stream_start: Duration::ZERO,
        stream_status: StreamStatus::empty(),
        dropped_frames: 0,
    }
}

fn numerical_case(name: &str, input: &[Vec<f32>; 2], frames: usize) {
    let rate = NonZeroU32::new(44_100).expect("numerical rate");
    let config = LimiterConfig::builder()
        .ceiling(Ceiling::try_from(0.98).expect("fixed ceiling"))
        .release_ms(ReleaseMs::try_from(50.0).expect("fixed release"))
        .lookahead_ms(LookaheadMs::try_from(5.0).expect("5 ms lookahead"))
        .build();
    let latency = config.latency_frames(rate);
    assert_eq!(latency, 221);
    // Two oracle half-widths of real silence close both the drain and omitted tail support.
    let total = (input[0].len() + latency + 64).next_multiple_of(frames);
    let expected = numerical_owner(input, rate, config, frames, total);
    let zero_config = LimiterConfig::builder()
        .ceiling(config.ceiling())
        .release_ms(config.release_ms())
        .build();
    let direct = numerical_owner(input, rate, zero_config, frames, total);
    let heard = numerical_callback(input, rate, config, frames, total);
    for (actual, expected) in heard.iter().zip(&expected) {
        assert_eq!(
            actual.iter().copied().map(f32::to_bits).collect::<Vec<_>>(),
            expected
                .iter()
                .copied()
                .map(f32::to_bits)
                .collect::<Vec<_>>()
        );
    }
    check_numerical(name, input, &heard, &direct, latency, frames, config);
}

fn numerical_owner(
    input: &[Vec<f32>; 2],
    rate: NonZeroU32,
    config: LimiterConfig,
    frames: usize,
    total: usize,
) -> [Vec<f32>; 2] {
    let mut limiter = PeakLimiter::new(
        rate,
        highest_rate(),
        NonZeroUsize::new(2).expect("stereo"),
        config,
        &pools(),
    )
    .expect("numerical limiter");
    let mut rendered = input.clone();
    for plane in &mut rendered {
        plane.resize(total, 0.0);
    }
    let [left, right] = &mut rendered;
    for (left, right) in left.chunks_mut(frames).zip(right.chunks_mut(frames)) {
        limiter.process_planar(&mut [left, right]);
    }
    rendered
}

fn numerical_callback(
    input: &[Vec<f32>; 2],
    rate: NonZeroU32,
    config: LimiterConfig,
    frames: usize,
    total: usize,
) -> [Vec<f32>; 2] {
    let stream = StreamInfo {
        sample_rate: rate,
        sample_rate_recip: f64::from(rate.get()).recip(),
        prev_sample_rate: rate,
        max_block_frames: NonZeroU32::new(u32::try_from(frames).expect("block frames"))
            .expect("nonempty block"),
        ..StreamInfo::default()
    };
    let node = LimiterNode::new(config, highest_rate(), pools());
    let mut custom_state = None;
    let mut processor = node
        .construct_processor(
            &EmptyConfig,
            ConstructProcessorContext::new(NodeID::DANGLING, &stream, &mut custom_state),
        )
        .expect("numerical callback");
    let (logger, _receiver) = realtime_logger(RealtimeLoggerConfig::default());
    let mut extra = ProcExtra {
        logger,
        store: ProcStore::with_capacity(0),
        scratch_buffers: ConstSequentialBuffer::<f32, NUM_SCRATCH_BUFFERS>::new(frames),
        declick_values: DeclickValues::new(NonZeroU32::new(16).expect("declick frames")),
    };
    let mut padded = input.clone();
    for plane in &mut padded {
        plane.resize(total, 0.0);
    }
    let mut heard = [vec![0.0; total], vec![0.0; total]];
    let mut info = info(frames, rate);
    for start in (0..total).step_by(frames) {
        let end = total.min(start + frames);
        info.frames = end - start;
        info.clock_samples = InstantSamples(i64::try_from(start).expect("numerical clock"));
        info.in_silence_mask = if padded
            .iter()
            .all(|plane| plane[start..end].iter().all(|v| *v == 0.0))
        {
            SilenceMask::new_all_silent(2)
        } else {
            SilenceMask::default()
        };
        let [left, right] = &mut heard;
        processor.process(
            &info,
            ProcBuffers {
                inputs: &[&padded[0][start..end], &padded[1][start..end]],
                outputs: &mut [&mut left[start..end], &mut right[start..end]],
            },
            &mut extra,
        );
    }
    heard
}

fn check_numerical(
    name: &str,
    input: &[Vec<f32>; 2],
    output: &[Vec<f32>; 2],
    direct: &[Vec<f32>; 2],
    latency: usize,
    partition: usize,
    config: LimiterConfig,
) {
    let ceiling = f32::from(config.ceiling());
    let release_ms = f32::from(config.release_ms());
    assert_eq!(input[0].len(), 256);
    assert_eq!(input[1].len(), 256);
    println!(
        "fixed numerical waveform: case={name} partition={partition} L={latency} left={:?} right={:?}",
        output[0], output[1]
    );
    let mut error = 0.0_f64;
    for (channel, plane) in output.iter().enumerate() {
        assert!(plane.len() >= 256 + latency + 64);
        assert_eq!(plane.len() % partition, 0);
        assert!(plane.iter().all(|v| v.is_finite() && v.abs() <= ceiling));
        assert!(plane.iter().any(|v| *v != 0.0));
        assert!(plane[..latency].iter().all(|v| *v == 0.0));
        assert!(plane[256 + latency..].iter().all(|v| *v == 0.0));
        for (frame, sample) in plane.iter().enumerate() {
            let sounding = frame
                .checked_sub(latency)
                .and_then(|i| input[channel].get(i))
                .is_some_and(|v| *v != 0.0);
            assert_eq!(
                *sample != 0.0,
                sounding,
                "{name}/{partition}/{channel}/{frame}"
            );
        }
        for frame in 0..256 {
            let difference = f64::from(plane[frame + latency]) - f64::from(direct[channel][frame]);
            error += difference * difference;
        }
    }
    for frame in 0..256 {
        let [left, right] = [output[0][frame + latency], output[1][frame + latency]];
        if input[0][frame].to_bits() == input[1][frame].to_bits() {
            assert_eq!(left.to_bits(), right.to_bits());
        } else {
            assert_eq!(input[0][frame].to_bits(), 4.0_f32.to_bits());
            assert_eq!(right.to_bits(), (input[1][frame] * (left * 0.25)).to_bits());
        }
    }
    let peak = output
        .iter()
        .map(|plane| reconstructed_peak(plane))
        .fold(0.0_f32, f32::max);
    assert!(peak.is_finite() && peak > 0.0);
    let over_db = 20.0 * (peak / ceiling).log10();
    let rmse = (error / 512.0).sqrt();
    println!(
        "fixed numerical result: case={name} partition={partition} rate=44100 L={latency} ceiling={ceiling} release_ms={release_ms} output_frames={} oracle_base_first=0 oracle_base_last={} phases=16 half_width=32 tail_zero_context={} over_ceiling_db={over_db:.9} aligned_zero_rmse={rmse:.17e}",
        output[0].len(),
        output[0].len() - 33,
        output[0].len() - 256 - latency
    );
    assert!(
        over_db <= 0.1,
        "fixed numerical {name}/{partition}: {over_db} dB"
    );
}
