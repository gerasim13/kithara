#![forbid(unsafe_code)]

use std::{hint::black_box, num::NonZeroU32};

use criterion::{Criterion, criterion_group, criterion_main};
use kithara::{
    audio::{BeatGrid, SessionBeat, TrackBeat, analysis::TrackAnalysis},
    events::PlaybackDirection,
    platform::time::{Duration, Instant},
    play::TrackBinding,
};

struct Consts;

impl Consts {
    const DECKS: usize = 4;
    const MEASURED_PLANS: usize = 20_000;
    const P99_BUDGET: Duration = Duration::from_micros(100);
    const SAMPLE_RATE: u32 = 48_000;
    const WARMUP_PLANS: usize = 2_000;
}

struct DeckPlan {
    binding: TrackBinding,
    end: SessionBeat,
    start: SessionBeat,
}

fn sample_rate() -> NonZeroU32 {
    NonZeroU32::new(Consts::SAMPLE_RATE).expect("bench sample rate is non-zero")
}

fn deck_plan(deck: usize) -> DeckPlan {
    const BEAT_FRAMES: u64 = 24_000;
    const BEATS_PER_SECOND: f64 = 2.0;
    const PLAN_FRAMES: u32 = 4_096;
    const START_BEAT: f64 = 64.0;

    let rate = sample_rate();
    let markers = (0..=128_u64)
        .map(|beat| beat * BEAT_FRAMES)
        .collect::<Vec<_>>();
    let downbeats = markers.iter().step_by(4).copied().collect();
    let analysis = TrackAnalysis::with_source_rate(
        Some(BeatGrid::new(120.0, markers, downbeats, Vec::new())),
        None,
        129 * BEAT_FRAMES,
        rate,
    );
    let deck_offset = f64::from(u32::try_from(deck).expect("deck index fits u32"));
    let binding = TrackBinding::new(
        &analysis,
        rate,
        SessionBeat::default(),
        TrackBeat::new(1.0 + deck_offset).expect("bench track anchor is valid"),
        PlaybackDirection::Forward,
    )
    .expect("bench binding is valid");
    let start = SessionBeat::new(START_BEAT).expect("bench start beat is valid");
    let plan_beats = f64::from(PLAN_FRAMES) * BEATS_PER_SECOND / f64::from(Consts::SAMPLE_RATE);
    let end = SessionBeat::new(START_BEAT + plan_beats).expect("bench end beat is valid");

    DeckPlan {
        binding,
        end,
        start,
    }
}

fn project_group(decks: &[DeckPlan; Consts::DECKS]) -> bool {
    decks.iter().all(|deck| {
        let binding = black_box(&deck.binding);
        let start = binding.source_frame_at(black_box(deck.start));
        let end = binding.source_frame_at(black_box(deck.end));

        matches!((start, end), (Ok(Some(start)), Ok(Some(end))) if end > start)
    })
}

fn percentile(sorted: &[Duration], pct: usize) -> Duration {
    let rank = (sorted.len() * pct).div_ceil(100);
    sorted[rank.saturating_sub(1).min(sorted.len() - 1)]
}

fn measured_p99(decks: &[DeckPlan; Consts::DECKS]) -> Duration {
    let mut durations = Vec::with_capacity(Consts::MEASURED_PLANS);
    for _ in 0..Consts::MEASURED_PLANS {
        let started = Instant::now();
        black_box(project_group(black_box(decks)));
        durations.push(started.elapsed());
    }
    durations.sort_unstable();
    let p50 = percentile(&durations, 50);
    let p99 = percentile(&durations, 99);
    let max = percentile(&durations, 100);
    eprintln!(
        "four-deck binding projection: p50={:.2} us p99={:.2} us max={:.2} us",
        p50.as_secs_f64() * 1e6,
        p99.as_secs_f64() * 1e6,
        max.as_secs_f64() * 1e6,
    );
    p99
}

fn bench_sync_plan(c: &mut Criterion) {
    let decks = std::array::from_fn(deck_plan);
    for _ in 0..Consts::WARMUP_PLANS {
        black_box(project_group(black_box(&decks)));
    }

    let p99 = measured_p99(&decks);
    assert!(
        project_group(&decks),
        "every measured deck span remains inside its binding"
    );
    assert!(
        p99 < Consts::P99_BUDGET,
        "four-deck binding projection p99 {:.2} us exceeds {:.2} us",
        p99.as_secs_f64() * 1e6,
        Consts::P99_BUDGET.as_secs_f64() * 1e6,
    );

    c.bench_function("sync/four_deck_binding_projection", |bencher| {
        bencher.iter(|| black_box(project_group(black_box(&decks))));
    });
}

criterion_group!(benches, bench_sync_plan);
criterion_main!(benches);
