use kithara::{
    broadcast::{Broadcast, BroadcastConfig, BroadcastHandle, RingFeed},
    host::bridge::MixTapWriter,
};
use kithara_platform::{
    CancelToken,
    sync::{Arc, atomic::AtomicU64},
    time::Duration,
};
use ringbuf::{HeapRb, traits::Split};

use super::state::{BroadcastResult, Packager};
use crate::pools::AppHost;

pub(crate) struct Backend;

/// The running stream fed by the Host mix tap.
pub(crate) struct Stream {
    handle: BroadcastHandle,
}

trait BroadcastHost {
    fn measured_sample_rate(&self) -> BroadcastResult<Option<u32>>;
    fn enable_tap(&self, writer: MixTapWriter) -> BroadcastResult<()>;
    fn disable_tap(&self) -> BroadcastResult<()>;
}

impl BroadcastHost for AppHost {
    fn measured_sample_rate(&self) -> BroadcastResult<Option<u32>> {
        Ok(self.sample_rate()?.measured)
    }

    fn enable_tap(&self, writer: MixTapWriter) -> BroadcastResult<()> {
        self.enable_mix_tap(writer)?;
        Ok(())
    }

    fn disable_tap(&self) -> BroadcastResult<()> {
        self.disable_mix_tap()?;
        Ok(())
    }
}

impl Packager for Backend {
    type Live = Stream;

    const IS_AVAILABLE: bool = true;

    fn is_live(live: &Stream) -> bool {
        live.handle.status().is_live
    }

    fn start(
        host: &AppHost,
        shutdown: &CancelToken,
        tap_lead: Duration,
    ) -> BroadcastResult<Option<Stream>> {
        let Some(config) = measured_config(host)? else {
            return Ok(None);
        };
        start(host, shutdown, &config, tap_lead).map(Some)
    }

    fn release(host: &AppHost) -> BroadcastResult<()> {
        release(host)
    }

    fn stop(live: Stream) {
        live.handle.stop();
    }

    fn url(live: &Stream) -> &str {
        live.handle.url()
    }
}

struct Ring;

impl Ring {
    const MILLIS_PER_SECOND: usize = 1_000;

    /// Interleaved samples the mix tap may run ahead of the packager by.
    fn capacity(sample_rate: usize, channels: usize, lead: Duration) -> Option<usize> {
        let millis = usize::try_from(lead.as_millis()).ok()?;
        let frames = sample_rate.checked_mul(millis)? / Self::MILLIS_PER_SECOND;
        frames.checked_mul(channels)
    }
}

fn start<H: BroadcastHost>(
    host: &H,
    shutdown: &CancelToken,
    config: &BroadcastConfig,
    tap_lead: Duration,
) -> BroadcastResult<Stream> {
    let capacity = ring_capacity(config, tap_lead)?;
    let (producer, consumer) = HeapRb::<f32>::new(capacity).split();
    let drops = Arc::new(AtomicU64::new(0));

    host.enable_tap(MixTapWriter::new(producer, Arc::clone(&drops)))?;

    let feed = RingFeed::new(consumer, drops);
    match Broadcast::start(config, feed, Some(shutdown.child())) {
        Ok(handle) => Ok(Stream { handle }),
        Err(error) => {
            if let Err(disable_error) = release(host) {
                tracing::error!(%disable_error, "failed to release mix tap after broadcast startup failure");
            }
            Err(error.into())
        }
    }
}

fn release<H: BroadcastHost>(host: &H) -> BroadcastResult<()> {
    host.disable_tap()
}

fn measured_config<H: BroadcastHost>(host: &H) -> BroadcastResult<Option<BroadcastConfig>> {
    Ok(host
        .measured_sample_rate()?
        .map(|sample_rate| BroadcastConfig::builder().sample_rate(sample_rate).build()))
}

fn ring_capacity(config: &BroadcastConfig, tap_lead: Duration) -> BroadcastResult<usize> {
    let sample_rate = usize::try_from(config.sample_rate)?;
    if sample_rate == 0 {
        return Err("session returned zero sample rate".into());
    }

    let channels = usize::from(config.channels);
    if channels == 0 {
        return Err("broadcast configured with no channels".into());
    }

    match Ring::capacity(sample_rate, channels, tap_lead) {
        None => Err("broadcast ring capacity overflow".into()),
        Some(0) => Err(format!(
            "broadcast tap lead {tap_lead:?} holds no samples at {sample_rate} Hz"
        )
        .into()),
        Some(capacity) => Ok(capacity),
    }
}

#[cfg(test)]
mod tests {
    use kithara::{
        audio::ConsumerWakeMode,
        play::{Cmd, PlayError, Reply, SessionDispatcher, SessionHandle, SessionSampleRate},
    };
    use kithara_platform::{
        sync::{
            Mutex,
            atomic::{AtomicU32, Ordering},
        },
        thread,
    };
    use struct_patch::Patch as _;

    use super::*;
    use crate::{document::schema::Document, pools::AppPools};

    /// The lead every test that does not measure the ring starts on air with.
    const TAP_LEAD: Duration = Duration::from_secs(2);

    struct SampleRateSession {
        sample_rate: AtomicU32,
        tap: Mutex<Option<MixTapWriter>>,
    }

    impl SampleRateSession {
        /// Requested, not measured: the broadcast reads only the measured rate.
        const REQUESTED_RATE: u32 = 44_100;

        fn new(sample_rate: u32) -> Self {
            Self {
                sample_rate: AtomicU32::new(sample_rate),
                tap: Mutex::new(None),
            }
        }
    }

    impl SessionDispatcher<AppPools> for SampleRateSession {
        fn consumer_wake_mode(&self) -> ConsumerWakeMode {
            ConsumerWakeMode::RealtimeDeferred
        }

        fn exec(&self, cmd: Cmd<AppPools>) -> Result<Reply, PlayError> {
            match cmd {
                Cmd::QuerySampleRate => {
                    let sample_rate = self.sample_rate.load(Ordering::Relaxed);
                    Ok(Reply::SampleRate(SessionSampleRate::new(
                        (sample_rate != 0).then_some(sample_rate),
                        Self::REQUESTED_RATE,
                    )))
                }
                Cmd::EnableMixTap { writer } => {
                    *self.tap.lock() = Some(writer);
                    Ok(Reply::Ok)
                }
                Cmd::DisableMixTap => {
                    self.tap.lock().take();
                    Ok(Reply::Ok)
                }
                _ => panic!("unexpected session command"),
            }
        }
    }

    impl BroadcastHost for SessionHandle<AppPools> {
        fn measured_sample_rate(&self) -> BroadcastResult<Option<u32>> {
            Ok(self.sample_rate()?.measured)
        }

        fn enable_tap(&self, writer: MixTapWriter) -> BroadcastResult<()> {
            self.exec_ok(Cmd::EnableMixTap { writer })?;
            Ok(())
        }

        fn disable_tap(&self) -> BroadcastResult<()> {
            self.exec_ok(Cmd::DisableMixTap)?;
            Ok(())
        }
    }

    fn on_air(sample_rate: u32) -> (Stream, Arc<SampleRateSession>, CancelToken) {
        let dispatcher = Arc::new(SampleRateSession::new(sample_rate));
        let session = SessionHandle::new(dispatcher.clone());
        let shutdown = CancelToken::root();
        let config = measured_config(&session)
            .expect("sample-rate query")
            .expect("a measured rate yields a config");
        let stream = start(&session, &shutdown, &config, TAP_LEAD).expect("the packager starts");
        (stream, dispatcher, shutdown)
    }

    #[kithara::test]
    fn configuration_waits_for_the_measured_session_sample_rate() {
        let dispatcher = Arc::new(SampleRateSession::new(0));
        let session = SessionHandle::new(dispatcher.clone());

        assert!(measured_config(&session).unwrap().is_none());
        assert!(
            measured_config(&session).unwrap().is_none(),
            "an unmeasured rate is a retry, not a failure"
        );

        dispatcher.sample_rate.store(48_000, Ordering::Relaxed);
        let config = measured_config(&session)
            .unwrap()
            .expect("measured sample rate");

        assert_eq!(config.sample_rate, 48_000);
    }

    #[kithara::test]
    fn starting_takes_the_mix_tap_and_stopping_gives_it_back() {
        let (stream, dispatcher, shutdown) = on_air(48_000);
        assert!(
            dispatcher.tap.lock().is_some(),
            "the running stream holds the session's mix tap"
        );

        release(&SessionHandle::new(dispatcher.clone())).expect("release mix tap");
        Backend::stop(stream);

        assert!(
            dispatcher.tap.lock().is_none(),
            "the drained stream returns the mix tap"
        );
        shutdown.cancel();
    }

    #[kithara::test]
    fn a_dropped_producer_ends_the_stream() {
        let (stream, dispatcher, shutdown) = on_air(48_000);
        dispatcher.tap.lock().take();

        for _ in 0..1_000 {
            if !Backend::is_live(&stream) {
                break;
            }
            thread::paced_backoff(Duration::from_millis(1));
        }

        assert!(!Backend::is_live(&stream));
        shutdown.cancel();
    }

    #[kithara::test]
    fn missing_session_sample_rate_is_rejected_before_ring_creation() {
        assert!(
            ring_capacity(&BroadcastConfig::builder().sample_rate(0).build(), TAP_LEAD).is_err()
        );
    }

    /// The ring carries interleaved samples, so its size has to follow the
    /// channel count the broadcast is configured for. Sizing it for a fixed
    /// stereo pair starves a wider mix of half its lead.
    #[kithara::test]
    fn the_ring_is_sized_for_the_configured_channel_count() {
        let stereo = ring_capacity(&BroadcastConfig::builder().channels(2).build(), TAP_LEAD)
            .expect("a stereo ring");
        let quad = ring_capacity(&BroadcastConfig::builder().channels(4).build(), TAP_LEAD)
            .expect("a quad ring");

        assert_eq!(quad, stereo * 2);
    }

    /// The lead is what the ring buys: how long the packager may stall before
    /// the mix tap starts dropping samples. Asking for twice the lead has to
    /// yield twice the ring, or the knob does not mean what it says.
    #[kithara::test]
    fn a_longer_tap_lead_buys_a_proportionally_deeper_ring() {
        let config = BroadcastConfig::builder().build();

        let short = ring_capacity(&config, TAP_LEAD).expect("a ring for the default lead");
        let long = ring_capacity(&config, TAP_LEAD * 2).expect("a ring for twice the lead");

        assert_eq!(long, short * 2);
    }

    /// A lead below one sample rounds down to an empty ring, which would take
    /// the mix tap's every write. That is a configuration error, not a ring.
    #[kithara::test]
    fn a_tap_lead_too_short_to_hold_a_sample_is_rejected() {
        let config = BroadcastConfig::builder().build();

        assert!(ring_capacity(&config, Duration::ZERO).is_err());
    }

    /// A document that names `broadcast: {bit_rate: ...}` must parse under
    /// the real configuration document schema, and the patch it carries must
    /// reach a `BroadcastConfig` without disturbing fields the document left
    /// unnamed.
    #[kithara::test]
    fn the_broadcast_section_parses_and_reaches_the_config() {
        let document: Document = serde_yaml_ng::from_str("broadcast:\n  bit_rate: 256000\n")
            .expect("the document types");
        let mut config = BroadcastConfig::builder().channels(4).build();

        config.apply(document.broadcast);

        assert_eq!(config.bit_rate, 256_000);
        assert_eq!(
            config.channels, 4,
            "a document naming only bit_rate must not reset the seeded channel count"
        );
    }
}
