use std::{
    num::NonZeroU32,
    sync::atomic::{AtomicUsize, Ordering},
};

use kithara_audio::{
    Fetch, PcmProducerPort, PcmSource, PreloadGate, TrackStep, WaitingReason, mock::PcmSourceMock,
};
use kithara_bufpool::{BytePool, PcmPool};
use kithara_decode::{PcmChunk, PcmMeta, PcmSpec};
use kithara_events::{AudioEvent, DeferredBus, Event, EventBus};
use kithara_platform::{sync::Arc, time::Duration};
use kithara_stream::{
    PlayheadRead, PlayheadState, PlayheadWrite, SeekControl, SeekObserve, SeekState,
};
use kithara_test_utils::kithara;
use unimock::{MockFn, Unimock, matching};

use super::*;
use crate::{
    effects::EffectDrain,
    worker::{
        EngineLoad, WarpSource,
        scheduler::{AtomicServiceClass, Node, ServiceClass, TickResult},
    },
};

fn empty_chunk() -> PcmChunk {
    PcmChunk::new(PcmMeta::default(), PcmPool::default().attach(Vec::new()))
}

fn test_node<S>(
    source: S,
    port: PcmProducerPort,
    preload_gate: Arc<PreloadGate>,
    seek_obs: Arc<dyn SeekObserve>,
) -> DecoderNode<S> {
    DecoderNode {
        seek_obs,
        source,
        retired_chunk: None,
        port,
        preload_gate,
        playhead: Arc::new(PlayheadState::new()) as Arc<dyn PlayheadWrite>,
        emit: Arc::new(DeferredBus::new(EventBus::new(8), 8)),
        service_class: Arc::new(AtomicServiceClass::new(ServiceClass::default())),
        preload_chunks: 1,
        engine_load: None,
        runtime: DecoderRuntime::default(),
    }
}

struct PersistentEofSource {
    seek: Arc<SeekState>,
}

struct RetiringSource {
    seek: Arc<SeekState>,
    retired: Arc<AtomicUsize>,
    chunks_left: usize,
}

impl PcmSource for RetiringSource {
    type Chunk = PcmChunk;

    fn retire_chunk(&self, _chunk: PcmChunk) {
        self.retired.fetch_add(1, Ordering::Relaxed);
    }

    fn seek_observe(&self) -> Arc<dyn SeekObserve> {
        Arc::clone(&self.seek) as Arc<dyn SeekObserve>
    }

    fn step_track(&mut self) -> TrackStep<PcmChunk> {
        if self.chunks_left == 0 {
            return TrackStep::Eof;
        }
        self.chunks_left -= 1;
        TrackStep::Produced(Fetch::data(empty_chunk(), 0))
    }
}

impl PcmSource for PersistentEofSource {
    type Chunk = PcmChunk;

    fn seek_observe(&self) -> Arc<dyn SeekObserve> {
        Arc::clone(&self.seek) as Arc<dyn SeekObserve>
    }

    fn step_track(&mut self) -> TrackStep<PcmChunk> {
        TrackStep::Eof
    }
}

#[kithara::test]
fn decoder_node_eof_under_backpressure() {
    let gate = Arc::new(PreloadGate::default());
    let (mut port, mut pop) = PcmProducerPort::probe(1);

    assert!(port.try_push(Fetch::data(empty_chunk(), 0)));
    assert!(port.try_push(Fetch::data(empty_chunk(), 0)));
    assert!(port.has_pending());

    let source = Unimock::new((
        PcmSourceMock::step_track.stub(|each| {
            each.call(matching!()).answers(&|_| TrackStep::Eof);
        }),
        PcmSourceMock::decode_epoch.stub(|each| {
            each.call(matching!()).returns(0u64);
        }),
    ));

    let bus = EventBus::new(8);
    let mut events = bus.subscribe();
    let mut node = test_node(
        source,
        port,
        gate,
        Arc::new(SeekState::new()) as Arc<dyn SeekObserve>,
    );
    node.emit = Arc::new(DeferredBus::new(bus, 8));

    assert_eq!(node.tick(), TickResult::Backpressured);
    assert!(!node.runtime.eof_sent);

    let _ = node.port.take_pending();

    assert_eq!(node.tick(), TickResult::Progress);
    assert!(node.runtime.eof_sent);
    assert!(node.port.has_pending());

    assert!(pop().is_some(), "the queued data must drain first");
    assert!(node.port.flush(), "the EOF marker must leave overflow");
    assert!(matches!(pop(), Some(Fetch::NaturalEof { .. })));
    assert_eq!(node.tick(), TickResult::Backpressured);

    node.emit.flush();
    let end_events = std::iter::from_fn(|| events.try_recv().ok())
        .filter(|envelope| matches!(envelope.event, Event::Audio(AudioEvent::EndOfStream { .. })))
        .count();
    assert_eq!(end_events, 1, "current-epoch EOF must publish exactly once");
}

#[kithara::test]
fn decoder_node_does_not_republish_exhausted_warp_source_eof() {
    let seek = Arc::new(SeekState::new());
    let source = PersistentEofSource {
        seek: Arc::clone(&seek),
    };
    let effects = Vec::new();
    let drain = EffectDrain::new(effects.len(), &BytePool::default());
    let spec = PcmSpec::new(2, NonZeroU32::new(44_100).expect("test sample rate"));
    #[cfg(not(target_arch = "wasm32"))]
    let source = {
        let config = kithara_warp::WarpConfig::builder().build();
        let warp = kithara_warp::Warp::new((), &config);
        let renderer = warp.renderer(spec, PcmPool::default());
        WarpSource::new(source, renderer, effects, drain, spec)
    };
    #[cfg(target_arch = "wasm32")]
    let source = WarpSource::new(source, effects, drain, spec);
    let (port, mut pop) = PcmProducerPort::probe(1);
    let bus = EventBus::new(8);
    let mut events = bus.subscribe();
    let mut node = test_node(
        source,
        port,
        Arc::new(PreloadGate::default()),
        seek as Arc<dyn SeekObserve>,
    );
    node.emit = Arc::new(DeferredBus::new(bus, 8));

    assert_eq!(node.tick(), TickResult::Progress);
    assert_eq!(node.tick(), TickResult::Progress);
    assert!(matches!(pop(), Some(Fetch::NaturalEof { .. })));
    assert_eq!(node.tick(), TickResult::Backpressured);
    assert_eq!(node.tick(), TickResult::Backpressured);

    node.emit.flush();
    let end_events = std::iter::from_fn(|| events.try_recv().ok())
        .filter(|envelope| matches!(envelope.event, Event::Audio(AudioEvent::EndOfStream { .. })))
        .count();
    assert_eq!(end_events, 1);
}

#[kithara::test]
fn decoder_node_records_engine_load_on_produced() {
    use std::num::NonZero;

    use kithara_decode::PcmSpec;

    let meter = Arc::new(EngineLoad::default());
    assert!(!meter.snapshot().is_active(), "idle before any tick");

    let (port, _pop) = PcmProducerPort::probe(4);
    let chunk = PcmChunk::new(
        PcmMeta {
            spec: PcmSpec {
                channels: 2,
                sample_rate: NonZero::new(44_100).unwrap(),
            },
            frames: 4_410,
            ..Default::default()
        },
        PcmPool::default().attach(vec![0.0f32; 4_410 * 2]),
    );
    let source = Unimock::new(
        PcmSourceMock::step_track
            .next_call(matching!())
            .returns(TrackStep::Produced(Fetch::data(chunk, 0))),
    );

    let mut node = DecoderNode {
        source,
        retired_chunk: None,
        port,
        seek_obs: Arc::new(SeekState::new()) as Arc<dyn SeekObserve>,
        preload_gate: Arc::new(PreloadGate::default()),
        playhead: Arc::new(PlayheadState::new()) as Arc<dyn PlayheadWrite>,
        emit: Arc::new(DeferredBus::new(EventBus::new(8), 8)),
        service_class: Arc::new(AtomicServiceClass::new(ServiceClass::default())),
        preload_chunks: 1,
        engine_load: Some(Arc::clone(&meter)),
        runtime: DecoderRuntime::default(),
    };

    assert_eq!(node.tick(), TickResult::Progress);
    assert!(
        meter.snapshot().is_active(),
        "engine meter records on a Produced tick: {:?}",
        meter.snapshot()
    );
}

#[kithara::test]
fn worker_telemetry_throttles_immediate_repeats() {
    let (port, _pop) = PcmProducerPort::probe(4);
    let source = Unimock::new(());
    let gate = Arc::new(PreloadGate::default());
    let seek = Arc::new(SeekState::new());
    let playhead = Arc::new(PlayheadState::new());
    playhead.set_position(Duration::from_millis(100));
    playhead.set_decoded_frontier(Duration::from_millis(350));
    let bus = EventBus::new(8);
    let mut events = bus.subscribe();
    let emit = Arc::new(DeferredBus::new(bus, 8));
    let meter = Arc::new(EngineLoad::default());
    meter.record(Duration::from_millis(5), 4_410, 44_100);

    let mut node = DecoderNode {
        source,
        retired_chunk: None,
        port,
        seek_obs: Arc::clone(&seek) as Arc<dyn SeekObserve>,
        preload_gate: gate,
        playhead: Arc::clone(&playhead) as Arc<dyn PlayheadWrite>,
        emit: Arc::clone(&emit),
        service_class: Arc::new(AtomicServiceClass::new(ServiceClass::default())),
        preload_chunks: 1,
        engine_load: Some(meter),
        runtime: DecoderRuntime::default(),
    };

    let now = Instant::now();
    node.maybe_emit_worker_telemetry(now);
    node.maybe_emit_worker_telemetry(now);
    emit.flush();

    assert!(matches!(
        events.try_recv().map(|envelope| envelope.event),
        Ok(Event::Audio(AudioEvent::BufferHealth {
            buffered_ms: 250,
            decoded_frontier_ms: 350,
            seek_epoch: 0,
        }))
    ));
    assert!(matches!(
        events.try_recv().map(|envelope| envelope.event),
        Ok(Event::Audio(AudioEvent::EngineLoad { .. }))
    ));
    assert!(
        events.try_recv().is_err(),
        "second immediate tick stays throttled"
    );
}

#[kithara::test]
fn decoder_node_distinguishes_failed_from_eof_on_the_wire() {
    fn drain_marker(
        port: &mut PcmProducerPort,
        pop: &mut impl FnMut() -> Option<Fetch<PcmChunk>>,
    ) -> Fetch<PcmChunk> {
        let _ = port.flush();
        pop().expect("producer pushed a terminal marker")
    }

    let gate = Arc::new(PreloadGate::default());

    let (eof_port, mut eof_pop) = PcmProducerPort::probe(1);
    let eof_source = Unimock::new((
        PcmSourceMock::step_track
            .next_call(matching!())
            .returns(TrackStep::Eof),
        PcmSourceMock::decode_epoch.stub(|each| {
            each.call(matching!()).returns(0u64);
        }),
    ));
    let mut eof_node = test_node(
        eof_source,
        eof_port,
        Arc::clone(&gate),
        Arc::new(SeekState::new()) as Arc<dyn SeekObserve>,
    );
    assert_eq!(eof_node.tick(), TickResult::Progress);
    let eof_marker = drain_marker(&mut eof_node.port, &mut eof_pop);

    let (failed_port, mut failed_pop) = PcmProducerPort::probe(1);
    let failed_source = Unimock::new((
        PcmSourceMock::step_track
            .next_call(matching!())
            .returns(TrackStep::Failed),
        PcmSourceMock::decode_epoch.stub(|each| {
            each.call(matching!()).returns(0u64);
        }),
    ));
    let mut failed_node = test_node(
        failed_source,
        failed_port,
        gate,
        Arc::new(SeekState::new()) as Arc<dyn SeekObserve>,
    );
    let _ = failed_node.tick();
    let failed_marker = drain_marker(&mut failed_node.port, &mut failed_pop);

    assert!(matches!(eof_marker, Fetch::NaturalEof { .. }));
    assert!(matches!(failed_marker, Fetch::Failure { .. }));
}

#[kithara::test]
fn eof_marker_and_deferred_event_keep_the_decode_epoch() {
    let gate = Arc::new(PreloadGate::default());
    let (port, mut pop) = PcmProducerPort::probe(1);

    let seek_state = Arc::new(SeekState::new());
    let seek_obs = Arc::clone(&seek_state) as Arc<dyn SeekObserve>;

    let source = Unimock::new((
        PcmSourceMock::step_track
            .next_call(matching!())
            .returns(TrackStep::Eof),
        PcmSourceMock::decode_epoch
            .next_call(matching!())
            .returns(0u64),
    ));

    let bus = EventBus::new(8);
    let mut events = bus.subscribe();
    let mut node = test_node(source, port, gate, seek_obs);
    node.emit = Arc::new(DeferredBus::new(bus, 8));
    assert_eq!(node.tick(), TickResult::Progress);

    let live_epoch = seek_state.begin(Duration::from_secs(1));
    assert_eq!(live_epoch, 1, "seek overtakes the deferred EOF flush");

    let _ = node.port.flush();
    let marker = pop().expect("producer pushed an EOF marker");
    assert!(matches!(&marker, Fetch::NaturalEof { .. }));
    assert_eq!(
        marker.epoch(),
        0,
        "EOF marker must carry the producer decode epoch"
    );
    node.emit.flush();
    let mut eof_epochs =
        std::iter::from_fn(|| events.try_recv().ok()).filter_map(|envelope| match envelope.event {
            Event::Audio(AudioEvent::EndOfStream { seek_epoch }) => Some(seek_epoch),
            _ => None,
        });
    assert_eq!(eof_epochs.next(), Some(0));
    assert_eq!(eof_epochs.next(), None);
}

#[kithara::test]
fn decoded_frontier_advances_only_after_final_port_admission() {
    let (mut port, _pop) = PcmProducerPort::probe(1);
    assert!(port.try_push(Fetch::data(empty_chunk(), 0)));
    assert!(port.try_push(Fetch::data(empty_chunk(), 0)));
    let end = Duration::from_millis(750);
    let mut chunk = empty_chunk();
    chunk.meta.end_timestamp = end;
    let source = Unimock::new(
        PcmSourceMock::step_track
            .next_call(matching!())
            .returns(TrackStep::Produced(Fetch::data(chunk, 0))),
    );
    let playhead = Arc::new(PlayheadState::new());
    let mut node = test_node(
        source,
        port,
        Arc::new(PreloadGate::default()),
        Arc::new(SeekState::new()) as Arc<dyn SeekObserve>,
    );
    node.playhead = Arc::clone(&playhead) as Arc<dyn PlayheadWrite>;

    assert_eq!(node.tick(), TickResult::Backpressured);
    assert_eq!(playhead.decoded_frontier(), Duration::ZERO);

    let _ = node.port.take_pending();
    assert_eq!(node.tick(), TickResult::Progress);
    assert_eq!(playhead.decoded_frontier(), end);
}

#[kithara::test]
fn decoder_node_preload_gate_waits_for_ring() {
    let gate = Arc::new(PreloadGate::default());
    let (mut port, mut pop) = PcmProducerPort::probe(1);

    assert!(port.try_push(Fetch::data(empty_chunk(), 0)));

    let source = Unimock::new((
        PcmSourceMock::step_track
            .next_call(matching!())
            .returns(TrackStep::Produced(Fetch::data(empty_chunk(), 0))),
        PcmSourceMock::step_track
            .next_call(matching!())
            .returns(TrackStep::Blocked(WaitingReason::Waiting)),
    ));

    let mut node = test_node(
        source,
        port,
        Arc::clone(&gate),
        Arc::new(SeekState::new()) as Arc<dyn SeekObserve>,
    );

    assert_eq!(node.tick(), TickResult::Progress);
    assert_eq!(node.runtime.chunks_sent, 1);
    assert!(!node.runtime.preloaded);
    assert!(!gate.is_ready());

    assert_eq!(node.tick(), TickResult::Backpressured);
    assert!(!node.runtime.preloaded);
    assert!(!gate.is_ready());

    let _ = pop();

    assert_eq!(node.tick(), TickResult::Waiting);
    assert!(node.runtime.preloaded);
    assert!(gate.is_ready());
}

#[kithara::test]
fn decoder_node_live_upstream_demand_does_not_tick_hang_wait() {
    let gate = Arc::new(PreloadGate::default());
    let (port, _pop) = PcmProducerPort::probe(2);

    let source = Unimock::new(
        PcmSourceMock::step_track
            .next_call(matching!())
            .returns(TrackStep::Blocked(WaitingReason::WaitingDemand)),
    );

    let mut node = test_node(
        source,
        port,
        gate,
        Arc::new(SeekState::new()) as Arc<dyn SeekObserve>,
    );

    assert_eq!(node.tick(), TickResult::UpstreamPending);
}

#[kithara::test]
fn decoder_node_seek_rearms_preload_gate() {
    let gate = Arc::new(PreloadGate::default());
    let (port, mut pop) = PcmProducerPort::probe(2);

    let seek_state = Arc::new(SeekState::new());
    let source = Unimock::new((
        PcmSourceMock::step_track
            .next_call(matching!())
            .returns(TrackStep::Produced(Fetch::data(empty_chunk(), 0))),
        PcmSourceMock::step_track
            .next_call(matching!())
            .returns(TrackStep::StateChanged),
        PcmSourceMock::step_track
            .next_call(matching!())
            .returns(TrackStep::Produced(Fetch::data(empty_chunk(), 0))),
    ));

    let mut node = test_node(
        source,
        port,
        Arc::clone(&gate),
        Arc::clone(&seek_state) as Arc<dyn SeekObserve>,
    );

    assert_eq!(node.tick(), TickResult::Progress);
    assert!(node.runtime.preloaded);
    assert!(gate.is_ready(), "first chunk opens the gate");

    let epoch = SeekControl::begin(&*seek_state, Duration::from_secs(1));

    assert_eq!(node.tick(), TickResult::Progress);
    assert!(!node.runtime.preloaded, "seek resets the preload runtime");
    assert!(!gate.is_ready(), "sync_seek_epoch closes the gate");

    let _ = pop();

    assert_eq!(node.tick(), TickResult::Progress);
    assert!(node.runtime.preloaded);
    assert!(gate.is_ready(), "post-seek refill reopens the gate");
    assert!(
        gate.is_ready_for_epoch(epoch),
        "post-seek refill must open the new seek epoch"
    );
}

#[kithara::test]
fn decoder_node_retires_displaced_seek_chunk_from_recycle_shell() {
    let (port, _pop) = PcmProducerPort::probe(1);
    let seek = Arc::new(SeekState::new());
    let retired = Arc::new(AtomicUsize::new(0));
    let source = RetiringSource {
        seek: Arc::clone(&seek),
        retired: Arc::clone(&retired),
        chunks_left: 2,
    };
    let mut node = test_node(
        source,
        port,
        Arc::new(PreloadGate::default()),
        Arc::clone(&seek) as Arc<dyn SeekObserve>,
    );

    assert_eq!(node.tick(), TickResult::Progress);
    assert_eq!(node.tick(), TickResult::Progress);
    assert!(node.port.has_pending(), "second chunk must occupy overflow");

    SeekControl::begin(&*seek, Duration::from_secs(1));
    let _ = node.tick();
    assert_eq!(retired.load(Ordering::Relaxed), 0);

    node.recycle();
    assert_eq!(retired.load(Ordering::Relaxed), 1);
}
