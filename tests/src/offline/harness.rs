use kithara::{
    audio::{EqBandConfig, StretchControls},
    bufpool::Region,
    decode::GaplessMode,
    events::{Event, EventReceiver, PlayerEvent},
    platform::{
        sync::{Arc, Mutex},
        tokio::sync::broadcast::error::TryRecvError,
    },
    play::{PlayWorker, PlayWorkerConfig, PlayerConfig, PlayerImpl, SessionDispatcher},
};

use super::OfflineSession;

pub struct OfflinePlayerHarness {
    events: Mutex<EventReceiver>,
    player: Arc<PlayerImpl>,
    session: Arc<OfflineSession>,
}

#[derive(Clone, bon::Builder)]
pub struct OfflinePlayerOptions {
    #[builder(default = 1.0)]
    crossfade_duration: f32,
    eq_layout: Option<Vec<EqBandConfig>>,
    #[builder(default)]
    gapless_mode: GaplessMode,
    timestretch: Option<Arc<StretchControls>>,
}

impl OfflinePlayerHarness {
    pub fn with_sample_rate(options: OfflinePlayerOptions, sample_rate: u32) -> Self {
        let session = Arc::new(OfflineSession::new_manual());
        let session_dispatcher = Arc::clone(&session) as Arc<dyn SessionDispatcher>;
        let region = Region::default();
        let worker = PlayWorker::new(
            PlayWorkerConfig::for_pools(region.byte_pool(), region.pcm_pool()).build(),
        );
        let player_config = PlayerConfig::builder()
            .crossfade_duration(options.crossfade_duration)
            .gapless_mode(options.gapless_mode)
            .sample_rate(sample_rate)
            .session(Arc::clone(&session_dispatcher))
            .worker(worker)
            .maybe_eq_layout(options.eq_layout)
            .maybe_timestretch(options.timestretch)
            .build();

        let player = Arc::new(PlayerImpl::new(player_config));
        let events = player.subscribe();

        Self {
            events: Mutex::new(events),
            player,
            session,
        }
    }

    pub fn player(&self) -> &Arc<PlayerImpl> {
        &self.player
    }

    pub fn session(&self) -> &Arc<OfflineSession> {
        &self.session
    }

    /// Synchronously render `frames` of audio.
    pub fn render(&self, frames: usize) -> Vec<f32> {
        self.session.render(frames)
    }

    /// Pump the player's notification ringbuf and drain `PlayerEvent`s
    /// from the bus subscriber.
    pub fn tick_and_drain(&self) -> Vec<PlayerEvent> {
        self.player.process_notifications();

        let mut events = Vec::new();
        let mut rx = self.events.lock();
        loop {
            match rx.try_recv().map(|env| env.event) {
                Ok(Event::Player(event)) => events.push(event),
                Ok(_) | Err(TryRecvError::Lagged(_)) => continue,
                Err(TryRecvError::Empty | TryRecvError::Closed) => break,
            }
        }
        events
    }
}
