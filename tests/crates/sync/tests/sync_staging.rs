#![cfg(not(target_os = "android"))]
#![cfg(not(target_arch = "wasm32"))]

use std::num::NonZeroU32;

use kithara::{
    platform::time::{Duration, Instant},
    play::{PlayWorker, PlayWorkerConfig, PlayerConfig, PlayerImpl},
    signal::{SessionFrame, TransportRevision},
    sync::{AlignmentSource, LoadGeneration, SyncError, SyncGroup, SyncIntent, SyncOperation},
    warp::{AssetFrame, BeatGridId},
};
use kithara_integration_tests::{
    audio_artifact::AudioArtifactSet,
    bufpool_ext::pools,
    cochlea::{CochleaReport, continuity_failures},
    kithara, usdt_trace,
};

use super::{
    sync_listening::{render_frames, write_capture},
    sync_product_matrix::{
        BLOCK_FRAMES, CHANNELS, PreparedSources, ProductHarness, STAGED_BESIDE_PLAYBACK,
        STAGED_CUE, STAGED_UNDER_LOOSE_DEADLINE, STAGED_WITHOUT_CAPACITY, SyncCase,
        synthetic_sources,
    },
};

/// Probe the executor fires once the group owner answered a receipt.
const RECEIPT: &str = "sync_receipt_delivered";
/// Probe codes of the receipts these scenarios expect.
const INSTALLED: u64 = 0;
const CAPACITY: u64 = 3;
const CANCELLED: u64 = 4;
const RECEIPT_TIMEOUT: Duration = Duration::from_secs(30);
/// Longer than a lane's ring holds, so the sounding lane has to keep decoding.
const LISTEN_FRAMES: usize = 48_000 * 6;
/// A cue off the downbeat, well inside the fixture.
const CUE_SECONDS: f64 = 5.25;
/// A second cue that supersedes the first.
const SUPERSEDING_CUE_SECONDS: f64 = 9.625;
/// Recording rate of the synthetic fixtures the cues index.
const FIXTURE_RATE: f64 = 48_000.0;

/// One receipt the owner answered: its rejection code and whether the owner
/// recorded it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Delivered {
    rejected: u64,
    accepted: bool,
}

fn delivered() -> Vec<Delivered> {
    usdt_trace::events()
        .iter()
        .filter(|event| event.probe == RECEIPT)
        .map(|event| Delivered {
            rejected: event.field("rejected").expect("receipt rejection"),
            accepted: event.field("accepted").expect("receipt answer") == 1,
        })
        .collect()
}

/// Renders until the owner has answered a receipt with `code`.
async fn render_until(harness: &mut ProductHarness, case: SyncCase, code: u64) -> Vec<Delivered> {
    let deadline = Instant::now() + RECEIPT_TIMEOUT;
    loop {
        let receipts = delivered();
        if receipts.iter().any(|receipt| receipt.rejected == code) {
            return receipts;
        }
        assert!(
            Instant::now() < deadline,
            "{}: no receipt {code} reached the owner; delivered {receipts:?}",
            case.id()
        );
        let _ = harness.render(case, BLOCK_FRAMES).await;
    }
}

/// Syncs the deck, then asks its group to prepare the track from the exact
/// recording frame `seconds` in: a launch the executor stages beside
/// whatever the deck plays.
async fn prepare_cue(harness: &mut ProductHarness, case: SyncCase, seconds: f64) {
    // The Host publishes its committed transport as the session grid only
    // when a transport command observes it; nothing else refreshes the root.
    harness.set_tempo(case, case.start_bpm(), true).await;
    harness.request_sync_intent(case, SyncIntent::Enable).await;
    let deck = harness.decks[0].id();
    let topology = harness
        .host
        .with(|host| host.topology())
        .await
        .unwrap_or_else(|error| panic!("{}: read the session topology: {error}", case.id()));
    let target = topology
        .members()
        .iter()
        .filter(|member| member.grid().id() == deck)
        .find_map(|member| {
            member
                .group_topology()?
                .members()
                .first()
                .map(|track| track.grid().id())
        })
        .unwrap_or_else(|| panic!("{}: the deck holds no track grid", case.id()));
    let transport = harness
        .host
        .transport_revision()
        .await
        .unwrap_or_else(|error| panic!("{}: query Host transport: {error}", case.id()));
    let now = SessionFrame::new(i64::try_from(harness.host.position()).unwrap_or(i64::MAX));
    let cue = AssetFrame::new(seconds * FIXTURE_RATE).expect("fixture cue is finite");
    let _ = harness
        .host
        .with(move |host| {
            host.transact(SyncOperation::Prepare {
                target,
                load: LoadGeneration::first(),
                transport,
                source: AlignmentSource::Prepared(cue),
                window: now..SessionFrame::new(i64::MAX),
            })
        })
        .await
        .unwrap_or_else(|rejected| panic!("{}: prepare the cue: {rejected}", case.id()));
}

#[kithara::test(
    native,
    tokio,
    multi_thread,
    serial,
    flash(false),
    timeout(Duration::from_secs(60))
)]
#[case::unbounded_deadline(STAGED_CUE)]
#[case::deadline_looser_than_the_ring(STAGED_UNDER_LOOSE_DEADLINE)]
async fn a_cued_sync_installs_mapped_pcm_before_anything_sounds(
    #[case] case: SyncCase,
    #[future(awt)] synthetic_sources: PreparedSources,
) {
    let mut harness = ProductHarness::new(case, &synthetic_sources, 0).await;
    prepare_cue(&mut harness, case, CUE_SECONDS).await;

    let receipts = render_until(&mut harness, case, INSTALLED).await;
    assert_eq!(
        receipts,
        [Delivered {
            rejected: INSTALLED,
            accepted: true,
        }],
        "{}: the owner records exactly one proven lane",
        case.id()
    );
}

#[kithara::test(
    native,
    tokio,
    multi_thread,
    serial,
    flash(false),
    timeout(Duration::from_secs(60))
)]
async fn unloading_the_track_reports_its_installed_lane_cancelled(
    #[future(awt)] synthetic_sources: PreparedSources,
) {
    let case = STAGED_CUE;
    let mut harness = ProductHarness::new(case, &synthetic_sources, 0).await;
    prepare_cue(&mut harness, case, CUE_SECONDS).await;
    let _ = render_until(&mut harness, case, INSTALLED).await;

    let control = harness.decks[0].control().clone();
    harness.host.run(move || control.clear()).await;
    let receipts = render_until(&mut harness, case, CANCELLED).await;
    harness.settle(case, 8).await;

    assert_eq!(
        delivered(),
        receipts,
        "{}: a dropped lane reports once and nothing follows it",
        case.id()
    );
    assert_eq!(
        receipts.last(),
        Some(&Delivered {
            rejected: CANCELLED,
            accepted: true,
        }),
        "{}: the owner takes the cancellation of its installed preparation",
        case.id()
    );
}

#[kithara::test(
    native,
    tokio,
    multi_thread,
    serial,
    flash(false),
    timeout(Duration::from_secs(60))
)]
async fn a_lane_the_worker_cannot_hold_is_refused_for_capacity(
    #[future(awt)] synthetic_sources: PreparedSources,
) {
    let case = STAGED_WITHOUT_CAPACITY;
    let mut harness = ProductHarness::new(case, &synthetic_sources, 0).await;
    prepare_cue(&mut harness, case, CUE_SECONDS).await;

    let receipts = render_until(&mut harness, case, CAPACITY).await;
    assert!(
        receipts.iter().all(|receipt| *receipt
            == Delivered {
                rejected: CAPACITY,
                accepted: true,
            }),
        "{}: nothing is installed without a slot: {receipts:?}",
        case.id()
    );
    let sounding = render_frames(&mut harness, case, LISTEN_FRAMES).await;
    assert!(
        sounding.iter().any(|sample| sample.abs() > f32::EPSILON),
        "{}: the sounding deck keeps its slot and plays on",
        case.id()
    );
}

#[kithara::test(
    native,
    tokio,
    multi_thread,
    serial,
    flash(false),
    timeout(Duration::from_secs(120))
)]
async fn the_sounding_lane_plays_on_while_its_staged_lane_is_superseded(
    #[future(awt)] synthetic_sources: PreparedSources,
) {
    let case = STAGED_BESIDE_PLAYBACK;
    let control = {
        let mut harness = ProductHarness::new(case, &synthetic_sources, 0).await;
        render_frames(&mut harness, case, LISTEN_FRAMES).await
    };
    let mut harness = ProductHarness::new(case, &synthetic_sources, 0).await;
    prepare_cue(&mut harness, case, CUE_SECONDS).await;
    prepare_cue(&mut harness, case, SUPERSEDING_CUE_SECONDS).await;
    let candidate = render_frames(&mut harness, case, LISTEN_FRAMES).await;
    let receipts = render_until(&mut harness, case, INSTALLED).await;
    assert!(
        receipts
            .iter()
            .all(|receipt| receipt.rejected == INSTALLED && receipt.accepted),
        "{}: a superseded lane is dropped without a receipt: {receipts:?}",
        case.id()
    );

    if let Some(artifacts) = AudioArtifactSet::from_env(case.id(), case.sample_rate, CHANNELS)
        .expect("configure staging artifacts")
    {
        for (label, pcm) in [
            ("control", &control),
            ("predecessor-through-cancel", &candidate),
        ] {
            let path = write_capture(&artifacts, label, pcm);
            eprintln!("KITHARA_AUDIO_ARTIFACT {label}: {}", path.display());
        }
    }
    let failures = continuity_failures(
        case.id(),
        &CochleaReport::measure(&candidate, CHANNELS, case.sample_rate),
        &CochleaReport::measure(&control, CHANNELS, case.sample_rate),
    );
    assert!(
        failures.is_empty(),
        "{}: staging beside the sounding lane changed what it plays:\n{}",
        case.id(),
        failures.join("\n"),
    );
}

#[kithara::test(native, tokio, multi_thread, serial, flash(false))]
async fn a_player_outside_any_session_refuses_a_staged_preparation() {
    let track = BeatGridId::allocate().expect("fixture grid id");
    let mut player = PlayerImpl::new(
        PlayerConfig::builder()
            .worker(PlayWorker::new(PlayWorkerConfig::builder(pools()).build()))
            .sample_rate(NonZeroU32::new(48_000).expect("fixture rate"))
            .track_grid_id(track)
            .build(),
    );
    let refused = player
        .transact(SyncOperation::Prepare {
            target: track,
            load: LoadGeneration::first(),
            transport: TransportRevision::first(),
            source: AlignmentSource::Prepared(AssetFrame::new(4_800.0).expect("fixture cue")),
            window: SessionFrame::new(0)..SessionFrame::new(i64::MAX),
        })
        .expect_err("an unbound player has no owner to report a staged lane to");
    assert_eq!(refused.error(), &SyncError::OwnerUnavailable);
}
