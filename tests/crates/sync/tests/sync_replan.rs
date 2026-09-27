#![cfg(not(target_os = "android"))]
#![cfg(not(target_arch = "wasm32"))]

use kithara::{
    platform::{sync::Arc, time::Duration},
    signal::SessionFrame,
    sync::{
        AlignmentSource, LoadGeneration, SyncGroup, SyncIntent, SyncOperation, SyncStatusSnapshot,
    },
    warp::AssetFrame,
};
use kithara_integration_tests::{grid::Start, kithara};

use super::sync_product_matrix::{
    Audible, BLOCK_FRAMES, PUBLIC_SYNTHETIC_ENABLE, ProductHarness, synthetic_sources,
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
/// Output the Host renders after the missed entry's request, at most.
const REPLAN_FRAMES: usize = 4 * 48_000;

fn block() -> i64 {
    i64::try_from(BLOCK_FRAMES).expect("render block fits i64")
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
    let passed =
        u64::try_from(MISSED_FROM + (WINDOW_BEATS + 1) * BEAT_FRAMES).expect("positive frame");
    while harness.host.position() < passed {
        let _ = harness.render(case, BLOCK_FRAMES).await;
    }

    let deck = Arc::clone(&harness.decks[0]);
    let target = deck.id();
    let heard = deck.playback_view().position.unwrap_or(0.0) * f64::from(case.sample_rate);
    let cue = AssetFrame::new(heard).expect("finite cue");
    let transport = harness.transport_revision(case).await;
    harness
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
    let state = harness
        .host
        .with(move |host| host.deck_sync_state(&deck))
        .await
        .expect("accepted deck state");
    let SyncStatusSnapshot::Prepared {
        operation: missed,
        activation: missed_at,
        ..
    } = state.status
    else {
        panic!("Enable issues one entry, got {:?}", state.status);
    };
    let requested = i64::try_from(harness.host.position()).expect("Host frame fits i64");
    assert!(
        i64::from(missed_at) < requested,
        "the entry's beat passed before any audio could claim it"
    );

    let mut replanned = None;
    let mut presented = None;
    let mut last = state.status;
    for _ in 0..REPLAN_FRAMES / BLOCK_FRAMES {
        let rendered = i64::try_from(harness.host.position()).expect("Host frame fits i64");
        let _ = harness.render(case, BLOCK_FRAMES).await;
        let deck = Arc::clone(&harness.decks[0]);
        last = harness
            .host
            .with(move |host| host.deck_sync_state(&deck))
            .await
            .expect("deck state")
            .status;
        match last {
            SyncStatusSnapshot::Prepared {
                operation,
                activation,
                ..
            } if operation != missed => {
                replanned.get_or_insert((operation, activation, rendered));
            }
            SyncStatusSnapshot::Converging { applied, .. }
            | SyncStatusSnapshot::Locked { applied, .. } => {
                presented = Some(applied);
                break;
            }
            _ => {}
        }
    }

    let applied = presented.unwrap_or_else(|| {
        panic!("rendering alone must sound the missed entry, last state {last:?}")
    });
    let (operation, activation, rendered) =
        replanned.expect("the Host plans the missed entry once more");
    assert!(operation > missed);
    assert_eq!(applied.stamp().operation(), operation);
    assert_eq!(
        applied.frontier().output(),
        SessionFrame::new(i64::from(activation) + 1),
        "the replanned entry sounds from its own activation"
    );
    let earliest = rendered + ENTRY_LEAD_FRAMES;
    assert!(
        (earliest..earliest + block() + BEAT_FRAMES).contains(&i64::from(activation)),
        "the replanned entry takes the next beat after the Host's lead: \
         rendered {rendered}, activation {activation:?}"
    );
    assert!(
        harness.failures.is_empty(),
        "the missed entry reported no harness failure: {:?}",
        harness.failures
    );
}
