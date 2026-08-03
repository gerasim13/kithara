#![cfg(not(target_arch = "wasm32"))]

use std::num::NonZeroU32;

use kithara::play::{Cmd, Reply, SessionBeat, SessionTransportSnapshot, Tempo};
use kithara_integration_tests::{
    kithara,
    ring::{ManualRingConfig, ManualRingSession},
};
use num_traits::ToPrimitive;

const SAMPLE_RATE: u32 = 48_000;

fn session(block_frames: u32, capacity_blocks: usize) -> ManualRingSession {
    let rate = NonZeroU32::new(SAMPLE_RATE).expect("invariant: test sample rate is non-zero");
    ManualRingSession::start(ManualRingConfig::new(rate, block_frames, capacity_blocks))
        .expect("invariant: manual ring session starts")
}

fn expect_ok(reply: Reply) {
    match reply {
        Reply::Ok => {}
        Reply::Err(error) => panic!("session command failed: {error}"),
        _ => panic!("unexpected session command reply"),
    }
}

fn set_tempo(session: &ManualRingSession, beats_per_minute: f64) {
    let tempo = Tempo::new(beats_per_minute).expect("invariant: test tempo is valid");
    expect_ok(
        session
            .exec(Cmd::SetSessionTempo { tempo })
            .expect("invariant: tempo command reaches the session"),
    );
}

fn set_playing(session: &ManualRingSession, playing: bool) {
    expect_ok(
        session
            .exec(Cmd::SetSessionPlaying { playing })
            .expect("invariant: play-state command reaches the session"),
    );
}

fn snapshot(session: &ManualRingSession) -> SessionTransportSnapshot {
    match session
        .exec(Cmd::QuerySessionTransport)
        .expect("invariant: transport query reaches the session")
    {
        Reply::SessionTransport(snapshot) => snapshot,
        Reply::Err(error) => panic!("transport query failed: {error}"),
        _ => panic!("unexpected transport query reply"),
    }
}

fn position(session: &ManualRingSession) -> f64 {
    snapshot(session).position().get()
}

fn clock_samples(session: &ManualRingSession) -> u64 {
    session
        .clock_samples()
        .expect("invariant: manual ring clock is readable")
}

fn sample_tolerance(beats_per_second: f64) -> f64 {
    beats_per_second / f64::from(SAMPLE_RATE)
}

#[kithara::test]
fn session_transport_advances_with_rendered_frames() {
    const BLOCK_FRAMES: u32 = 512;
    const BLOCKS: usize = 7;
    let session = session(BLOCK_FRAMES, BLOCKS);
    set_tempo(&session, 120.0);

    session
        .credit(BLOCKS)
        .expect("invariant: credited blocks render");

    let frames = clock_samples(&session);
    let expected = frames
        .to_f64()
        .expect("invariant: rendered frame count fits f64")
        * 2.0
        / f64::from(SAMPLE_RATE);
    assert!((position(&session) - expected).abs() <= sample_tolerance(2.0));
}

#[kithara::test]
fn transport_position_is_independent_of_render_partitioning() {
    const TOTAL_FRAMES: u32 = 4_096;
    let mut positions = [0.0; 3];
    for (index, block_frames) in [1_024, 512, 128].into_iter().enumerate() {
        let blocks = usize::try_from(TOTAL_FRAMES / block_frames)
            .expect("invariant: test block count fits usize");
        let session = session(block_frames, blocks);
        set_tempo(&session, 120.0);
        session
            .credit(blocks)
            .expect("invariant: partitioned render completes");
        assert_eq!(clock_samples(&session), u64::from(TOTAL_FRAMES));
        positions[index] = position(&session);
    }

    assert_eq!(positions[0], positions[1]);
    assert_eq!(positions[1], positions[2]);
}

#[kithara::test]
fn tempo_change_preserves_beat_and_changes_slope_at_the_scheduled_boundary() {
    const BLOCK_FRAMES: u32 = 512;
    let session = session(BLOCK_FRAMES, 6);
    set_tempo(&session, 120.0);
    session
        .credit(2)
        .expect("invariant: initial tempo commits and advances");
    let initial = snapshot(&session);

    set_tempo(&session, 60.0);
    session
        .credit(1)
        .expect("invariant: old tempo reaches the scheduled boundary");
    let boundary = snapshot(&session);
    let old_step = f64::from(BLOCK_FRAMES) * 2.0 / f64::from(SAMPLE_RATE);
    assert_eq!(boundary.revision(), initial.revision());
    assert!(
        (boundary.position().get() - initial.position().get() - old_step).abs()
            <= sample_tolerance(2.0)
    );

    session
        .credit(1)
        .expect("invariant: new tempo applies at the boundary");
    let changed = snapshot(&session);
    let new_step = f64::from(BLOCK_FRAMES) / f64::from(SAMPLE_RATE);
    assert_eq!(changed.revision().get(), initial.revision().get() + 1);
    assert_eq!(changed.tempo().beats_per_minute(), 60.0);
    assert!(
        (changed.position().get() - boundary.position().get() - new_step).abs()
            <= sample_tolerance(1.0)
    );
}

#[kithara::test]
fn tempo_revision_is_not_observed_before_the_render_commit() {
    const BLOCK_FRAMES: u32 = 512;
    let session = session(BLOCK_FRAMES, 2);
    set_tempo(&session, 120.0);
    session.credit(1).expect("invariant: initial tempo commits");
    let before = snapshot(&session);

    set_tempo(&session, 90.0);

    assert_eq!(snapshot(&session), before);
}

#[kithara::test]
fn setting_the_same_tempo_does_not_create_a_new_revision() {
    const BLOCK_FRAMES: u32 = 512;
    let session = session(BLOCK_FRAMES, 4);
    set_tempo(&session, 120.0);
    set_tempo(&session, 120.0);
    session
        .credit(1)
        .expect("invariant: initial tempo commits once");
    let committed = snapshot(&session);
    assert_eq!(committed.revision().get(), 1);

    set_tempo(&session, 120.0);
    // Render past where a redundant revision would have landed: without this
    // the query would still be reading the pre-command snapshot.
    session
        .credit(2)
        .expect("invariant: a redundant tempo commits nothing");
    let later = snapshot(&session);
    assert_eq!(later.revision(), committed.revision());
    assert_eq!(later.tempo(), committed.tempo());
}

#[kithara::test]
fn session_seek_relocates_to_the_exact_target_beat() {
    const BLOCK_FRAMES: u32 = 512;
    let session = session(BLOCK_FRAMES, 6);
    set_tempo(&session, 120.0);
    session.credit(1).expect("invariant: initial tempo commits");
    let target = SessionBeat::new(7.25).expect("invariant: seek target is finite");
    expect_ok(
        session
            .exec(Cmd::SeekSession { target })
            .expect("invariant: seek command reaches the session"),
    );
    session
        .credit(1)
        .expect("invariant: active tempo reaches the seek boundary");
    session
        .credit(1)
        .expect("invariant: seek applies at the exact boundary");

    let rendered_step = f64::from(BLOCK_FRAMES) * 2.0 / f64::from(SAMPLE_RATE);
    let relocated_boundary = snapshot(&session).position().get() - rendered_step;
    assert!((relocated_boundary - target.get()).abs() <= sample_tolerance(2.0));
}

#[kithara::test]
fn paused_transport_holds_its_position_across_rendered_blocks() {
    const BLOCK_FRAMES: u32 = 512;
    let session = session(BLOCK_FRAMES, 8);
    set_tempo(&session, 120.0);
    session.credit(1).expect("invariant: initial tempo commits");
    set_playing(&session, false);
    session
        .credit(1)
        .expect("invariant: playing transport reaches the pause boundary");
    session
        .credit(1)
        .expect("invariant: pause applies at the boundary");
    let paused = snapshot(&session);
    assert!(!paused.is_playing());

    session
        .credit(4)
        .expect("invariant: paused blocks continue rendering");
    let later = snapshot(&session);
    assert!(!later.is_playing());
    assert_eq!(later.position(), paused.position());
}

#[kithara::test]
fn changing_tempo_while_paused_does_not_resume_playback() {
    const BLOCK_FRAMES: u32 = 512;
    let session = session(BLOCK_FRAMES, 10);
    set_tempo(&session, 120.0);
    session.credit(1).expect("invariant: initial tempo commits");
    set_playing(&session, false);
    session
        .credit(2)
        .expect("invariant: pause applies at its boundary");
    let paused = snapshot(&session);
    assert!(!paused.is_playing());

    set_tempo(&session, 90.0);
    session
        .credit(2)
        .expect("invariant: retuned tempo applies at its boundary");
    let retuned = snapshot(&session);

    assert!(!retuned.is_playing());
    assert_eq!(retuned.tempo().beats_per_minute(), 90.0);
    assert_eq!(retuned.position(), paused.position());
}

#[kithara::test]
fn resuming_after_a_pause_continues_from_the_held_position() {
    const BLOCK_FRAMES: u32 = 512;
    let session = session(BLOCK_FRAMES, 12);
    set_tempo(&session, 120.0);
    session.credit(1).expect("invariant: initial tempo commits");
    set_playing(&session, false);
    session
        .credit(2)
        .expect("invariant: pause applies at its boundary");
    let paused = snapshot(&session);
    session
        .credit(3)
        .expect("invariant: paused blocks continue rendering");

    set_playing(&session, true);
    session
        .credit(2)
        .expect("invariant: resume applies at its boundary");
    let resumed = snapshot(&session);

    let step = f64::from(BLOCK_FRAMES) * 2.0 / f64::from(SAMPLE_RATE);
    assert!(resumed.is_playing());
    assert!(
        (resumed.position().get() - paused.position().get() - step).abs() <= sample_tolerance(2.0),
        "resume must continue from the held beat, not skip the paused span"
    );
}

#[kithara::test]
fn tempo_rejects_values_outside_the_representable_range() {
    for invalid in [
        0.0,
        -1.0,
        f64::NAN,
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::MAX,
        f64::MIN_POSITIVE,
        Tempo::MIN_BEATS_PER_MINUTE - 0.001,
        Tempo::MAX_BEATS_PER_MINUTE + 0.001,
    ] {
        assert!(
            Tempo::new(invalid).is_err(),
            "tempo {invalid} must be rejected"
        );
    }
    for valid in [
        Tempo::MIN_BEATS_PER_MINUTE,
        120.0,
        Tempo::MAX_BEATS_PER_MINUTE,
    ] {
        assert!(Tempo::new(valid).is_ok(), "tempo {valid} must be accepted");
    }
}
