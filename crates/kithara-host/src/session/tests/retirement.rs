//! Graph retirement follows the device callback however far apart its
//! callbacks run, and gives up on a callback that falls silent.
#![cfg(not(target_arch = "wasm32"))]

use std::num::{NonZeroU32, NonZeroUsize};

use audioadapter_buffers::direct::InterleavedSlice;
use firewheel::{
    ActivateInfo, FirewheelContext, backend::BackendProcessInfo, node::StreamStatus,
    processor::FirewheelProcessor,
};
use kithara_events::EventBus;
use kithara_platform::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle, spawn_named},
    time::{Duration, WallInstant},
};
use kithara_test_utils::{
    bufpool::{TestPools, pools},
    kithara,
};

use super::{
    super::{
        dispatch::run_cmd,
        protocol::{Cmd, DeviceStream, PlayerId, Reply, SessionError},
        state::SessionState,
    },
    graph::{attach_player, state as test_state},
};
use crate::api::SlotId;

const BLOCK_FRAMES: usize = 128;

type TestState = SessionState<DeviceStream<DeviceThread>, TestPools>;

/// Whether the fixture device lets its callback run.
#[derive(Clone, Copy)]
enum Callback {
    Runs,
    SilentUntil(WallInstant),
    Silent,
}

/// The fixture device's audio thread. Dropping it stops the callback.
struct DeviceThread {
    running: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Drop for DeviceThread {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// A platform device runs its callback on real time.
#[kithara::flash(false)]
fn run_device(mut processor: FirewheelProcessor, callback: &Mutex<Callback>, running: &AtomicBool) {
    let mut output = [0.0_f32; BLOCK_FRAMES * 2];
    while running.load(Ordering::Acquire) {
        let runs = match *callback.lock() {
            Callback::Runs => true,
            Callback::SilentUntil(until) => WallInstant::now() >= until,
            Callback::Silent => false,
        };
        if runs {
            let input = InterleavedSlice::new(&[] as &[f32], 0, 0)
                .expect("invariant: an empty input adapter is well formed");
            let mut output = InterleavedSlice::new_mut(&mut output, 2, BLOCK_FRAMES)
                .expect("invariant: the fixture output block is stereo");
            processor.process(
                &input,
                &mut output,
                BackendProcessInfo {
                    frames: BLOCK_FRAMES,
                    process_timestamp: Some(bevy_platform::time::Instant::now()),
                    duration_since_stream_start: Duration::ZERO,
                    input_stream_status: StreamStatus::empty(),
                    output_stream_status: StreamStatus::empty(),
                    dropped_frames: 0,
                    process_to_playback_delay: None,
                },
            );
        }
        thread::yield_now();
    }
}

/// Starts a device whose callback runs on its own thread, as a platform
/// device's does, gated by `callback`.
fn threaded_device(
    callback: &Arc<Mutex<Callback>>,
    callback_stall: Duration,
) -> impl FnMut(&mut FirewheelContext, u32) -> Result<DeviceStream<DeviceThread>, String> + Send + 'static
{
    let callback = Arc::clone(callback);
    move |ctx, sample_rate| {
        let sample_rate = NonZeroU32::new(sample_rate)
            .ok_or_else(|| "the fixture device needs a sample rate".to_owned())?;
        let max_block_frames =
            NonZeroU32::new(512).expect("invariant: fixture block size is non-zero");
        let processor = ctx
            .activate(ActivateInfo {
                sample_rate,
                max_block_frames,
                num_stream_in_channels: 0,
                num_stream_out_channels: 2,
                input_to_output_latency_seconds: 0.0,
            })
            .map_err(|error| error.to_string())?;
        let running = Arc::new(AtomicBool::new(true));
        let thread = spawn_named("kithara-test-device", {
            let running = Arc::clone(&running);
            let callback = Arc::clone(&callback);
            move || run_device(processor, &callback, &running)
        });
        Ok(DeviceStream::new(
            DeviceThread {
                running,
                thread: Some(thread),
            },
            callback_stall,
        ))
    }
}

fn running_slot(state: &mut TestState) -> (PlayerId, SlotId) {
    let grid_id = attach_player(state);
    let player_id = match run_cmd(
        state,
        Cmd::RegisterPlayer {
            grid_id,
            bus: EventBus::default(),
            eq_layout: Vec::new(),
            gate_smoothing: kithara_play::DEFAULT_GATE_SMOOTHING,
            pools: pools(),
            sample_rate: TestState::DEFAULT_SAMPLE_RATE,
        },
    ) {
        Reply::PlayerRegistered(registered) => registered.id,
        Reply::Err(error) => panic!("player registration failed: {error}"),
        _ => panic!("player registration returned an unexpected reply"),
    };
    match run_cmd(
        state,
        Cmd::StartPlayer {
            player_id,
            sample_rate: TestState::DEFAULT_SAMPLE_RATE,
            render_quantum_frames: None,
            response_budget_frames: NonZeroUsize::new(448),
            master_volume: 1.0,
        },
    ) {
        Reply::Ok => {}
        Reply::Err(error) => panic!("player start failed: {error}"),
        _ => panic!("player start returned an unexpected reply"),
    }
    match run_cmd(state, Cmd::AllocateSlot { player_id }) {
        Reply::SlotAllocated(allocated) => (player_id, allocated.slot),
        Reply::Err(error) => panic!("slot allocation failed: {error}"),
        _ => panic!("slot allocation returned an unexpected reply"),
    }
}

/// A device may run its callback far apart from the graph block size: a
/// release waits for that callback instead of a deadline measured in blocks.
#[kithara::test]
fn a_release_waits_for_a_device_callback_that_runs_late() {
    let callback = Arc::new(Mutex::new(Callback::Runs));
    let mut state = test_state(threaded_device(&callback, Duration::from_secs(1)));
    let (player_id, slot) = running_slot(&mut state);

    // Two 512-frame blocks at 44.1 kHz last 23 ms; the callback comes later.
    let late = WallInstant::now() + Duration::from_millis(60);
    *callback.lock() = Callback::SilentUntil(late);
    match run_cmd(&mut state, Cmd::ReleaseSlot { player_id, slot }) {
        Reply::Ok => {}
        Reply::Err(error) => panic!("a late device callback must still retire the slot: {error}"),
        _ => panic!("slot release returned an unexpected reply"),
    }

    assert!(
        WallInstant::now() >= late,
        "the release completed before the device callback could hand the slot back"
    );
    assert_eq!(
        state.graph.deck(0).map(|deck| deck.slots.len()),
        Some(0),
        "the released slot leaves its deck"
    );
}

/// A callback that stays silent never hands the slot back, and the release
/// reports that instead of blocking the Host.
#[kithara::test]
fn a_release_gives_up_on_a_device_callback_that_stays_silent() {
    let callback = Arc::new(Mutex::new(Callback::Runs));
    let mut state = test_state(threaded_device(&callback, Duration::from_millis(50)));
    let (player_id, slot) = running_slot(&mut state);

    *callback.lock() = Callback::Silent;
    match run_cmd(&mut state, Cmd::ReleaseSlot { player_id, slot }) {
        Reply::Err(SessionError::CallbackQuiescencePending) => {}
        Reply::Err(error) => panic!("a silent device callback must fail the release: {error}"),
        _ => panic!("a silent device callback cannot retire the slot"),
    }
}
