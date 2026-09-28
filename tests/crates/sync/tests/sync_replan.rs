#![cfg(not(target_os = "android"))]
#![cfg(not(target_arch = "wasm32"))]

use kithara::{
    platform::{sync::Arc, time::Duration},
    signal::SessionFrame,
    sync::{
        AlignmentSource, LoadGeneration, SyncGroup, SyncIntent, SyncOperation, SyncOperationId,
        SyncStatusSnapshot,
    },
    warp::AssetFrame,
};
use kithara_integration_tests::{grid::Start, kithara, usdt_trace};

use super::sync_product_matrix::{
    Audible, BLOCK_FRAMES, PUBLIC_SYNTHETIC_ENABLE, ProductHarness, SyncCase, synthetic_sources,
};

/// Output frames one beat of the 120 BPM Host lasts at 48 kHz.
const BEAT_FRAMES: i64 = 24_000;
/// Output frames the Host leaves between the audio it has rendered and an
/// entry it plans itself.
const ENTRY_LEAD_FRAMES: i64 = 2_048;
/// Beats an entry window spans past its activation: one bar and a beat.
const WINDOW_BEATS: i64 = 5;
/// The activation of the entry whose every admissible beat has passed.
const MISSED_FROM: i64 = BEAT_FRAMES;
/// The Host beat the fresh observation lands just ahead of. Eight beats are
/// a whole number of render blocks, and the missed entry's window is over.
const LEAD_BEAT: i64 = 8 * BEAT_FRAMES;
/// Where the Host observes the deck afresh: within the entry lead before
/// [`LEAD_BEAT`], so only the lead moves the entry on to the next beat.
const OBSERVED: i64 = LEAD_BEAT - ENTRY_LEAD_FRAMES / 2;
/// Output the Host renders after the replanned entry's ticket, at most.
const PRESENT_FRAMES: usize = 4 * 48_000;
/// Probe fired once a staged entry's ticket is handed to the audio thread.
const TICKET_HANDED: &str = "sync_ticket_handed";

fn block() -> i64 {
    i64::try_from(BLOCK_FRAMES).expect("render block fits i64")
}

fn host_frame(harness: &ProductHarness) -> i64 {
    i64::try_from(harness.host.position()).expect("Host frame fits i64")
}

async fn status(harness: &ProductHarness) -> SyncStatusSnapshot {
    let deck = Arc::clone(&harness.decks[0]);
    harness
        .host
        .with(move |host| host.deck_sync_state(&deck))
        .await
        .expect("deck state")
        .status
}

/// Waits, without rendering, until `operation`'s staged ticket is handed to
/// the audio thread, so the next block is the first that can judge it.
async fn handed(trace: &usdt_trace::Scope, operation: SyncOperationId) {
    let operation = u64::from(operation);
    trace
        .wait_for(|events| {
            events.iter().any(|event| {
                event.probe == TICKET_HANDED && event.field("operation") == Some(operation)
            })
        })
        .await;
}

async fn render_block(harness: &mut ProductHarness, case: SyncCase) {
    let _ = harness.render(case, BLOCK_FRAMES).await;
}

#[kithara::test(
    native,
    tokio,
    multi_thread,
    serial,
    flash(false),
    timeout(Duration::from_secs(90))
)]
async fn a_late_deck_entry_sounds_on_the_next_beat_the_host_plans_itself() {
    let case = PUBLIC_SYNTHETIC_ENABLE;
    let sources = synthetic_sources().await;
    let mut harness = ProductHarness::new_for_block(
        case,
        &sources,
        Start::Seconds(0.0),
        Audible::Deck(0),
        BLOCK_FRAMES,
    )
    .await;
    let trace = usdt_trace::scope();
    let requested = OBSERVED - block();
    assert!(requested >= MISSED_FROM + (WINDOW_BEATS + 1) * BEAT_FRAMES);
    while host_frame(&harness) < requested {
        render_block(&mut harness, case).await;
    }
    assert_eq!(host_frame(&harness), requested, "blocks reach the request");

    let deck = Arc::clone(&harness.decks[0]);
    let target = deck.id();
    let heard = deck.playback_view().position.unwrap_or(0.0) * f64::from(case.sample_rate);
    let cue = AssetFrame::new(heard).expect("finite cue");
    let transport = harness.transport_revision(case).await;
    let _ = harness
        .host
        .with(move |host| {
            host.transact(SyncOperation::Sync {
                target,
                load: LoadGeneration::first(),
                transport,
                source: AlignmentSource::Prepared(cue),
                activation: SessionFrame::new(MISSED_FROM),
                intent: SyncIntent::Enable,
            })
        })
        .await
        .expect("a caller may ask for an entry whose beats have passed");
    let issued = status(&harness).await;
    let SyncStatusSnapshot::Prepared {
        operation: missed,
        activation: missed_at,
        ..
    } = issued
    else {
        panic!("Enable issues one entry, got {issued:?}");
    };
    assert!(
        i64::from(missed_at) < requested,
        "the entry's beat passed before any audio could claim it"
    );
    handed(&trace, missed).await;

    render_block(&mut harness, case).await;
    assert_eq!(host_frame(&harness), OBSERVED);
    let replanned = status(&harness).await;
    let SyncStatusSnapshot::Prepared {
        operation,
        activation,
        ..
    } = replanned
    else {
        panic!(
            "the block that misses the entry lets the Host plan it once more, got {replanned:?}"
        );
    };
    assert!(operation > missed, "the Host plans a new decision");
    assert_eq!(
        i64::from(activation),
        LEAD_BEAT + BEAT_FRAMES,
        "the replanned entry takes the first beat after the Host's lead"
    );
    handed(&trace, operation).await;

    let mut last = None;
    for _ in 0..PRESENT_FRAMES / BLOCK_FRAMES {
        render_block(&mut harness, case).await;
        match status(&harness).await {
            SyncStatusSnapshot::Converging { applied, .. }
            | SyncStatusSnapshot::Locked { applied, .. } => {
                assert_eq!(applied.stamp().operation(), operation);
                assert_eq!(
                    applied.frontier().output(),
                    SessionFrame::new(i64::from(activation) + 1),
                    "the replanned entry sounds from its own activation"
                );
                assert!(
                    harness.failures.is_empty(),
                    "the missed entry reported no harness failure: {:?}",
                    harness.failures
                );
                return;
            }
            state => last = Some(state),
        }
    }
    panic!("rendering alone must sound the replanned entry, last state {last:?}");
}
