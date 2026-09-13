#![cfg(not(target_arch = "wasm32"))]

use ::kithara::{
    signal::{render_rate_revision, render_warp_map_revision},
    warp::{SyncAdmission, SyncIntent},
};
use kithara_test_utils::{
    kithara,
    probe::capture::{self as probe_capture, ProbeEvent},
};

use super::sync_product_matrix::{
    ProductHarness, Provider, SEQUENTIAL_SYNC, prepare_fixture_grids, prepared_sources,
};

const QUANTUM_FRAMES: usize = 32;
const RING_CHUNKS: usize = 5;
const CALLBACK_FRAMES: usize = 128;

fn target_revision(event: &ProbeEvent, warp_map: u64) -> Option<u64> {
    event
        .u64("render_revision")
        .filter(|revision| render_warp_map_revision(*revision) == warp_map)
}

async fn free_events(target_only: bool) -> (Vec<ProbeEvent>, u64, u64) {
    let recorder = probe_capture::install();
    let case = SEQUENTIAL_SYNC;
    let sources = prepared_sources(Provider::Synthetic).await;
    let mut harness = ProductHarness::new_for_block(case, &sources, 0, CALLBACK_FRAMES).await;
    prepare_fixture_grids(&mut harness, case, &sources).await;
    harness.decks[0].set_default_rate(0.75);
    if target_only {
        // The worker and consumer probes carry no TrackId. Keep the host deck
        // silent so every producer/reader admission belongs to Free's target lane.
        harness.decks[0].play();
    } else {
        for deck in &harness.decks {
            deck.play();
        }
    }
    harness.request_sync(case).await;
    harness.settle_sync_activation(case).await;
    for _ in 0..4 {
        let _ = harness.render(case, CALLBACK_FRAMES).await;
    }

    let baseline = recorder.snapshot().len();
    let baseline_underruns = harness.player_controls[0]
        .rt_metrics()
        .map_or(0, |metrics| metrics.underruns());
    let admission = harness
        .request_sync_intent(case, SyncIntent::Free)
        .await
        .remove(0);
    let SyncAdmission::Preparing { warp_map, .. } = admission else {
        panic!("Free must reserve a typed worker handoff: {admission:?}");
    };
    let warp_map = u64::from(warp_map);

    for _ in 0..16 {
        let events = recorder.snapshot();
        let post = &events[baseline..];
        if post.iter().any(|event| {
            event.probe_name() == Some("prepared_sync_acknowledged")
                && event.u64("free") == Some(1)
                && event.u64("warp_map_revision") == Some(warp_map)
        }) {
            for _ in 0..4 {
                let _ = harness.render(case, CALLBACK_FRAMES).await;
            }
            let events = recorder.snapshot();
            let post = &events[baseline..];
            let current_underruns = harness.player_controls[0]
                .rt_metrics()
                .map_or(0, |metrics| metrics.underruns());
            if current_underruns > baseline_underruns {
                let trace: Vec<_> = post
                    .iter()
                    .enumerate()
                    .filter_map(|(index, event)| {
                        matches!(
                            event.probe_name(),
                            Some(
                                "free_adoption_installed"
                                    | "free_adoption_activation"
                                    | "producer_pcm_admitted"
                                    | "pcm_reader_admitted"
                                    | "pcm_consumed"
                                    | "pcm_underrun"
                                    | "prepared_sync_acknowledged"
                                    | "render_revision_floor"
                                    | "pcm_revision_discarded"
                            )
                        )
                        .then(|| {
                            format!(
                                "{index}:{}:track={:?}:out={:?}..{:?}:frames={:?}:revision={:?}:map={:?}:source_end={:?}:available={:?}:floor={:?}",
                                event.probe_name().unwrap_or("unknown"),
                                event.u64("track_id"),
                                event.u64("output_start"),
                                event.u64("output_end"),
                                event.u64("frames"),
                                event.u64("render_revision"),
                                event.u64("warp_map_revision"),
                                event.u64("source_end"),
                                event.u64("available_frames"),
                                event.u64("revision")
                            )
                        })
                    })
                    .collect();
                eprintln!("free_handoff_underrun_trace {trace:?}");
            }
            return (
                post.to_vec(),
                warp_map,
                current_underruns.saturating_sub(baseline_underruns),
            );
        }
        let _ = harness.render(case, CALLBACK_FRAMES).await;
    }
    panic!("Free did not reach a consumption-owned acknowledgement in 16 deterministic callbacks");
}

#[kithara::test(native, tokio, multi_thread, serial, flash(false))]
async fn free_warp_to_ring_preserves_target_pcm_and_bounds_queued_reader_admissions() {
    // `target_only` keeps the second deck silent: producer and reader probes
    // are generic audio seams, so their event stream is one adopted lane here.
    let (events, warp_map, _) = free_events(true).await;
    let installed = events
        .iter()
        .position(|event| {
            event.probe_name() == Some("free_adoption_installed")
                && event.u64("warp_map") == Some(warp_map)
        })
        .expect("WarpSource installs Free through the production worker");
    let produced = events
        .iter()
        .enumerate()
        .find_map(|(index, event)| {
            (index > installed && event.probe_name() == Some("producer_pcm_admitted"))
                .then(|| target_revision(event, warp_map).map(|revision| (index, revision)))
                .flatten()
        })
        .expect("WarpRenderer output reaches the producer port");
    let admitted = events
        .iter()
        .enumerate()
        .find_map(|(index, event)| {
            (index > produced.0 && event.probe_name() == Some("pcm_reader_admitted"))
                .then(|| target_revision(event, warp_map).map(|revision| (index, revision)))
                .flatten()
        })
        .expect("the target producer chunk reaches RingConsumer");
    let target_frames = events[produced.0]
        .u64("frames")
        .expect("producer records target PCM width");
    assert!(
        (1..=QUANTUM_FRAMES as u64).contains(&target_frames),
        "the target producer admission is nonempty and does not exceed Q=32"
    );
    assert_eq!(
        produced.1, admitted.1,
        "the ring preserves the target packed map/rate revision"
    );
    assert_eq!(
        events[admitted.0].u64("frames"),
        Some(target_frames),
        "the reader admits the exact producer PCM width"
    );
    assert!(
        render_rate_revision(produced.1) > 0,
        "Free carries its captured manual rate revision"
    );
    let queued_old_frames = events[produced.0 + 1..admitted.0]
        .iter()
        .filter(|event| event.probe_name() == Some("pcm_reader_admitted"))
        .filter(|event| event.u64("render_revision") != Some(produced.1))
        .map(|event| event.u64("frames").expect("reader records PCM width"))
        .sum::<u64>();
    let bound = u64::try_from((RING_CHUNKS - 1) * QUANTUM_FRAMES).expect("fixture bound fits u64");
    assert!(
        queued_old_frames <= bound,
        "old reader admissions between target producer push and target reader admission are bounded by (C - 1) * Q: {queued_old_frames} > {bound}"
    );
}

#[kithara::test(native, tokio, multi_thread, serial, flash(false))]
async fn free_handoff_default_nonblocking_has_no_post_request_underrun() {
    let (events, warp_map, underruns) = free_events(false).await;
    let track_id = events
        .iter()
        .find_map(|event| {
            (event.probe_name() == Some("free_adoption_installed")
                && event.u64("warp_map") == Some(warp_map))
            .then(|| event.u64("track"))
            .flatten()
        })
        .expect("Free identifies the adopted target track");
    let target = events
        .iter()
        .enumerate()
        .find(|(_, event)| {
            event.probe_name() == Some("pcm_consumed")
                && event.u64("track_id") == Some(track_id)
                && target_revision(event, warp_map).is_some()
        })
        .expect("the adopted target reaches RT");
    let mut contiguous_frames = 0_u64;
    let mut previous_end = None;
    let complete_index = events
        .iter()
        .enumerate()
        .skip(target.0)
        .find_map(|(index, event)| {
            if event.probe_name() != Some("pcm_consumed")
                || event.u64("track_id") != Some(track_id)
                || target_revision(event, warp_map) != target_revision(target.1, warp_map)
            {
                return None;
            }
            let (start, end) = event
                .u64("output_start")
                .zip(event.u64("output_end"))
                .expect("target output range");
            if previous_end != Some(start) {
                contiguous_frames = 0;
            }
            contiguous_frames = contiguous_frames.saturating_add(end.saturating_sub(start));
            previous_end = Some(end);
            (contiguous_frames >= CALLBACK_FRAMES as u64).then_some(index)
        })
        .expect("the adopted target presents 128 contiguous output frames");
    assert!(
        !events[..=complete_index].iter().any(|event| {
            event.probe_name() == Some("pcm_underrun") && event.u64("track_id") == Some(track_id)
        }),
        "the nonblocking adopted track must not underrun from Free request through 128 target frames"
    );
    assert_eq!(
        underruns, 0,
        "the nonblocking target deck records no post-request PCM underrun"
    );
}

#[kithara::test(native, tokio, multi_thread, serial, flash(false))]
async fn free_ring_to_rt_admits_consumes_and_acknowledges_the_same_target_pcm() {
    let (events, warp_map, _) = free_events(false).await;
    let (installed, track_id) = events
        .iter()
        .enumerate()
        .find_map(|(index, event)| {
            (event.probe_name() == Some("free_adoption_installed")
                && event.u64("warp_map") == Some(warp_map))
            .then(|| event.u64("track").map(|track_id| (index, track_id)))
            .flatten()
        })
        .expect("WarpSource installs the target Free adoption");
    let produced = events
        .iter()
        .enumerate()
        .find_map(|(index, event)| {
            (index > installed && event.probe_name() == Some("producer_pcm_admitted"))
                .then(|| target_revision(event, warp_map).map(|revision| (index, revision)))
                .flatten()
        })
        .expect("production decoder produces target PCM");
    let admitted = events
        .iter()
        .enumerate()
        .find_map(|(index, event)| {
            (index > produced.0 && event.probe_name() == Some("pcm_reader_admitted"))
                .then(|| target_revision(event, warp_map).map(|revision| (index, revision)))
                .flatten()
        })
        .expect("RingConsumer admits the produced target PCM");
    let consumed = events
        .iter()
        .enumerate()
        .find(|(index, event)| {
            *index > admitted.0
                && event.probe_name() == Some("pcm_consumed")
                && target_revision(event, warp_map) == Some(produced.1)
                && event.u64("track_id") == Some(track_id)
        })
        .expect("PlayerResource consumes the admitted target PCM");
    let render = events
        .iter()
        .enumerate()
        .find(|(index, event)| {
            if *index <= consumed.0
                || event.probe_name() != Some("render")
                || event.u64("track_id") != Some(track_id)
            {
                return false;
            }
            let Some((render_start, render_end)) = event
                .u64("output_base")
                .zip(event.u64("range_start"))
                .zip(event.u64("range_end"))
                .and_then(|((base, start), end)| {
                    base.checked_add(start).zip(base.checked_add(end))
                })
            else {
                return false;
            };
            consumed
                .1
                .u64("output_start")
                .zip(consumed.1.u64("output_end"))
                .is_some_and(|(start, end)| render_start <= start && end <= render_end)
        })
        .expect("the owning track render callback contains the first target PCM span");
    let acknowledged = events
        .iter()
        .enumerate()
        .find(|(index, event)| {
            *index > render.0
                && event.probe_name() == Some("prepared_sync_acknowledged")
                && event.u64("free") == Some(1)
                && event.u64("warp_map_revision") == Some(warp_map)
        })
        .expect("acknowledgement follows actual RT consumption, not the producer cursor");
    let handoff = events
        .iter()
        .enumerate()
        .find(|(_, event)| {
            event.probe_name() == Some("free_adoption_activation")
                && event.u64("warp_map") == Some(warp_map)
        })
        .expect("the worker records the Free handoff output origin");
    assert!(
        produced.0 < admitted.0
            && admitted.0 < consumed.0
            && consumed.0 < render.0
            && render.0 < acknowledged.0
    );
    assert_eq!(produced.1, admitted.1);
    assert_eq!(Some(produced.1), target_revision(consumed.1, warp_map));
    assert!(
        !events[produced.0 + 1..consumed.0].iter().any(|event| {
            event.probe_name() == Some("pcm_revision_discarded")
                && event.u64("revision") == Some(produced.1)
        }),
        "the target revision is never discarded before RT presents it"
    );
    let consumed_frames = consumed
        .1
        .u64("output_end")
        .zip(consumed.1.u64("output_start"))
        .map(|(end, start)| end.saturating_sub(start))
        .expect("RT probe records the presented output range");
    assert!(
        consumed_frames <= CALLBACK_FRAMES as u64,
        "RT consumes target PCM in its B=128 callback"
    );
    let target_frames_at_ack = events[consumed.0..acknowledged.0]
        .iter()
        .filter(|event| {
            event.probe_name() == Some("pcm_consumed")
                && target_revision(event, warp_map) == Some(produced.1)
                && event.u64("track_id") == Some(track_id)
        })
        .map(|event| {
            event
                .u64("output_end")
                .zip(event.u64("output_start"))
                .map(|(end, start)| end.saturating_sub(start))
                .expect("target output range")
        })
        .sum::<u64>();
    let mut contiguous_frames = 0_u64;
    let mut previous_end = None;
    let complete_index = events
        .iter()
        .enumerate()
        .skip(consumed.0)
        .find_map(|(index, event)| {
            if event.probe_name() != Some("pcm_consumed")
                || target_revision(event, warp_map) != Some(produced.1)
            {
                return None;
            }
            let (start, end) = event
                .u64("output_start")
                .zip(event.u64("output_end"))
                .expect("target output range");
            if previous_end != Some(start) {
                contiguous_frames = 0;
            }
            contiguous_frames = contiguous_frames.saturating_add(end.saturating_sub(start));
            previous_end = Some(end);
            (contiguous_frames >= CALLBACK_FRAMES as u64).then_some(index)
        })
        .expect("four deterministic callbacks present 128 contiguous target frames");
    let full_target_end = events[complete_index]
        .u64("output_end")
        .expect("full target completion has an output end");
    let admitted_source_frames = events[admitted.0..=complete_index]
        .iter()
        .filter(|event| {
            event.probe_name() == Some("pcm_reader_admitted")
                && target_revision(event, warp_map) == Some(produced.1)
        })
        .map(|event| {
            event
                .u64("source_end")
                .zip(event.u64("source_start"))
                .map(|(end, start)| end.saturating_sub(start))
                .expect("reader admission source range")
        })
        .sum::<u64>();
    assert!(
        target_frames_at_ack > 0,
        "Free acknowledgement follows a real target presentation, not producer progress"
    );
    let activation_output = handoff
        .1
        .u64("output")
        .expect("Free handoff records its output origin");
    assert!(
        full_target_end.saturating_sub(activation_output)
            <= u64::try_from(QUANTUM_FRAMES + CALLBACK_FRAMES + (CALLBACK_FRAMES - 1))
                .expect("fixture bound fits u64"),
        "full target completion must remain within Q + B + (B - 1): origin={activation_output}, full_end={full_target_end}, target_before_ack={target_frames_at_ack}, contiguous_target={contiguous_frames}, reader_admitted_source={admitted_source_frames}, indices producer={} reader={} consumed={} render={} ack={} full_target={}",
        produced.0,
        admitted.0,
        consumed.0,
        render.0,
        acknowledged.0,
        complete_index
    );
}
