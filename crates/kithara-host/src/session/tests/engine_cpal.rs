//! The lifecycle contract runs through the same Host graph with a cpal backend.
//! A test-only dispatcher owns that graph so the production Host never exposes
//! its resident engine or raw session.
use firewheel::{FirewheelContext, cpal::CpalStream};
use kithara_audio::ConsumerWakeMode;
use kithara_platform::{
    sync::{Arc, Mutex, mpsc},
    thread::{JoinHandle, spawn_named},
};
use kithara_play::{
    Cmd, EngineImpl, PlayError, PlayWorker, PlayWorkerConfig, PlayerConfig, PlayerImpl, Reply,
    SessionBinding, SessionDispatcher, SessionSampleRate, StreamShape, player::Player,
};
use kithara_test_utils::{
    bufpool::{TestPools, pools},
    kithara,
};

use super::{engine_session_contract as contract, graph::GraphSession};
use crate::session::state::RootView;

struct CpalGraphSession {
    cmd_tx: Mutex<mpsc::Sender<CpalMessage>>,
    view: RootView,
    worker: Mutex<Option<JoinHandle<()>>>,
}

enum CpalMessage {
    Command {
        cmd: Cmd<TestPools>,
        reply_tx: mpsc::Sender<Reply>,
    },
    Shutdown,
}

impl CpalGraphSession {
    fn new() -> Self {
        let (cmd_tx, cmd_rx) = mpsc::channel::<CpalMessage>();
        let (view_tx, view_rx) = mpsc::channel();
        let worker = spawn_named("kithara-engine-cpal-contract", move || {
            let mut graph = GraphSession::<CpalStream, TestPools>::new(start_stream);
            let _ = view_tx.send(graph.view());
            while let Ok(message) = cmd_rx.recv() {
                match message {
                    CpalMessage::Command { cmd, reply_tx } => {
                        let _ = reply_tx.send(graph.exec(cmd));
                    }
                    CpalMessage::Shutdown => break,
                }
            }
        });
        Self {
            cmd_tx: Mutex::new(cmd_tx),
            view: view_rx
                .recv()
                .expect("invariant: the cpal contract session publishes its view"),
            worker: Mutex::new(Some(worker)),
        }
    }
}

impl Drop for CpalGraphSession {
    fn drop(&mut self) {
        let _ = self.cmd_tx.lock().send(CpalMessage::Shutdown);
        if let Some(worker) = self.worker.lock().take() {
            let _ = worker.join();
        }
    }
}

impl SessionDispatcher<TestPools> for CpalGraphSession {
    fn consumer_wake_mode(&self) -> ConsumerWakeMode {
        ConsumerWakeMode::RealtimeDeferred
    }

    fn exec(&self, cmd: Cmd<TestPools>) -> Result<Reply, PlayError> {
        let (reply_tx, reply_rx) = mpsc::channel();
        self.cmd_tx
            .lock()
            .send(CpalMessage::Command { cmd, reply_tx })
            .map_err(|_| PlayError::SessionGone {
                reason: "cpal contract session stopped accepting commands",
            })?;
        reply_rx.recv().map_err(|_| PlayError::SessionGone {
            reason: "cpal contract session dropped its reply channel",
        })
    }

    delegate::delegate! {
        to self.view {
            fn sample_rate(&self) -> SessionSampleRate;
            fn stream_shape(&self) -> Option<StreamShape>;
        }
    }
}

fn start_stream(ctx: &mut FirewheelContext, sample_rate: u32) -> Result<CpalStream, String> {
    let config = firewheel::cpal::CpalConfig {
        output: firewheel::cpal::CpalOutputConfig {
            desired_sample_rate: Some(sample_rate),
            ..Default::default()
        },
        ..Default::default()
    };
    CpalStream::new(ctx, config).map_err(|error| error.to_string())
}

fn run_contract(contract: impl FnOnce(&EngineImpl<TestPools>)) {
    let session: Arc<dyn SessionDispatcher<TestPools>> = Arc::new(CpalGraphSession::new());
    let mut player = PlayerImpl::new(
        PlayerConfig::builder()
            .sample_rate(GraphSession::<CpalStream, TestPools>::DEFAULT_SAMPLE_RATE)
            .worker(PlayWorker::new(PlayWorkerConfig::builder(pools()).build()))
            .session(SessionBinding::new(
                session,
                GraphSession::<CpalStream, TestPools>::DEFAULT_SAMPLE_RATE,
            ))
            .build(),
    );
    contract(player.engine());
    Player::close(&mut player).expect("close cpal fixture player");
}

#[kithara::test]
fn engine_start_stop_roundtrip() {
    run_contract(contract::start_stop_roundtrip);
}

#[kithara::test]
fn engine_holds_its_slot_while_running() {
    run_contract(contract::holds_its_slot_while_running);
}
