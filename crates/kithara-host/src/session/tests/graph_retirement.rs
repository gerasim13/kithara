use std::{
    cell::RefCell,
    num::{NonZeroU32, NonZeroUsize},
};

use audioadapter_buffers::direct::InterleavedSlice;
use firewheel::{
    ActivateInfo, FirewheelContext, backend::BackendProcessInfo, node::StreamStatus,
    processor::FirewheelProcessor,
};
use kithara_command::When;
use kithara_effects::{GainDb, eq::generate_log_spaced_bands};
use kithara_events::EventBus;
use kithara_platform::time::Duration;
use kithara_signal::{SessionEpoch, SessionFrame};
use kithara_test_utils::{
    bufpool::{TestPools, pools},
    kithara,
};
use kithara_warp::{
    Beat, BeatGrid, BeatGridQuery, BeatGridRevision, BeatGridState, BeatGridUnavailable, MapAxis,
    MapPoint, MapPosition, SessionAxis,
};

use crate::{
    api::{SessionTransportSnapshot, Tempo},
    consts,
    host::{HostSettingsChange, HostSettingsExec},
    session::{
        dispatch::{invalidate_audio_route, run_cmd},
        graph::deck_at,
        protocol::{Cmd, PlayerId, Reply, SessionError},
        state::SessionState,
        tests::graph::{attach_player, state as test_state},
    },
};

/// The process-wide output device, held by whichever stream owns it.
#[derive(Default)]
struct AudioDevice {
    processor: Option<FirewheelProcessor>,
    retired_processors: Vec<FirewheelProcessor>,
    defer_processor_drop: bool,
    next_stream: u64,
    owner: u64,
}

thread_local! {
    static DEVICE: RefCell<AudioDevice> = RefCell::new(AudioDevice::default());
}

fn device<R>(f: impl FnOnce(&mut AudioDevice) -> R) -> R {
    DEVICE.with(|cell| f(&mut cell.borrow_mut()))
}

/// A fixture stream. It owns nothing but its identity: the processor lives
/// in the thread-local device, and dropping the stream is what retires it.
struct TestStream {
    stream: u64,
}

type TestState = SessionState<TestStream, TestPools>;

impl Drop for TestStream {
    fn drop(&mut self) {
        device(|dev| {
            if dev.owner == self.stream {
                let processor = dev.processor.take();
                if dev.defer_processor_drop {
                    dev.retired_processors.extend(processor);
                }
            }
        });
    }
}

fn start_test_stream(ctx: &mut FirewheelContext, sample_rate: u32) -> Result<TestStream, String> {
    let stream = device(|dev| {
        dev.next_stream += 1;
        dev.next_stream
    });
    let sample_rate = NonZeroU32::new(sample_rate).unwrap_or(
        NonZeroU32::new(TestState::DEFAULT_SAMPLE_RATE)
            .expect("invariant: fixture default sample rate is non-zero"),
    );
    let max_block_frames = NonZeroU32::new(512).expect("invariant: fixture block size is non-zero");
    let processor = ctx
        .activate(ActivateInfo {
            sample_rate,
            max_block_frames,
            num_stream_in_channels: 0,
            num_stream_out_channels: 2,
            input_to_output_latency_seconds: 0.0,
        })
        .map_err(|err| err.to_string())?;
    device(|dev| {
        dev.owner = stream;
        dev.processor = Some(processor);
    });
    Ok(TestStream { stream })
}

/// `false` means no stream owns the device, which is what silence looks like.
fn deliver_one_block() -> bool {
    device(|dev| {
        let Some(processor) = dev.processor.as_mut() else {
            return false;
        };
        let mut output = [0.0_f32; consts::GRAPH_BLOCK_FRAMES * 2];
        let input = InterleavedSlice::new(&[] as &[f32], 0, 0)
            .expect("invariant: an empty input adapter is well formed");
        let mut output = InterleavedSlice::new_mut(&mut output, 2, consts::GRAPH_BLOCK_FRAMES)
            .expect("invariant: the fixture output block is stereo");
        processor.process(
            &input,
            &mut output,
            BackendProcessInfo {
                frames: consts::GRAPH_BLOCK_FRAMES,
                // Firewheel stamps a block with its own clock type, so the
                // platform clock cannot be handed over here.
                process_timestamp: Some(bevy_platform::time::Instant::now()),
                duration_since_stream_start: Duration::ZERO,
                input_stream_status: StreamStatus::empty(),
                output_stream_status: StreamStatus::empty(),
                dropped_frames: 0,
                process_to_playback_delay: None,
            },
        );
        true
    })
}

fn processed_frames(state: &TestState) -> i64 {
    state
        .ctx
        .as_ref()
        .map_or(-1, |fw_ctx| fw_ctx.audio_clock().samples.0)
}

fn register(state: &mut TestState) -> PlayerId {
    let grid_id = attach_player(state);
    match run_cmd(
        state,
        Cmd::RegisterPlayer {
            grid_id,
            bus: EventBus::default(),
            eq_layout: generate_log_spaced_bands(5),
            gate_smoothing: kithara_play::DEFAULT_GATE_SMOOTHING,
            pools: pools(),
        },
    ) {
        Reply::PlayerRegistered(registered) => registered.id,
        Reply::Err(err) => panic!("player registration failed: {err}"),
        _ => panic!("player registration returned unexpected reply"),
    }
}

fn start(state: &mut TestState, player_id: PlayerId) {
    match run_cmd(
        state,
        Cmd::StartPlayer {
            player_id,
            render_quantum_frames: None,
            response_budget_frames: NonZeroUsize::new(448),
            master_volume: 1.0,
        },
    ) {
        Reply::Ok => {}
        Reply::Err(err) => panic!("player {player_id} failed to start: {err}"),
        _ => panic!("player start returned unexpected reply"),
    }
}

fn unregister(state: &mut TestState, player_id: PlayerId) {
    match run_cmd(state, Cmd::UnregisterPlayer { player_id }) {
        Reply::Ok => {}
        Reply::Err(err) => panic!("player {player_id} failed to unregister: {err}"),
        _ => panic!("player unregister returned unexpected reply"),
    }
}

fn stop(state: &mut TestState, player_id: PlayerId) {
    match run_cmd(state, Cmd::StopPlayer { player_id }) {
        Reply::Ok => {}
        Reply::Err(err) => panic!("player {player_id} failed to stop: {err}"),
        _ => panic!("player stop returned unexpected reply"),
    }
}

fn render_and_read_session_grid(state: &mut TestState) -> SessionTransportSnapshot {
    assert!(deliver_one_block(), "the transport must render a block");
    match run_cmd(state, Cmd::QuerySessionTransport) {
        Reply::SessionTransport(snapshot) => snapshot,
        Reply::Err(error) => panic!("transport snapshot failed: {error}"),
        _ => panic!("transport query returned an unexpected reply"),
    }
}

#[kithara::test]
fn a_session_tick_publishes_the_session_grid_the_graph_committed() {
    device(|dev| *dev = AudioDevice::default());
    let mut state = test_state(start_test_stream);
    let player = register(&mut state);
    start(&mut state, player);
    assert!(deliver_one_block(), "the transport must render a block");

    assert!(matches!(run_cmd(&mut state, Cmd::Tick), Reply::Ok));

    let committed = state
        .transport_control
        .as_mut()
        .expect("a running stream keeps transport control")
        .observation()
        .snapshot()
        .expect("the rendered block committed the tempo")
        .session_grid();
    assert_eq!(
        state.root_view.grid(),
        committed,
        "with no synchronization command, the session tick publishes the committed grid"
    );
}

/// Stopping is the verb a host reaches for when playback ends, and it must
/// release the output on its own - the existing coverage reaches this only
/// through unregister, which a host that stops without dropping its player
/// never performs. Naming the two separately is what tells a caller that
/// kept its player whether the device is free, which is the difference
/// between an audio session that can be deactivated and one that reports
/// itself busy.
#[kithara::test]
fn stopping_the_last_player_releases_the_output() {
    device(|dev| *dev = AudioDevice::default());
    let mut state = test_state(start_test_stream);
    let player_id = register(&mut state);
    start(&mut state, player_id);

    stop(&mut state, player_id);

    assert!(state.ctx.is_none());
}

/// The browser hands its output over once, on a user gesture, and a closed
/// `AudioContext` can never be resumed: releasing it on idle leaves every
/// later context suspended, so the player reports playback over silence.
/// The exception belongs to the session that declared it, not to the
/// target it happens to be compiled for — a mock backend on the same
/// target still releases its device above.
#[kithara::test]
fn a_session_that_retains_its_output_keeps_it_when_the_last_player_stops() {
    device(|dev| *dev = AudioDevice::default());
    let mut state = test_state(start_test_stream);
    state.retains_output = true;
    let player_id = register(&mut state);
    start(&mut state, player_id);

    stop(&mut state, player_id);

    assert!(
        state.ctx.is_some(),
        "a session whose device cannot be rebuilt must hold it while idle"
    );
}

#[kithara::test]
fn a_running_player_changes_its_eq_layout_in_place() {
    device(|dev| *dev = AudioDevice::default());
    let mut state = test_state(start_test_stream);
    let player_id = register(&mut state);
    start(&mut state, player_id);
    let slot = match run_cmd(&mut state, Cmd::AllocateSlot { player_id }) {
        Reply::SlotAllocated(allocated) => allocated.slot,
        Reply::Err(err) => panic!("slot allocation failed: {err}"),
        _ => panic!("slot allocation returned unexpected reply"),
    };
    let previous_eq = deck_at(&state, 0)
        .expect("the registered deck is present")
        .master_eq_node_id;
    let previous_volume = deck_at(&state, 0)
        .expect("the registered deck is present")
        .master_volume_node_id;
    let mut layout = generate_log_spaced_bands(4);
    for (band, gain) in layout.iter_mut().zip([-6.0, -3.0, 1.5, 4.0]) {
        band.set_gain_db(GainDb::from(gain));
    }

    assert!(matches!(
        run_cmd(
            &mut state,
            Cmd::SetPlayerEqLayout {
                player_id,
                eq_layout: layout,
            },
        ),
        Reply::Ok
    ));

    let player = deck_at(&state, 0).expect("the registered deck is present");
    assert_eq!(player.eq_layout.len(), 4);
    assert_eq!(player.shared_eq.snapshot(), vec![-6.0, -3.0, 1.5, 4.0]);
    assert_eq!(player.slots.len(), 1);
    assert_eq!(player.slots[0].slot_id, slot);
    assert_eq!(player.master_eq_node_id, previous_eq);
    assert_eq!(player.master_volume_node_id, previous_volume);
    assert_eq!(
        player.master_eq_memo.as_ref().map(|memo| memo.band_count()),
        Some(4)
    );
    assert!(matches!(
        run_cmd(
            &mut state,
            Cmd::SetPlayerEqGain {
                player_id,
                band: 3,
                gain_db: 5.0,
            },
        ),
        Reply::Ok
    ));
}

#[kithara::test]
fn a_second_player_started_after_the_last_one_left_gets_a_processed_stream() {
    device(|dev| *dev = AudioDevice::default());
    let mut state = test_state(start_test_stream);

    let first = register(&mut state);
    start(&mut state, first);
    assert!(
        deliver_one_block(),
        "the first player's stream must own the output device"
    );

    unregister(&mut state, first);

    assert!(
        state.ctx.is_none(),
        "the session must release the output device once no player feeds it"
    );

    let second = register(&mut state);
    start(&mut state, second);
    assert!(matches!(
        run_cmd(&mut state, Cmd::AllocateSlot { player_id: second }),
        Reply::SlotAllocated(..)
    ));

    let before = processed_frames(&state);
    assert!(
        deliver_one_block(),
        "the second player's stream must own the output device"
    );
    assert!(
        processed_frames(&state) > before,
        "the second player's stream delivered no processed callback"
    );
}

#[kithara::test]
fn idle_context_recreation_advances_generation_before_deferred_processor_drop() {
    device(|dev| {
        *dev = AudioDevice::default();
        dev.defer_processor_drop = true;
    });
    let mut state = test_state(start_test_stream);
    let first_player = register(&mut state);
    let initial = state.root.snapshot();
    assert_eq!(initial.revision(), BeatGridRevision::first());
    assert_eq!(
        initial.state(),
        BeatGridState::Unavailable(BeatGridUnavailable::NoGeometry)
    );
    assert_eq!(
        initial.axis(),
        MapAxis::Session(SessionAxis::new(
            NonZeroU32::new(TestState::DEFAULT_SAMPLE_RATE)
                .expect("the fixture sample rate is non-zero"),
            SessionEpoch::new(0),
        ))
    );
    assert_eq!(
        state
            .reserved_session_grid
            .expect("registration seeds session-grid generation")
            .stamp()
            .expect("the initial session-grid revision is committed"),
        initial.stamp()
    );
    start(&mut state, first_player);
    let before = render_and_read_session_grid(&mut state);
    let first_live = state.root.snapshot();
    assert_eq!(first_live, before.session_grid());
    assert_eq!(
        first_live.revision(),
        initial
            .revision()
            .checked_next()
            .expect("the fixture grid revision can advance")
    );
    let old_beat = MapPoint::new(
        before.session_grid_stamp(),
        Beat::new(1.0).expect("invariant: fixture beat is finite"),
    );

    invalidate_audio_route(&mut state, "deferred route before idle teardown")
        .expect("the route restarts");
    let route_boundary = state.root.snapshot();
    assert_eq!(
        state
            .reserved_session_grid
            .expect("the deferred route owns a reserved generation")
            .stamp()
            .expect("the deferred route reservation has a revision"),
        route_boundary.stamp()
    );
    assert!(state.stream_needs_restart);

    unregister(&mut state, first_player);
    let unavailable = state.root.snapshot();
    assert_eq!(
        unavailable.revision(),
        first_live
            .revision()
            .checked_next()
            .and_then(BeatGridRevision::checked_next)
            .expect("the fixture grid revision can advance twice")
    );
    assert_eq!(
        unavailable.state(),
        BeatGridState::Unavailable(BeatGridUnavailable::NoGeometry)
    );
    assert_eq!(
        unavailable.axis(),
        MapAxis::Session(SessionAxis::new(
            NonZeroU32::new(TestState::DEFAULT_SAMPLE_RATE)
                .expect("the fixture sample rate is non-zero"),
            SessionEpoch::new(2),
        ))
    );
    assert_eq!(
        state
            .reserved_session_grid
            .expect("idle teardown returns session-grid generation")
            .stamp()
            .expect("the restart boundary has a reserved revision"),
        unavailable.stamp()
    );
    assert!(
        state.ctx.is_none(),
        "idle teardown must destroy the context"
    );
    device(|dev| {
        assert_eq!(
            dev.retired_processors.len(),
            1,
            "old processor must still be alive while the new context starts"
        );
    });

    let second_player = register(&mut state);
    start(&mut state, second_player);
    let after = render_and_read_session_grid(&mut state);
    let second_live = state.root.snapshot();
    assert_eq!(second_live, after.session_grid());
    assert_eq!(
        second_live.revision(),
        unavailable
            .revision()
            .checked_next()
            .expect("the fixture grid revision can advance")
    );

    assert_eq!(
        before.session_grid_stamp().grid_id(),
        after.session_grid_stamp().grid_id(),
        "one session keeps one session-grid identity"
    );
    assert!(after.session_epoch() > before.session_epoch());
    assert!(after.session_grid_stamp().revision() > before.session_grid_stamp().revision());
    assert!(matches!(
        after.session_grid().position_at(old_beat),
        BeatGridQuery::Stale { expected, given }
            if expected == after.session_grid_stamp()
                && given == before.session_grid_stamp()
    ));
    device(|dev| {
        assert_eq!(dev.retired_processors.len(), 1);
        dev.retired_processors.clear();
        dev.defer_processor_drop = false;
    });
}

#[kithara::test]
fn deferred_route_restart_converges_before_unrendered_idle_shutdown() {
    device(|dev| {
        *dev = AudioDevice::default();
        dev.defer_processor_drop = true;
    });
    let mut state = test_state(start_test_stream);
    let player = register(&mut state);
    start(&mut state, player);
    let live = render_and_read_session_grid(&mut state);

    invalidate_audio_route(&mut state, "test route restart").expect("the route restarts");
    let reserved = state.root.snapshot();
    assert!(reserved.revision() > live.session_grid_stamp().revision());
    assert_eq!(
        state
            .reserved_session_grid
            .expect("the delayed processor keeps an exact route reservation")
            .stamp()
            .expect("the route reservation has a revision"),
        reserved.stamp()
    );
    assert!(state.stream_needs_restart);
    let Reply::SampleRate(rate) = run_cmd(&mut state, Cmd::QuerySampleRate) else {
        panic!("the pending route restart must answer the sample-rate query")
    };
    assert_eq!(rate.measured, None);
    assert_eq!(rate.requested, 44_100);
    assert!(matches!(
        run_cmd(&mut state, Cmd::QuerySessionTransport),
        Reply::Err(SessionError::TransportNotProcessed)
    ));
    assert_eq!(
        state.root.snapshot(),
        reserved,
        "a stale transport observation must not replace the route reservation"
    );
    let tempo = Tempo::new(121.0).expect("invariant: fixture tempo is valid");
    assert!(
        state
            .exec(HostSettingsChange::Tempo(tempo), When::Next, &mut ())
            .is_ok(),
        "a change for the next block waits in the queue across a route restart"
    );
    assert_eq!(
        state.root.snapshot(),
        reserved,
        "a queued change must not touch an unfinished route boundary"
    );
    device(|dev| {
        assert_eq!(dev.retired_processors.len(), 1);
        dev.retired_processors.clear();
        dev.defer_processor_drop = false;
    });
    assert!(matches!(run_cmd(&mut state, Cmd::Tick), Reply::Ok));
    assert!(!state.stream_needs_restart);
    assert!(state.reserved_session_grid.is_none());
    let converged = state
        .transport_control
        .as_mut()
        .expect("the restarted stream keeps transport control")
        .observation()
        .session_grid()
        .stamp()
        .expect("the restarted transport has a grid revision");
    assert_eq!(converged, reserved.stamp());
    let restart_frame = SessionFrame::new(
        state
            .ctx
            .as_ref()
            .expect("the restarted stream keeps its context")
            .audio_clock()
            .samples
            .0,
    );

    assert!(
        deliver_one_block(),
        "the restarted processor must render its preserved transport"
    );
    let restarted = match run_cmd(&mut state, Cmd::QuerySessionTransport) {
        Reply::SessionTransport(snapshot) => snapshot,
        Reply::Err(error) => panic!("restarted transport snapshot failed: {error}"),
        _ => panic!("restarted transport query returned an unexpected reply"),
    };
    let published = state.root.snapshot();
    assert_eq!(published.state(), BeatGridState::Live);
    let MapAxis::Session(reserved_axis) = reserved.axis() else {
        panic!("the route reservation uses the session axis")
    };
    let MapAxis::Session(published_axis) = published.axis() else {
        panic!("the restarted grid uses the session axis")
    };
    assert_eq!(published_axis.epoch(), reserved_axis.epoch());
    assert!(published.revision() > reserved.revision());
    assert_eq!(published, restarted.session_grid());
    assert_eq!(
        restarted
            .anchor()
            .frame_at(live.position())
            .expect("the preserved beat is representable on the restarted axis"),
        restart_frame
    );
    let old_position = MapPoint::new(
        live.session_grid_stamp(),
        MapPosition::Session(SessionFrame::new(0)),
    );
    assert!(matches!(
        published.beat_at(old_position),
        BeatGridQuery::Stale { .. }
    ));

    unregister(&mut state, player);

    let unavailable = state.root.snapshot();
    assert!(unavailable.revision() > reserved.revision());
    assert_eq!(
        unavailable.state(),
        BeatGridState::Unavailable(BeatGridUnavailable::NoGeometry)
    );
    assert!(state.ctx.is_none());
}
