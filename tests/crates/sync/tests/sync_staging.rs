#![cfg(not(target_os = "android"))]
#![cfg(not(target_arch = "wasm32"))]

use kithara::{
    platform::time::{Duration, Instant},
    signal::SessionFrame,
    sync::{AlignmentSource, LoadGeneration, SyncAdmission, SyncGroup, SyncIntent, SyncOperation},
    warp::AssetFrame,
};
use kithara_integration_tests::{grid::Start, kithara, usdt_trace};

use super::{
    sync_listening::render_frames,
    sync_product_matrix::{
        Audible, BLOCK_FRAMES, CHANNELS, NEWTECHNO_PHRASE, PreparedSources, ProductHarness,
        STAGED_BESIDE_PLAYBACK, STAGED_BESIDE_PLAYBACK_CONTROL, STAGED_CUE,
        STAGED_CUE_BESIDE_A_DECK, STAGED_UNDER_LOOSE_DEADLINE, STAGED_WITHOUT_CAPACITY, SyncCase,
        TUNNEL_CUE, newtechno_sources, tunnel_sources,
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
/// A cue on the second beat of the Tunnel's fifth bar.
const TUNNEL_WEAK_CUE: Start = Start::Bar { bar: 4, beat: 1 };
/// A second Tunnel cue that supersedes the first.
const TUNNEL_SUPERSEDING_CUE: Start = Start::bar(8);
/// Newtechno's third phrase, a second cue that supersedes its second.
const NEWTECHNO_SUPERSEDING_CUE: Start = Start::Beat(128);
/// Recording rate of The Tunnel, the frame rate its cues index.
const TUNNEL_RATE: f64 = 44_100.0;
/// Recording rate of newtechno.
const NEWTECHNO_RATE: f64 = 48_000.0;

/// One receipt the owner answered: the operation it answers for, its
/// rejection code and whether the owner recorded it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Delivered {
    operation: u64,
    rejected: u64,
    accepted: bool,
}

fn delivered() -> Vec<Delivered> {
    usdt_trace::events()
        .iter()
        .filter(|event| event.probe == RECEIPT)
        .map(|event| Delivered {
            operation: event.field("operation").expect("receipt operation"),
            rejected: event.field("rejected").expect("receipt rejection"),
            accepted: event.field("accepted").expect("receipt answer") == 1,
        })
        .collect()
}

/// Renders until the owner has answered a receipt with `code` for
/// `operation`.
async fn render_until(
    harness: &mut ProductHarness,
    case: SyncCase,
    operation: u64,
    code: u64,
) -> Vec<Delivered> {
    let deadline = Instant::now() + RECEIPT_TIMEOUT;
    loop {
        let receipts = delivered();
        if receipts
            .iter()
            .any(|receipt| receipt.operation == operation && receipt.rejected == code)
        {
            return receipts;
        }
        assert!(
            Instant::now() < deadline,
            "{}: no receipt {code} for operation {operation} reached the owner; delivered {receipts:?}",
            case.id()
        );
        let _ = harness.render(case, BLOCK_FRAMES).await;
    }
}

/// Syncs the deck, then asks its group to prepare the track from the exact
/// recording frame `seconds` in: a launch the executor stages beside
/// whatever the deck plays. Returns the operation the preparation carries.
async fn prepare_cue(harness: &mut ProductHarness, case: SyncCase, rate: f64, cue: Start) -> u64 {
    let seconds = harness.start_seconds(0, cue);
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
    let cue = AssetFrame::new(seconds * rate).expect("fixture cue is finite");
    let admission = harness
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
    let SyncAdmission::Prepared(preparation) = admission else {
        panic!("{}: the cue was not prepared: {admission:?}", case.id());
    };
    u64::from(preparation.stamp().operation())
}

#[kithara::test(
    native,
    tokio,
    multi_thread,
    serial,
    flash(false),
    timeout(Duration::from_secs(60))
)]
#[case::tunnel_unbounded_deadline(tunnel_sources().await, TUNNEL_RATE, TUNNEL_CUE, STAGED_CUE)]
#[case::tunnel_deadline_looser_than_the_ring(
    tunnel_sources().await,
    TUNNEL_RATE,
    TUNNEL_CUE,
    STAGED_UNDER_LOOSE_DEADLINE
)]
#[case::tunnel_weak_beat(tunnel_sources().await, TUNNEL_RATE, TUNNEL_WEAK_CUE, STAGED_CUE)]
#[case::newtechno_unbounded_deadline(
    newtechno_sources().await,
    NEWTECHNO_RATE,
    NEWTECHNO_PHRASE,
    STAGED_CUE
)]
#[case::newtechno_deadline_looser_than_the_ring(
    newtechno_sources().await,
    NEWTECHNO_RATE,
    NEWTECHNO_PHRASE,
    STAGED_UNDER_LOOSE_DEADLINE
)]
async fn a_cued_sync_installs_mapped_pcm_before_anything_sounds(
    #[case] sources: PreparedSources,
    #[case] rate: f64,
    #[case] cue: Start,
    #[case] case: SyncCase,
) {
    let mut harness = ProductHarness::new(case, &sources, cue, Audible::Deck(0)).await;
    let operation = prepare_cue(&mut harness, case, rate, cue).await;

    let receipts = render_until(&mut harness, case, operation, INSTALLED).await;
    assert_eq!(
        receipts,
        [Delivered {
            operation,
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
#[case::tunnel(tunnel_sources().await, TUNNEL_RATE, TUNNEL_CUE)]
#[case::tunnel_weak_beat(tunnel_sources().await, TUNNEL_RATE, TUNNEL_WEAK_CUE)]
#[case::newtechno(newtechno_sources().await, NEWTECHNO_RATE, NEWTECHNO_PHRASE)]
async fn unloading_the_track_reports_its_installed_lane_cancelled(
    #[case] sources: PreparedSources,
    #[case] rate: f64,
    #[case] cue: Start,
) {
    let case = STAGED_CUE_BESIDE_A_DECK;
    let mut harness = ProductHarness::new(case, &sources, cue, Audible::Deck(0)).await;
    let operation = prepare_cue(&mut harness, case, rate, cue).await;
    let _ = render_until(&mut harness, case, operation, INSTALLED).await;

    let control = harness.decks[0].control().clone();
    harness.host.run(move || control.clear()).await;
    let receipts = render_until(&mut harness, case, operation, CANCELLED).await;
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
            operation,
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
#[case::tunnel(tunnel_sources().await, TUNNEL_RATE, TUNNEL_CUE)]
#[case::tunnel_weak_beat(tunnel_sources().await, TUNNEL_RATE, TUNNEL_WEAK_CUE)]
#[case::newtechno(newtechno_sources().await, NEWTECHNO_RATE, NEWTECHNO_PHRASE)]
async fn a_lane_the_worker_cannot_hold_is_refused_for_capacity(
    #[case] sources: PreparedSources,
    #[case] rate: f64,
    #[case] cue: Start,
) {
    let case = STAGED_WITHOUT_CAPACITY;
    let mut harness = ProductHarness::new(case, &sources, cue, Audible::Deck(0)).await;
    let operation = prepare_cue(&mut harness, case, rate, cue).await;

    let receipts = render_until(&mut harness, case, operation, CAPACITY).await;
    assert!(
        receipts.iter().all(|receipt| *receipt
            == Delivered {
                operation,
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
#[case::tunnel(tunnel_sources().await, TUNNEL_RATE, TUNNEL_CUE, TUNNEL_SUPERSEDING_CUE)]
#[case::tunnel_weak_beat(
    tunnel_sources().await,
    TUNNEL_RATE,
    TUNNEL_WEAK_CUE,
    TUNNEL_SUPERSEDING_CUE
)]
#[case::newtechno(
    newtechno_sources().await,
    NEWTECHNO_RATE,
    NEWTECHNO_PHRASE,
    NEWTECHNO_SUPERSEDING_CUE
)]
async fn the_sounding_lane_plays_on_while_its_staged_lane_is_superseded(
    #[case] sources: PreparedSources,
    #[case] rate: f64,
    #[case] cue: Start,
    #[case] superseding: Start,
) {
    let case = STAGED_BESIDE_PLAYBACK;
    let control = {
        let control = STAGED_BESIDE_PLAYBACK_CONTROL;
        let mut harness =
            ProductHarness::new_for_block(control, &sources, cue, Audible::Deck(0), BLOCK_FRAMES)
                .await;
        render_frames(&mut harness, control, LISTEN_FRAMES).await
    };
    let mut harness =
        ProductHarness::new_for_block(case, &sources, cue, Audible::Deck(0), BLOCK_FRAMES).await;
    harness.mark("staged cue, then a superseding cue");
    let superseded = prepare_cue(&mut harness, case, rate, cue).await;
    let successor = prepare_cue(&mut harness, case, rate, superseding).await;
    let candidate = render_frames(&mut harness, case, LISTEN_FRAMES).await;
    let receipts = render_until(&mut harness, case, successor, INSTALLED).await;
    assert!(
        receipts.iter().all(|receipt| receipt.rejected == INSTALLED
            && (receipt.operation == superseded
                || receipt
                    == &Delivered {
                        operation: successor,
                        rejected: INSTALLED,
                        accepted: true,
                    })),
        "{}: the successor installs once; a superseded lane is dropped without a \
         rejection, installed at most before it was replaced: {receipts:?}",
        case.id()
    );

    assert_eq!(candidate.len(), control.len());
    let diverged = candidate
        .iter()
        .zip(&control)
        .position(|(heard, expected)| heard.to_bits() != expected.to_bits());
    assert_eq!(
        diverged.map(|sample| sample / usize::from(CHANNELS)),
        None,
        "{}: staging beside the sounding lane changed what it plays from this frame on",
        case.id(),
    );
}
