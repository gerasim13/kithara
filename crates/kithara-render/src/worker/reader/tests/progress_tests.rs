use kithara_platform::time::{Duration, WallInstant};
use kithara_test_utils::kithara;
use kithara_worker::{
    DispatcherConfig, Event, Observer, Task, TaskConfig, TickResult, Wake, Worker, WorkerConfig,
};
use ringbuf::traits::{Consumer, Producer};

use super::transport_tests::receive_notice;
use crate::{
    mock::pcm_fixture::{PcmFixture, chunk},
    worker::{PcmPacket, reader::PcmProducer, scheduler::StreamWake},
};

async fn join_thread<F>(join: F)
where
    F: FnOnce() -> std::thread::Result<()> + Send + 'static,
{
    kithara_platform::tokio::task::spawn_sync(join)
        .await
        .unwrap_or_else(|error| std::panic::resume_unwind(error.into_panic()))
        .unwrap_or_else(|panic| std::panic::resume_unwind(panic));
}

#[kithara::test(native, tokio)]
async fn pcm_thread_completion_does_not_require_thread_exit_in_async_poll() {
    let (completed, completion) = kithara_platform::tokio::sync::oneshot::channel();
    let (release, released) = std::sync::mpsc::channel();
    let thread = kithara_platform::thread::spawn_named("pcm-exit-order", move || {
        completed.send(()).expect("publish completion before exit");
        released.recv().expect("release thread exit");
    });
    completion.await.expect("thread published completion");
    assert!(!thread.is_finished(), "completion precedes thread exit");
    kithara_platform::tokio::join!(
        biased;
        join_thread(move || thread.join()),
        async { release.send(()).expect("allow thread exit after join yields") },
    );
}

struct ProducerTask {
    producer: PcmProducer,
    started: bool,
    next: usize,
    first_output: std::sync::mpsc::Sender<()>,
}

impl Task for ProducerTask {
    fn tick(&mut self) -> TickResult {
        if !std::mem::replace(&mut self.started, true) {
            return TickResult::Backpressured;
        }
        if let Some(packet) = self.producer.reverse.try_pop() {
            drop(packet);
        }
        if self.next == 32 {
            return TickResult::Backpressured;
        }
        let samples = vec![f32::from(u16::try_from(self.next).expect("packet index")); 480];
        if self
            .producer
            .forward
            .try_push(PcmPacket::Chunk(Box::new(chunk(
                kithara_signal::SegmentId::FIRST,
                &samples,
            ))))
            .is_err()
        {
            return TickResult::Backpressured;
        }
        self.producer.signal();
        self.producer.flush();
        self.next += 1;
        if self.next == 1 {
            self.first_output.send(()).expect("first output notice");
        }
        TickResult::Progress
    }
}

struct DemandObserver {
    wake: std::sync::Arc<std::sync::OnceLock<Wake>>,
    parked: Option<std::sync::mpsc::Sender<()>>,
}

impl Observer for DemandObserver {
    fn on_event(&mut self, event: Event) {
        if matches!(event, Event::Backpressured(_))
            && let Some(parked) = self.parked.take()
        {
            let wake = self.wake.get().expect("dispatcher wake installed");
            let baseline = kithara_worker::mock::wake_state(wake).0;
            parked.send(()).expect("producer is empty and parked");
            wait_for_demand(wake, baseline);
        }
    }
}

#[kithara::flash(io)]
#[kithara::hang_watchdog]
fn wait_for_demand(wake: &Wake, baseline: u64) {
    loop {
        hang_tick!();
        let (signals, pending) = kithara_worker::mock::wake_state(wake);
        if signals > baseline || pending {
            hang_reset!();
            return;
        }
        hang_park!(|remaining: Duration| {
            std::thread::sleep(remaining.min(Duration::from_micros(100)));
        });
    }
}

#[kithara::test(native, tokio)]
#[kithara::hang_watchdog]
#[case::blocking_offline(true)]
#[case::paced_nonblocking(false)]
async fn a_live_producer_serves_consumed_packets_without_underruns(#[case] blocking: bool) {
    let wake_slot = std::sync::Arc::new(std::sync::OnceLock::new());
    let (parked, empty) = std::sync::mpsc::channel();
    let (produced, output) = std::sync::mpsc::channel();
    let worker = Worker::new(WorkerConfig::new());
    let dispatcher = worker.dispatcher(
        DispatcherConfig::builder()
            .name("consumer-progress")
            .observer(DemandObserver {
                wake: std::sync::Arc::clone(&wake_slot),
                parked: Some(parked),
            })
            .build(),
    );
    let wake = dispatcher.wake_handle();
    assert!(wake_slot.set(wake.clone()).is_ok());
    let mut fixture = PcmFixture::with_wake(4, blocking, StreamWake::new(wake.clone())).await;
    let producer = fixture.producer.take().expect("producer owner");
    let _task = dispatcher
        .register(TaskConfig::new(), move |_| ProducerTask {
            producer,
            started: false,
            next: 0,
            first_output: produced,
        })
        .expect("register live producer");
    receive_notice(&empty);
    let baseline = kithara_worker::mock::wake_state(&wake).0;
    let mut receiver = fixture.receiver.take().expect("receiver");
    if blocking {
        receiver = kithara_platform::tokio::task::spawn_sync(move || {
            receiver.wait_for_packet();
            receiver
        })
        .await
        .expect("off-RT packet wait");
    } else {
        assert!(receiver.pop().is_none(), "the first poll states demand");
    }
    receive_notice(&output);
    assert_eq!(
        kithara_worker::mock::wake_state(&wake).0,
        baseline + u64::from(blocking),
        "only the explicit off-RT wait signals the producer immediately"
    );
    let mut underruns = 0;
    let started = WallInstant::now();
    for expected in 0..32 {
        hang_tick!();
        if blocking {
            receiver = kithara_platform::tokio::task::spawn_sync(move || {
                receiver.wait_for_packet();
                receiver
            })
            .await
            .expect("off-RT packet wait");
        }
        match receiver.pop() {
            Some(PcmPacket::Chunk(packet)) => {
                assert_eq!(packet.frames(), 480);
                assert!(packet.samples.iter().all(|sample| {
                    *sample == f32::from(u16::try_from(expected).expect("packet index"))
                }));
                receiver
                    .recycle(PcmPacket::Chunk(packet))
                    .expect("packet returns to producer");
                hang_reset!();
            }
            _ => underruns += 1,
        }
        let _real_io = kithara_platform::flash::real_io();
        kithara_platform::time::sleep(Duration::from_millis(10)).await;
        hang_tick!();
    }
    assert!(started.elapsed() >= Duration::from_millis(320));
    assert_eq!(underruns, 0, "paced callbacks never wait for PCM");
    fixture.receiver = Some(receiver);
    dispatcher.shutdown();
}

#[cfg(unix)]
mod pcm_input_wait {
    use std::{
        io::{ErrorKind, Read, Write},
        os::unix::net::UnixStream,
        panic::{AssertUnwindSafe, catch_unwind, resume_unwind},
        sync::mpsc,
    };

    use kithara_platform::{
        sync::{Arc, ThreadGate, WaitGate},
        time::{Duration, WallInstant},
        tokio::sync::oneshot,
    };
    use kithara_signal::SegmentId;
    use kithara_test_utils::kithara;
    use ringbuf::traits::Producer;

    use super::super::packet_fixture;
    use crate::{
        mock::pcm_fixture::chunk,
        worker::{
            PcmPacket,
            reader::{PcmProducer, mock},
        },
    };

    #[kithara::hang_watchdog(timeout = Duration::from_secs(1))]
    fn parse_input(
        input: &mut UnixStream,
        ready: &ThreadGate,
        producer: &mut PcmProducer,
        packet: PcmPacket,
    ) {
        {
            let _real_io = kithara_platform::flash::real_io();
            let mut byte = [0];
            loop {
                hang_tick!();
                let since = ready.current();
                match input.read(&mut byte) {
                    Ok(count) => {
                        assert_eq!(count, 1, "input EOF is not PCM progress");
                        assert_eq!(byte, [1]);
                        break;
                    }
                    Err(error) if error.kind() == ErrorKind::WouldBlock => {
                        hang_park!(|remaining: Duration| {
                            ready.wait_timeout(since, remaining.min(Duration::from_millis(25)));
                        });
                    }
                    Err(error) => panic!("input read failed: {error}"),
                }
            }
        }
        producer
            .forward
            .try_push(packet)
            .expect("known PCM fits the empty ring");
        producer.signal();
        hang_reset!();
    }

    async fn input_wait(mock_wait: bool, deliver: bool) {
        let (mut input, mut peer) = UnixStream::pair().expect("input socket pair");
        input.set_nonblocking(true).expect("nonblocking parser");
        let ready = Arc::new(ThreadGate::default());
        let peer_ready = Arc::clone(&ready);
        let (start, started) = mpsc::channel();
        let (peer_finished, peer_completion) = oneshot::channel();
        let peer = std::thread::spawn(move || {
            started.recv().expect("consumer started");
            if deliver {
                kithara_platform::thread::sleep(Duration::from_millis(500));
                peer.write_all(&[1]).expect("external input byte");
                peer_ready.signal();
            }
            let _ = peer_finished.send(peer);
        });
        let packet = chunk(SegmentId::FIRST, &[0.25; 8]);
        let (mut receiver, mut producer) = packet_fixture(true, packet.spec());
        assert!(receiver.peek().is_none());
        let (parser_finished, parser_completion) = oneshot::channel();
        let parser = kithara_platform::thread::spawn_named("pcm-input-parser", move || {
            let parsed = catch_unwind(AssertUnwindSafe(|| {
                parse_input(
                    &mut input,
                    &ready,
                    &mut producer,
                    PcmPacket::Chunk(Box::new(packet)),
                );
            }));
            // Keep both owners alive even when the parser watchdog fires first.
            let _ = parser_finished.send((producer, input, parsed));
        });
        let outcome = kithara_platform::tokio::task::spawn_sync(move || {
            let began = WallInstant::now();
            start.send(()).expect("start external peer");
            if mock_wait {
                mock::wait_for_packet(&receiver);
            } else {
                receiver.wait_for_packet();
            }
            let PcmPacket::Chunk(packet) = receiver.pop().expect("PCM, not producer closure")
            else {
                panic!("expected PCM, not a failure packet");
            };
            assert_eq!(packet.frames(), 8);
            assert_eq!(&packet.samples[..], &[0.25; 8]);
            assert!(!packet.meta.end_of_track);
            assert!(began.elapsed() < Duration::from_secs(1));
        })
        .await;
        let parsed = parser_completion.await;
        super::join_thread(move || parser.join()).await;
        let (producer, input, parsed) = parsed.expect("parser completion");
        let completed_peer = peer_completion.await;
        super::join_thread(move || peer.join()).await;
        let peer = completed_peer.expect("peer completion");
        outcome.unwrap_or_else(|error| resume_unwind(error.into_panic()));
        parsed.unwrap_or_else(|panic| resume_unwind(panic));
        drop((producer, input, peer));
    }

    #[kithara::test(native, tokio, hang_timeout_secs(1))]
    #[case::production(false)]
    #[case::mock(true)]
    async fn accounted_input_reaches_pcm_wait(#[case] mock_wait: bool) {
        input_wait(mock_wait, true).await;
    }

    #[kithara::test(native, tokio, hang_timeout_secs(1))]
    #[case::production(false)]
    #[case::mock(true)]
    #[should_panic(expected = "::wait_for_packet` no progress for 1s")]
    async fn accounted_input_without_bytes_hits_pcm_watchdog(#[case] mock_wait: bool) {
        input_wait(mock_wait, false).await;
    }
}

#[cfg(feature = "flash")]
mod pcm_clock_contract {
    use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};

    use kithara_platform::{
        sync::mpsc,
        time::{Duration, Instant, WallInstant},
        tokio::sync::oneshot,
    };
    use kithara_signal::SegmentId;
    use kithara_test_utils::kithara;
    use ringbuf::traits::{Observer, Producer};

    use super::super::packet_fixture;
    use crate::{
        mock::pcm_fixture::chunk,
        worker::{
            PcmPacket,
            reader::{PcmReceiver, mock},
        },
    };

    enum WaitPath {
        Production,
        Mock,
        ActiveMock,
    }

    #[kithara::flash(true)]
    fn engine_now() -> Instant {
        Instant::now()
    }

    #[kithara::flash(true)]
    fn active_mock_wait(receiver: &PcmReceiver) {
        mock::wait_for_packet(receiver);
    }

    async fn modeled_wait(path: WaitPath, delay: Duration) -> (Duration, Duration) {
        let packet = chunk(SegmentId::FIRST, &[0.25; 8]);
        let (mut receiver, mut producer) = packet_fixture(true, packet.spec());
        let (start, started) = mpsc::channel();
        let (finished, completion) = oneshot::channel();
        let writer = kithara_platform::thread::spawn_named("pcm-clock-writer", move || {
            started.recv().expect("consumer started");
            kithara_platform::thread::sleep(delay);
            producer
                .forward
                .try_push(PcmPacket::Chunk(Box::new(packet)))
                .expect("known PCM fits the empty ring");
            producer.signal();
            let _ = finished.send(producer);
        });
        // The root poll and reserved pool credit hold time until the consumer parks.
        let outcome = kithara_platform::tokio::task::spawn_sync(move || {
            let began = engine_now();
            let wall_began = WallInstant::now();
            let result = catch_unwind(AssertUnwindSafe(|| {
                assert!(kithara_platform::flash::ambient_snapshot());
                assert!(receiver.peek().is_none());
                assert!(receiver.forward.write_is_held());
                start.send(()).expect("start modeled producer");
                match path {
                    WaitPath::Production => receiver.wait_for_packet(),
                    WaitPath::Mock => mock::wait_for_packet(&receiver),
                    WaitPath::ActiveMock => active_mock_wait(&receiver),
                }
                let elapsed = engine_now().duration_since(began);
                let wall_elapsed = wall_began.elapsed();
                let PcmPacket::Chunk(packet) = receiver.pop().expect("PCM, not producer closure")
                else {
                    panic!("expected PCM, not a failure packet");
                };
                assert_eq!(packet.spec().channels, 1);
                assert_eq!(packet.frames(), 8);
                assert!(!packet.samples.is_empty());
                assert!(packet.samples.iter().all(|sample| sample.is_finite()));
                assert_eq!(&packet.samples[..], &[0.25; 8]);
                assert!(!packet.meta.end_of_track);
                assert!(elapsed >= delay, "modeled PCM arrived after {elapsed:?}");
                assert!(wall_elapsed < Duration::from_secs(1));
                (elapsed, wall_elapsed)
            }));
            // Preserve the receiver when its watchdog fires before the writer.
            (receiver, result)
        })
        .await;
        let completed = completion.await;
        super::join_thread(move || writer.join()).await;
        let producer = completed.expect("modeled producer completion");
        let (receiver, result) = outcome.unwrap_or_else(|error| resume_unwind(error.into_panic()));
        drop((receiver, producer));
        result.unwrap_or_else(|panic| resume_unwind(panic))
    }

    #[kithara::test(native, tokio, hang_timeout_secs(1))]
    #[case::production(WaitPath::Production)]
    #[case::mock(WaitPath::Mock)]
    #[case::active_mock(WaitPath::ActiveMock)]
    async fn modeled_pcm_within_budget_reaches_wait(#[case] path: WaitPath) {
        modeled_wait(path, Duration::from_millis(500)).await;
    }

    #[kithara::test(native, tokio, hang_timeout_secs(1))]
    #[case::production(WaitPath::Production)]
    #[case::mock(WaitPath::Mock)]
    #[case::active_mock(WaitPath::ActiveMock)]
    #[should_panic(expected = "::wait_for_packet` no progress for 1s")]
    async fn modeled_pcm_after_budget_hits_watchdog(#[case] path: WaitPath) {
        let (elapsed, wall_elapsed) = modeled_wait(path, Duration::from_secs(2)).await;
        panic!(
            "PCM wait accepted nonterminal PCM after {elapsed:?} virtual time / {wall_elapsed:?} wall"
        );
    }
}
