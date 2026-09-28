//! A released slot retires until the device callback hands its processor
//! back, however long that callback stays silent, and never blocks the Host.
#![cfg(not(target_arch = "wasm32"))]

use std::num::NonZeroU32;

use audioadapter_buffers::direct::InterleavedSlice;
use firewheel::{
    ActivateInfo, FirewheelContext, backend::BackendProcessInfo, node::StreamStatus,
    processor::FirewheelProcessor,
};
use kithara_platform::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread::{self, JoinHandle, spawn_named},
    time::{Duration, WallInstant},
};
use kithara_sync::CloseError;
use kithara_test_utils::{bufpool::TestPools, kithara};

use super::{
    super::{
        dispatch::{idle_tick, run_cmd},
        native::complete_shutdown,
        protocol::{Cmd, HostReply, PlayerId, Reply, SessionError, SessionStream},
        state::SessionState,
    },
    graph::state as test_state,
    running::running_slot,
};
use crate::{api::SlotId, error::PlayError};

const BLOCK_FRAMES: usize = 128;

type TestState = SessionState<DeviceThread, TestPools>;

/// Whether the fixture device lets its callback run.
#[derive(Clone, Copy)]
enum Callback {
    Runs,
    Silent,
}

/// Where a device that outlives its stream keeps the processor.
type Kept = Arc<Mutex<Option<FirewheelProcessor>>>;

/// The fixture device's audio thread. Dropping it stops the callback; a
/// device given `kept` then keeps its processor there instead of handing it
/// back.
struct DeviceThread {
    running: Arc<AtomicBool>,
    thread: Option<JoinHandle<FirewheelProcessor>>,
    kept: Option<Kept>,
}

/// A platform device runs its own callbacks.
impl SessionStream for DeviceThread {}

impl Drop for DeviceThread {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Release);
        if let Some(thread) = self.thread.take()
            && let Ok(processor) = thread.join()
            && let Some(kept) = &self.kept
        {
            *kept.lock() = Some(processor);
        }
    }
}

/// A platform device runs its callback on real time. The gate stays held
/// through each callback, so none is in flight once the test changes it.
#[kithara::flash(false)]
fn run_device(
    mut processor: FirewheelProcessor,
    callback: &Mutex<Callback>,
    running: &AtomicBool,
) -> FirewheelProcessor {
    let mut output = [0.0_f32; BLOCK_FRAMES * 2];
    while running.load(Ordering::Acquire) {
        let gate = callback.lock();
        if matches!(*gate, Callback::Runs) {
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
        drop(gate);
        thread::yield_now();
    }
    processor
}

/// Starts a device whose callback runs on its own thread, as a platform
/// device's does, gated by `callback`; with `kept`, the device keeps its
/// processor there once its stream stops.
fn threaded_device(
    callback: &Arc<Mutex<Callback>>,
    kept: Option<&Kept>,
) -> impl FnMut(&mut FirewheelContext, u32) -> Result<DeviceThread, String> + Send + 'static {
    let callback = Arc::clone(callback);
    let kept = kept.map(Arc::clone);
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
        Ok(DeviceThread {
            running,
            thread: Some(thread),
            kept: kept.clone(),
        })
    }
}

fn release(state: &mut TestState, player_id: PlayerId, slot: SlotId) {
    match run_cmd(state, Cmd::ReleaseSlot { player_id, slot }) {
        Reply::Ok => {}
        Reply::Err(error) => panic!("a silent device callback must not fail the release: {error}"),
        _ => panic!("slot release returned an unexpected reply"),
    }
    assert_eq!(
        state.graph.deck(0).map(|deck| deck.slots.len()),
        Some(0),
        "the released slot leaves its deck at once"
    );
}

/// A released slot stays retiring while the callback is silent, and the
/// Host reaps it on a tick once the callback has handed its processor back.
#[kithara::test]
fn a_released_slot_retires_until_the_device_callback_hands_it_back() {
    let callback = Arc::new(Mutex::new(Callback::Runs));
    let mut state = test_state(threaded_device(&callback, None));
    let (player_id, slot) = running_slot(&mut state);

    *callback.lock() = Callback::Silent;
    release(&mut state, player_id, slot);
    for _ in 0..3 {
        idle_tick(&mut state);
    }
    assert_eq!(
        state.retiring.len(),
        1,
        "a silent callback still holds the slot's processor"
    );

    *callback.lock() = Callback::Runs;
    let deadline = WallInstant::now() + Duration::from_secs(5);
    while !state.retiring.is_empty() {
        assert!(
            WallInstant::now() < deadline,
            "a running callback must hand the retired slot back"
        );
        idle_tick(&mut state);
        thread::yield_now();
    }
}

/// An interrupted device never runs its callback again, so neither a release
/// nor the player stop may wait for it; the stop drops the idle stream,
/// which hands every retiring processor back.
#[kithara::test]
fn a_player_stop_reaps_the_slot_a_silent_callback_never_handed_back() {
    let callback = Arc::new(Mutex::new(Callback::Runs));
    let mut state = test_state(threaded_device(&callback, None));
    let (player_id, slot) = running_slot(&mut state);

    *callback.lock() = Callback::Silent;
    release(&mut state, player_id, slot);
    assert_eq!(state.retiring.len(), 1);

    match run_cmd(&mut state, Cmd::StopPlayer { player_id }) {
        Reply::Ok => {}
        Reply::Err(error) => panic!("a silent device callback must not fail the stop: {error}"),
        _ => panic!("player stop returned an unexpected reply"),
    }
    assert!(state.stream.is_none(), "an idle session drops its stream");
    assert!(
        state.retiring.is_empty(),
        "dropping the stream hands the retiring processor back"
    );
}

/// A device whose audio thread keeps its processor after the stream stops,
/// as a callback the platform never returns from does, still holds a slot's
/// receipt producer when the session closes. The shutdown still ends the
/// session and returns, and its reply says the audio had not quiesced.
#[kithara::test]
fn a_shutdown_whose_callback_kept_its_processor_replies_it_was_not_quiesced() {
    let callback = Arc::new(Mutex::new(Callback::Runs));
    let kept = Kept::default();
    let mut state = test_state(threaded_device(&callback, Some(&kept)));
    let _slot = running_slot(&mut state);
    let (_cmd_tx, cmd_rx) = mpsc::channel();
    let (reply_tx, reply_rx) = mpsc::channel();

    complete_shutdown(cmd_rx, state, &reply_tx);

    match reply_rx.recv().expect("the shutdown replies") {
        HostReply::Err(PlayError::Session(SessionError::SyncClose(CloseError::CallbackLive(
            _,
        )))) => {}
        HostReply::Ok => panic!("a callback that kept its processor must not end in success"),
        HostReply::Err(error) => panic!("the shutdown failed otherwise: {error}"),
        _ => panic!("the shutdown returned an unexpected reply"),
    }
    assert!(
        kept.lock().take().is_some(),
        "the device kept its processor"
    );
}
