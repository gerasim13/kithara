#![cfg(not(target_os = "android"))]
#![cfg(not(target_arch = "wasm32"))]

use kithara::{
    platform::{sync::Arc, time::Duration},
    play::{PlayError, SessionError},
    sync::{SyncError, SyncGroup, SyncIntent, SyncStatusSnapshot},
};
use kithara_integration_tests::{grid::Start, kithara};

use super::{
    sync_listening::render_frames,
    sync_product_matrix::{
        Audible, PreparedSources, ProductHarness, RETIRED_BESIDE_PLAYBACK,
        RETIRED_BESIDE_PLAYBACK_CONTROL, SyncCase, synthetic_sources,
    },
};

/// The muted deck the Host syncs and, in the candidate run, removes.
const RETIRED: usize = 0;
/// The free deck every run hears.
const SOUNDING: usize = 1;
/// The output frame each run reaches before the removal; the synced deck
/// has converged by then.
const REMOVED_AT: u64 = 4 * 48_000;
/// Output the free deck renders after the removal.
const LISTEN_FRAMES: usize = 3 * 48_000;

/// What the free deck played after the removal point, and where its own
/// clock stood at the end.
struct Heard {
    pcm: Vec<f32>,
    position: Option<f64>,
}

/// Syncs the muted deck onto the Host, renders to [`REMOVED_AT`], removes
/// that deck when `remove` holds, then hears the free deck on.
async fn heard_after(case: SyncCase, sources: &PreparedSources, remove: bool) -> Heard {
    let mut harness =
        ProductHarness::new(case, sources, Start::Seconds(0.0), Audible::Deck(SOUNDING)).await;
    let transport = harness.transport_revision(case).await;
    harness
        .request_deck_sync(case, RETIRED, transport, SyncIntent::Enable)
        .await;
    let before = usize::try_from(REMOVED_AT - harness.host.position()).expect("fixture fits usize");
    let _ = render_frames(&mut harness, case, before).await;
    let retired = Arc::clone(&harness.decks[RETIRED]);
    let synced = {
        let deck = Arc::clone(&retired);
        harness
            .host
            .with(move |host| host.deck_sync_state(&deck))
            .await
            .unwrap_or_else(|error| panic!("{}: read the synced deck: {error}", case.id()))
            .status
    };
    assert!(
        matches!(
            synced,
            SyncStatusSnapshot::Converging { .. } | SyncStatusSnapshot::Locked { .. }
        ),
        "{}: the synced deck follows the Host before its removal, got {synced:?}",
        case.id()
    );
    let sounding = Arc::clone(&harness.decks[SOUNDING]);
    if remove {
        harness.decks.remove(RETIRED);
        let removed = Arc::clone(&retired);
        harness
            .host
            .with(move |host| host.remove(&removed))
            .await
            .unwrap_or_else(|error| panic!("{}: remove the synced deck: {error}", case.id()));
        let topology = harness
            .host
            .with(|host| host.topology())
            .await
            .unwrap_or_else(|error| panic!("{}: read the session topology: {error}", case.id()));
        assert!(
            topology
                .members()
                .iter()
                .all(|member| member.grid().id() != retired.id()),
            "{}: the removed deck leaves the Host topology",
            case.id()
        );
        let id = retired.id();
        let refused = harness
            .host
            .with(move |host| host.deck_sync_state(&retired))
            .await;
        assert!(
            matches!(
                refused,
                Err(PlayError::Session(SessionError::Sync(SyncError::GroupNotFound { group_id })))
                    if group_id == id
            ),
            "{}: the Host no longer answers for the removed deck, got {refused:?}",
            case.id()
        );
        assert_eq!(
            harness.host.position(),
            REMOVED_AT,
            "{}: the removal renders nothing",
            case.id()
        );
    }
    let pcm = render_frames(&mut harness, case, LISTEN_FRAMES).await;
    assert!(
        harness.failures.is_empty(),
        "{}: {:?}",
        case.id(),
        harness.failures
    );
    Heard {
        pcm,
        position: sounding.playback_view().position,
    }
}

/// Removing a deck the Host keeps in sync leaves the free deck beside it
/// playing exactly what it plays when nothing is removed, on the same clock.
#[kithara::test(native, tokio, multi_thread, serial, timeout(Duration::from_secs(120)))]
async fn removing_a_synced_deck_leaves_the_free_deck_playing_on() {
    let sources = synthetic_sources().await;
    let control = heard_after(RETIRED_BESIDE_PLAYBACK_CONTROL, &sources, false).await;
    let candidate = heard_after(RETIRED_BESIDE_PLAYBACK, &sources, true).await;

    assert_eq!(candidate.pcm.len(), control.pcm.len());
    let diverged = candidate
        .pcm
        .iter()
        .zip(&control.pcm)
        .position(|(heard, expected)| heard != expected);
    assert_eq!(
        diverged, None,
        "removing the synced deck changed what the free deck plays from this sample on"
    );
    assert_eq!(
        candidate.position, control.position,
        "the free deck's clock runs on as if nothing were removed"
    );
}
