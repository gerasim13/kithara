use std::{array, fmt};

use cochlea_features::{Audio as ProbeAudio, ProbeOpts, SegmentOpts, probe, segment_timeline};
use kithara_integration_tests::TestServerHelper;
use num_traits::ToPrimitive;

use super::*;

#[derive(Clone, Copy, Debug)]
struct ChannelMetric {
    peak_step: f32,
    peak_step_frame: usize,
    background_step: f32,
    peak_residual: f32,
    peak_residual_frame: usize,
    background_residual: f32,
}

#[derive(Clone, Copy, Debug)]
struct ChannelComparison {
    channel: usize,
    switched: ChannelMetric,
    control: ChannelMetric,
    excess: ChannelMetric,
    step_limit: f32,
    residual_limit: f32,
}

impl fmt::Display for ChannelComparison {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "ch{} switched(step={:.6}@{}, bg={:.6}, residual={:.6}@{}, bg={:.6}) control(step={:.6}, residual={:.6}) excess(step={:.6}@{}, bg={:.6}, residual={:.6}@{}, bg={:.6}) limits(step={:.6}, residual={:.6})",
            self.channel,
            self.switched.peak_step,
            self.switched.peak_step_frame,
            self.switched.background_step,
            self.switched.peak_residual,
            self.switched.peak_residual_frame,
            self.switched.background_residual,
            self.control.peak_step,
            self.control.peak_residual,
            self.excess.peak_step,
            self.excess.peak_step_frame,
            self.excess.background_step,
            self.excess.peak_residual,
            self.excess.peak_residual_frame,
            self.excess.background_residual,
            self.step_limit,
            self.residual_limit,
        )
    }
}

#[derive(Debug)]
struct ContinuityReport {
    channels: Vec<ChannelComparison>,
    failures: Vec<String>,
}

#[derive(Clone, Copy, Debug)]
struct CochleaMetric {
    silent_buckets: usize,
    onsets: usize,
    clipped_samples: usize,
    true_peak_over_0dbtp: bool,
    leading_silence_ms: f64,
    trailing_silence_ms: f64,
}

#[derive(Debug)]
struct AnalyzedControl {
    render: ControlRender,
    cochlea: CochleaMetric,
}

fn format_channels(channels: &[ChannelComparison]) -> String {
    channels
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("; ")
}

fn percentile(values: &mut [f32], numerator: usize, denominator: usize) -> f32 {
    assert!(!values.is_empty(), "percentile input must not be empty");
    values.sort_by(f32::total_cmp);
    let index = values.len().saturating_sub(1).saturating_mul(numerator) / denominator;
    values[index]
}

fn channel_metric(samples: &[f32], channel: usize) -> ChannelMetric {
    let channels = usize::from(CHANNELS);
    let frames = samples.len() / channels;
    assert!(frames >= 3, "continuity metric needs at least three frames");
    let omega = sine_omega();
    let recurrence = 2.0 * omega.cos();
    let mut steps = Vec::with_capacity(frames.saturating_sub(1));
    let mut residuals = Vec::with_capacity(frames.saturating_sub(2));
    let mut peak_step = 0.0f32;
    let mut peak_step_frame = 0usize;
    let mut peak_residual = 0.0f32;
    let mut peak_residual_frame = 0usize;

    let sample = |frame: usize| samples[frame * channels + channel];
    for frame in 1..frames {
        let step = (sample(frame) - sample(frame - 1)).abs();
        steps.push(step);
        if step > peak_step {
            peak_step = step;
            peak_step_frame = frame;
        }
        if frame >= 2 {
            let residual =
                (sample(frame) - recurrence * sample(frame - 1) + sample(frame - 2)).abs();
            residuals.push(residual);
            if residual > peak_residual {
                peak_residual = residual;
                peak_residual_frame = frame;
            }
        }
    }

    ChannelMetric {
        peak_step,
        peak_step_frame,
        background_step: percentile(&mut steps, 999, 1_000),
        peak_residual,
        peak_residual_frame,
        background_residual: percentile(&mut residuals, 999, 1_000),
    }
}

fn time_aligned_excess_metric(switched: &[f32], control: &[f32], channel: usize) -> ChannelMetric {
    let channels = usize::from(CHANNELS);
    let frames = switched.len() / channels;
    let omega = sine_omega();
    let recurrence = 2.0 * omega.cos();
    let mut steps = Vec::with_capacity(frames.saturating_sub(1));
    let mut residuals = Vec::with_capacity(frames.saturating_sub(2));
    let mut peak_step = 0.0f32;
    let mut peak_step_frame = 0usize;
    let mut peak_residual = 0.0f32;
    let mut peak_residual_frame = 0usize;
    let switched_sample = |frame: usize| switched[frame * channels + channel];
    let control_sample = |frame: usize| control[frame * channels + channel];

    for frame in 1..frames {
        let switched_step = (switched_sample(frame) - switched_sample(frame - 1)).abs();
        let control_step = (control_sample(frame) - control_sample(frame - 1)).abs();
        let excess_step = (switched_step - control_step).max(0.0);
        steps.push(excess_step);
        if excess_step > peak_step {
            peak_step = excess_step;
            peak_step_frame = frame;
        }
        if frame >= 2 {
            let switched_residual = (switched_sample(frame)
                - recurrence * switched_sample(frame - 1)
                + switched_sample(frame - 2))
            .abs();
            let control_residual = (control_sample(frame) - recurrence * control_sample(frame - 1)
                + control_sample(frame - 2))
            .abs();
            let excess_residual = (switched_residual - control_residual).max(0.0);
            residuals.push(excess_residual);
            if excess_residual > peak_residual {
                peak_residual = excess_residual;
                peak_residual_frame = frame;
            }
        }
    }

    ChannelMetric {
        peak_step,
        peak_step_frame,
        background_step: percentile(&mut steps, 999, 1_000),
        peak_residual,
        peak_residual_frame,
        background_residual: percentile(&mut residuals, 999, 1_000),
    }
}

fn primary_continuity_report(switched: &[f32], control: &[f32]) -> ContinuityReport {
    assert_eq!(
        switched.len(),
        control.len(),
        "switch and time-aligned control renders must have equal lengths",
    );
    let mut channels = Vec::with_capacity(usize::from(CHANNELS));
    let mut failures = Vec::new();
    for channel in 0..usize::from(CHANNELS) {
        let switched_metric = channel_metric(switched, channel);
        let control_metric = channel_metric(control, channel);
        let excess = time_aligned_excess_metric(switched, control, channel);
        let step_limit = excess.background_step.mul_add(ORACLE_RATIO, ORACLE_SLACK);
        let residual_limit = excess
            .background_residual
            .mul_add(ORACLE_RATIO, ORACLE_SLACK);
        if excess.peak_step > step_limit {
            failures.push(format!(
                "channel {channel} sample-step excess {:.6} at frame {} exceeds time-aligned control limit {:.6}",
                excess.peak_step, excess.peak_step_frame, step_limit,
            ));
        }
        if excess.peak_residual > residual_limit {
            failures.push(format!(
                "channel {channel} phase-residual excess {:.6} at frame {} exceeds time-aligned control limit {:.6}",
                excess.peak_residual,
                excess.peak_residual_frame,
                residual_limit,
            ));
        }
        channels.push(ChannelComparison {
            channel,
            switched: switched_metric,
            control: control_metric,
            excess,
            step_limit,
            residual_limit,
        });
    }
    ContinuityReport { channels, failures }
}

/// The metric plus the onset times behind its count. A bare count says a switch
/// grew an onset but not where, and "where" is the whole diagnosis: at the
/// splice it is the join, spread through the tail it is the destination decoder
/// started mid-stream.
fn measure_cochlea(samples: &[f32]) -> (CochleaMetric, Vec<f64>) {
    let audio = ProbeAudio {
        samples: samples.to_vec(),
        channels: CHANNELS,
        sample_rate: SAMPLE_RATE,
    };
    let report = probe(&audio, &ProbeOpts::default());
    let opts = SegmentOpts::default().with_window_ms(COCHLEA_WINDOW_MS);
    let silent_buckets = segment_timeline(&audio, &opts)
        .segments
        .iter()
        .filter(|segment| segment.silent)
        .count();
    let metric = CochleaMetric {
        silent_buckets,
        onsets: report.onsets.count,
        clipped_samples: report.clipping.clipped_samples,
        true_peak_over_0dbtp: report.clipping.true_peak_over_0dbtp,
        leading_silence_ms: report.silence.leading_ms,
        trailing_silence_ms: report.silence.trailing_ms,
    };
    (metric, report.onsets.times_ms.clone())
}

fn cochlea_failures(
    switched: &[f32],
    source: CochleaMetric,
    destination: CochleaMetric,
) -> Vec<String> {
    let (switched, switched_onsets_ms) = measure_cochlea(switched);
    let silent_limit = source.silent_buckets.max(destination.silent_buckets);
    let onset_limit = source.onsets.max(destination.onsets);
    let clipped_limit = source.clipped_samples.max(destination.clipped_samples);
    let mut failures = Vec::new();
    if switched.silent_buckets > silent_limit {
        failures.push(format!(
            "Cochlea found extra silent buckets: switched={}, source={}, destination={}",
            switched.silent_buckets, source.silent_buckets, destination.silent_buckets,
        ));
    }
    if switched.onsets > onset_limit {
        failures.push(format!(
            "Cochlea found extra onsets: switched={}, source={}, destination={}, switched_onsets_ms={switched_onsets_ms:?}",
            switched.onsets, source.onsets, destination.onsets,
        ));
    }
    if switched.clipped_samples > clipped_limit {
        failures.push(format!(
            "Cochlea found extra clipped samples: switched={}, source={}, destination={}",
            switched.clipped_samples, source.clipped_samples, destination.clipped_samples,
        ));
    }
    if switched.true_peak_over_0dbtp
        && !source.true_peak_over_0dbtp
        && !destination.true_peak_over_0dbtp
    {
        failures.push("Cochlea found a switch-only true-peak over 0 dBTP".to_owned());
    }
    let leading_limit = source
        .leading_silence_ms
        .max(destination.leading_silence_ms);
    if switched.leading_silence_ms > leading_limit {
        failures.push(format!(
            "Cochlea found extra leading silence: switched={:.3}ms, source={:.3}ms, destination={:.3}ms",
            switched.leading_silence_ms,
            source.leading_silence_ms,
            destination.leading_silence_ms,
        ));
    }
    let trailing_limit = source
        .trailing_silence_ms
        .max(destination.trailing_silence_ms);
    if switched.trailing_silence_ms > trailing_limit {
        failures.push(format!(
            "Cochlea found extra trailing silence: switched={:.3}ms, source={:.3}ms, destination={:.3}ms",
            switched.trailing_silence_ms,
            source.trailing_silence_ms,
            destination.trailing_silence_ms,
        ));
    }
    failures
}

/// TEMPORARY diagnostic: the amplitude envelope, sample by sample.
///
/// For a pure sine `A·sin(ωn)`, `x² + ((x[n+1]-x[n-1])/(2 sin ω))²` is `A²`
/// whatever the phase. A gain event then reads directly as a dip, and its
/// depth and length say what applied it — which a residual cannot.
fn dump_envelope(samples: &[f32], channel: usize, centre: usize) -> String {
    let channels = usize::from(CHANNELS);
    let frames = samples.len() / channels;
    let omega = sine_omega();
    let two_sin = 2.0 * omega.sin();
    let sample = |frame: usize| samples[frame * channels + channel];
    let envelope = |frame: usize| {
        let derivative = (sample(frame + 1) - sample(frame - 1)) / two_sin;
        sample(frame)
            .mul_add(sample(frame), derivative * derivative)
            .sqrt()
    };

    let first = centre.saturating_sub(240).max(1);
    let last = centre.saturating_add(240).min(frames.saturating_sub(2));
    let reference = envelope(first);
    let mut out = format!(
        "  envelope ch{channel} {first}..={last}, reference {reference:.6} (every 8th frame)\n"
    );
    for at in (first..=last).step_by(8) {
        let value = envelope(at);
        let deviation = (value - reference) / reference * 100.0;
        out.push_str(&format!("    {at:>7} A={value:.6} {deviation:+.3}%\n"));
    }
    out
}

/// TEMPORARY diagnostic: where every excursion sits, and how far apart.
/// One frame number is numerology; the spacing between them names the boundary
/// (segment 22050, AAC frame 1024, render block 512, or none of those).
fn dump_excursions(samples: &[f32], channel: usize, background: f32) -> String {
    let channels = usize::from(CHANNELS);
    let frames = samples.len() / channels;
    let omega = sine_omega();
    let recurrence = 2.0 * omega.cos();
    let sample = |frame: usize| samples[frame * channels + channel];
    let threshold = background * 8.0;

    let mut hits: Vec<(usize, f32)> = Vec::new();
    for frame in 2..frames {
        let residual = (sample(frame) - recurrence * sample(frame - 1) + sample(frame - 2)).abs();
        // Only the first frame of each run: one event spans a few samples.
        if residual > threshold && hits.last().is_none_or(|(last, _)| frame > last + 8) {
            hits.push((frame, residual));
        }
    }

    let mut out = format!(
        "  excursions ch{channel} over {threshold:.6} (8x background {background:.6}): {} events\n",
        hits.len(),
    );
    let mut previous: Option<usize> = None;
    for (frame, residual) in hits.iter().take(40) {
        let gap = previous.map_or_else(String::new, |last| format!(" gap={}", frame - last));
        out.push_str(&format!("    {frame:>7} residual={residual:.6}{gap}\n"));
        previous = Some(*frame);
    }
    out
}

/// TEMPORARY diagnostic: the shape of the discontinuity, not just its size.
/// A single-sample click, a phase slip and a repeated block all read as one
/// number here; only the neighbourhood tells them apart.
fn dump_around(samples: &[f32], channel: usize, frame: usize) -> String {
    let channels = usize::from(CHANNELS);
    let frames = samples.len() / channels;
    let omega = sine_omega();
    let recurrence = 2.0 * omega.cos();
    let sample = |frame: usize| samples[frame * channels + channel];

    let first = frame.saturating_sub(12);
    let last = frame.saturating_add(12).min(frames.saturating_sub(1));
    let mut out = format!("  dump ch{channel} frames {first}..={last} (peak at {frame}):\n");
    for at in first..=last {
        let residual = if at >= 2 {
            (sample(at) - recurrence * sample(at - 1) + sample(at - 2)).abs()
        } else {
            0.0
        };
        let step = if at >= 1 {
            (sample(at) - sample(at - 1)).abs()
        } else {
            0.0
        };
        let mark = if at == frame { " <<<" } else { "" };
        out.push_str(&format!(
            "    {at:>7} x={:+.6} step={:.6} residual={:.6}{mark}\n",
            sample(at),
            step,
            residual,
        ));
    }
    out
}

fn assert_clean_control(control: &[f32], label: &str) -> (CochleaMetric, Vec<f64>) {
    let (cochlea, onsets_ms) = measure_cochlea(control);
    assert_eq!(
        cochlea.silent_buckets, 0,
        "{label} no-switch control is not a valid baseline: {} silent Cochlea buckets",
        cochlea.silent_buckets,
    );
    assert!(
        cochlea.leading_silence_ms <= COCHLEA_WINDOW_MS,
        "{label} no-switch control has {:.3}ms leading silence",
        cochlea.leading_silence_ms,
    );
    assert!(
        cochlea.trailing_silence_ms <= COCHLEA_WINDOW_MS,
        "{label} no-switch control has {:.3}ms trailing silence",
        cochlea.trailing_silence_ms,
    );

    let mut failures = Vec::new();
    let mut metrics = Vec::with_capacity(usize::from(CHANNELS));
    for channel in 0..usize::from(CHANNELS) {
        let metric = channel_metric(control, channel);
        let step_limit = metric.background_step.mul_add(ORACLE_RATIO, ORACLE_SLACK);
        let residual_limit = metric
            .background_residual
            .mul_add(ORACLE_RATIO, ORACLE_SLACK);
        if metric.peak_step > step_limit {
            failures.push(format!(
                "channel {channel} sample step {:.6} at frame {} exceeds standalone control limit {:.6}",
                metric.peak_step, metric.peak_step_frame, step_limit,
            ));
        }
        if metric.peak_residual > residual_limit {
            failures.push(format!(
                "channel {channel} phase residual {:.6} at frame {} exceeds standalone control limit {:.6}\n{}",
                metric.peak_residual,
                metric.peak_residual_frame,
                residual_limit,
                format!(
                    "{}{}",
                    dump_around(control, channel, metric.peak_residual_frame),
                    dump_excursions(control, channel, metric.background_residual)
                        + &dump_envelope(control, channel, metric.peak_residual_frame),
                ),
            ));
        }
        metrics.push(metric);
    }
    assert!(
        failures.is_empty(),
        "{label} no-switch control contains a PCM discontinuity:\n{}\nmetrics={metrics:?}",
        failures.join("\n"),
    );
    (cochlea, onsets_ms)
}

fn sine_omega() -> f32 {
    std::f32::consts::TAU * SINE_HZ.to_f32().expect("fixture frequency fits f32")
        / SAMPLE_RATE.to_f32().expect("fixture sample rate fits f32")
}

#[kithara::test]
#[case::positive_rising(0.4, 17)]
#[case::negative_falling(-0.4, 42)]
fn quality_switch_oracle_rejects_an_injected_single_sample_click(
    #[case] click_delta: f32,
    #[case] phase_offset: usize,
) {
    let frames = usize::try_from(SAMPLE_RATE)
        .expect("fixture sample rate fits usize")
        .saturating_mul(2);
    let mut control = Vec::with_capacity(frames * usize::from(CHANNELS));
    for frame in 0..frames {
        let phase = sine_omega() * frame.to_f32().expect("fixture frame index fits f32");
        let sample = 0.25 * phase.sin();
        control.extend_from_slice(&[sample, sample]);
    }
    let mut clicked = control.clone();
    let click_frame = frames / 2 + phase_offset;
    for channel in 0..usize::from(CHANNELS) {
        clicked[click_frame * usize::from(CHANNELS) + channel] += click_delta;
    }

    let report = primary_continuity_report(&clicked, &control);
    assert!(
        report.channels.iter().all(|channel| {
            channel.excess.peak_step > channel.step_limit
                || channel.excess.peak_residual > channel.residual_limit
        }),
        "the production oracle must reject a calibrated {click_delta:+.1} one-sample click on every channel: {}",
        format_channels(&report.channels),
    );
}

#[kithara::test(
    tokio,
    multi_thread,
    native,
    serial,
    timeout(Duration::from_secs(180)),
    env(KITHARA_HANG_TIMEOUT_SECS = "3")
)]
#[case::symphonia(DecoderBackend::Symphonia)]
#[cfg_attr(
    any(target_os = "macos", target_os = "ios"),
    case::apple(DecoderBackend::Apple)
)]
async fn manual_quality_switches_match_time_aligned_no_switch_pcm(#[case] backend: DecoderBackend) {
    let server = TestServerHelper::new().await;
    let created = server
        .create_hls(fixture())
        .await
        .expect("create phase-aligned quality-switch fixture");
    let master_url = created.master_url();
    let mut failures = Vec::new();

    let mut controls: [Option<AnalyzedControl>; 3] = array::from_fn(|_| None);
    for (label, variant) in [("aac-low", AAC_LOW), ("aac-high", AAC_HIGH), ("flac", FLAC)] {
        let control_label = format!("{label}-no-switch-control");
        let control = render_no_switch_control(&master_url, variant, backend, &control_label).await;
        let (cochlea, onsets_ms) = assert_clean_control(&control.samples, &control_label);
        if let Some(reference) = controls.iter().flatten().next() {
            assert_eq!(
                control.capture_frame, reference.render.capture_frame,
                "all no-switch controls must capture the same source frame: {control_label}={}, reference={}",
                control.capture_frame, reference.render.capture_frame,
            );
        }
        println!(
            "QUALITY_SWITCH_CONTROL variant={variant} backend={backend:?} capture_source_frame={} frames={} onsets_ms={onsets_ms:?}",
            control.capture_frame,
            control.samples.len() / usize::from(CHANNELS),
        );
        controls[variant] = Some(AnalyzedControl {
            render: control,
            cochlea,
        });
    }

    for transition in TRANSITIONS {
        let switched = render_switch(&master_url, transition, backend).await;
        let frames = switched.samples.len() / usize::from(CHANNELS);
        let source_control = controls[transition.from]
            .as_ref()
            .expect("source variant control must exist");
        let destination_control = controls[transition.to]
            .as_ref()
            .expect("destination variant control must exist");
        assert_eq!(
            switched.capture_frame, source_control.render.capture_frame,
            "{} switch and source control must capture the exact same source frame: switched={}, control={}",
            transition.label, switched.capture_frame, source_control.render.capture_frame,
        );

        let primary = primary_continuity_report(&switched.samples, &source_control.render.samples);
        let cochlea = cochlea_failures(
            &switched.samples,
            source_control.cochlea,
            destination_control.cochlea,
        );
        println!(
            "QUALITY_SWITCH_CONTINUITY transition={} backend={backend:?} capture_source_frame={} applied_frame={} target_decoder_frame={} frames={} channels={} cochlea_failures={cochlea:?}",
            transition.label,
            switched.capture_frame,
            switched.applied_frame,
            switched.target_decoder_frame,
            frames,
            format_channels(&primary.channels),
        );
        failures.extend(
            primary
                .failures
                .into_iter()
                .chain(cochlea)
                .map(|failure| format!("{}: {failure}", transition.label)),
        );
    }

    assert!(
        failures.is_empty(),
        "quality switches added PCM discontinuities for backend {backend:?}:\n{}",
        failures.join("\n"),
    );
}
