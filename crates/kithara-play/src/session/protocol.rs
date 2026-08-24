mod wire {
    use firewheel::FirewheelCtx;
    use kithara_audio::{EqBandConfig, SyncError};
    use kithara_bufpool::PcmPool;
    use kithara_events::EventBus;
    use kithara_platform::sync::mpsc;

    use crate::{
        api::{SessionBeat, SessionDuckingMode, SessionTransportSnapshot, SlotId, Tempo},
        bridge::{MixTapWriter, SlotControl},
        resource::{AssetMapRegistry, AssetMapRegistryError},
    };

    pub type PlayerId = u64;

    pub type StartStreamFn<B> =
        Box<dyn FnMut(&mut FirewheelCtx<B>, u32) -> Result<(), String> + Send>;

    #[derive(Debug, Clone, thiserror::Error)]
    #[non_exhaustive]
    pub enum SessionError {
        #[error("player not found: {0}")]
        PlayerNotFound(PlayerId),
        #[error("invalid session sample rate: {0}")]
        InvalidSampleRate(u32),
        #[error("player identity space is exhausted")]
        PlayerIdExhausted,
        #[error("player already started: {0}")]
        AlreadyStarted(PlayerId),
        #[error("player not running: {0}")]
        NotRunning(PlayerId),
        #[error("slot not found: {0:?}")]
        SlotNotFound(SlotId),
        #[error("session context not initialised")]
        NoContext,
        #[error("eq band out of range: {band} (bands: {bands})")]
        EqBandOutOfRange { band: usize, bands: usize },
        #[error("master volume {level} out of range for player {player_id}")]
        MasterVolumeOutOfRange { player_id: PlayerId, level: f32 },
        #[error("duplicate player in master volume batch: {0}")]
        DuplicatePlayer(PlayerId),
        #[error("stream start failed: {0}")]
        StreamStart(String),
        #[error("graph edit failed: {0}")]
        Graph(String),
        #[error("session mix tap already has a consumer")]
        MixTapActive,
        #[error("session transport has not been processed")]
        TransportNotProcessed,
        #[error("session transport commit was rejected at the render boundary")]
        TransportCommitRejected,
        #[error("session transport update failed: {0}")]
        TransportSync(String),
        #[error("session transport frame is exhausted")]
        TransportFrameExhausted,
        #[error("session transport revision is exhausted")]
        TransportRevisionExhausted,
        #[error(transparent)]
        Sync(#[from] SyncError),
        #[error(transparent)]
        BeatMapRegistry(#[from] AssetMapRegistryError),
        #[error("stream stopped: {reason}; restart failed: {source}")]
        RestartFailed { reason: String, r#source: String },
    }

    #[non_exhaustive]
    pub enum Cmd {
        RegisterPlayer {
            bus: EventBus,
            eq_layout: Vec<EqBandConfig>,
            pcm_pool: PcmPool,
            sample_rate: u32,
        },
        UnregisterPlayer {
            player_id: PlayerId,
        },
        StartPlayer {
            master_volume: f32,
            player_id: PlayerId,
            sample_rate: u32,
        },
        StopPlayer {
            player_id: PlayerId,
        },
        AllocateSlot {
            player_id: PlayerId,
        },
        ReleaseSlot {
            player_id: PlayerId,
            slot: SlotId,
        },
        SetPlayerMasterVolumes {
            levels: Vec<PlayerLevel>,
        },
        SetPlayerSlotVolume {
            player_id: PlayerId,
            slot: SlotId,
            volume: f32,
        },
        SetPlayerEqGain {
            band: usize,
            gain_db: f32,
            player_id: PlayerId,
        },
        SetPlayerEqLayout {
            eq_layout: Vec<EqBandConfig>,
            player_id: PlayerId,
        },
        EnableMixTap {
            writer: MixTapWriter,
        },
        DisableMixTap,
        SetSessionDucking {
            mode: SessionDuckingMode,
        },
        SessionDucking,
        SetSessionTempo {
            tempo: Tempo,
        },
        SetSessionPlaying {
            playing: bool,
        },
        SeekSession {
            target: SessionBeat,
        },
        QuerySessionTransport,
        QueryAssetMaps,
        InvalidateAudioRoute {
            reason: String,
        },
        QuerySampleRate,
        Tick,
    }

    pub struct CmdMsg {
        pub cmd: Cmd,
        pub reply_tx: mpsc::Sender<Reply>,
    }

    /// One player's session-input level in a batch update. `level` is a linear
    /// amplitude in `0.0..=1.0`.
    #[derive(Clone, Copy, Debug, PartialEq)]
    #[non_exhaustive]
    pub struct PlayerLevel {
        pub player_id: PlayerId,
        pub level: f32,
    }

    impl PlayerLevel {
        #[must_use]
        pub const fn new(player_id: PlayerId, level: f32) -> Self {
            Self { player_id, level }
        }
    }

    #[non_exhaustive]
    pub enum Reply {
        Ok,
        PlayerRegistered(PlayerId),
        SessionDucking(SessionDuckingMode),
        SessionTransport(SessionTransportSnapshot),
        AssetMaps(AssetMapRegistry),
        SlotAllocated(AllocatedSlot),
        SampleRate(SessionSampleRate),
        Err(SessionError),
    }

    /// What the session knows about its output rate.
    #[derive(Clone, Copy)]
    #[non_exhaustive]
    pub struct SessionSampleRate {
        /// The rate the output stream runs at, once a stream exists.
        pub measured: Option<u32>,
        /// The rate the session last asked the device for.
        pub requested: u32,
    }

    impl SessionSampleRate {
        #[must_use]
        pub const fn new(measured: Option<u32>, requested: u32) -> Self {
            Self {
                measured,
                requested,
            }
        }

        /// The rate to build a resampler for.
        #[must_use]
        pub const fn output(self) -> u32 {
            match self.measured {
                Some(measured) => measured,
                None => self.requested,
            }
        }
    }

    #[non_exhaustive]
    pub struct AllocatedSlot {
        pub control: SlotControl,
        pub slot: SlotId,
    }
}

mod handle {
    use kithara_audio::{ConsumerWakeMode, EqBandConfig};
    use kithara_bufpool::PcmPool;
    use kithara_events::EventBus;
    use kithara_platform::sync::Arc;

    use super::wire::{AllocatedSlot, Cmd, PlayerId, PlayerLevel, Reply, SessionSampleRate};
    use crate::{api::SlotId, error::PlayError, resource::AssetMapRegistry};

    pub trait SessionDispatcher: Send + Sync + 'static {
        fn exec(&self, cmd: Cmd) -> Result<Reply, PlayError>;

        /// Describe how PCM consumers hosted by this session may wake workers.
        fn consumer_wake_mode(&self) -> ConsumerWakeMode;

        fn exec_ok(&self, cmd: Cmd) -> Result<Reply, PlayError> {
            match self.exec(cmd)? {
                Reply::Err(err) => Err(PlayError::Session(err)),
                reply => Ok(reply),
            }
        }
    }

    #[derive(Clone)]
    pub struct SessionHandle(Arc<dyn SessionDispatcher>);

    impl SessionHandle {
        #[must_use]
        pub fn new(dispatcher: Arc<dyn SessionDispatcher>) -> Self {
            Self(dispatcher)
        }

        /// Returns the map namespace owned by this exact session endpoint.
        pub fn beat_maps(&self) -> Result<AssetMapRegistry, PlayError> {
            match self.exec_ok(Cmd::QueryAssetMaps)? {
                Reply::AssetMaps(registry) => Ok(registry),
                _ => Err(PlayError::Internal(
                    "unexpected reply for session asset maps query".into(),
                )),
            }
        }

        pub fn allocate_slot(&self, player_id: PlayerId) -> Result<AllocatedSlot, PlayError> {
            match self.exec_ok(Cmd::AllocateSlot { player_id })? {
                Reply::SlotAllocated(allocated) => Ok(allocated),
                _ => Err(PlayError::Internal(
                    "unexpected reply for session allocate slot".into(),
                )),
            }
        }

        #[must_use]
        pub fn dispatcher(&self) -> Arc<dyn SessionDispatcher> {
            Arc::clone(&self.0)
        }

        delegate::delegate! {
            to self.0 {
                #[must_use]
                pub fn consumer_wake_mode(&self) -> ConsumerWakeMode;
                pub fn exec(&self, cmd: Cmd) -> Result<Reply, PlayError>;
                pub fn exec_ok(&self, cmd: Cmd) -> Result<Reply, PlayError>;
            }
        }

        pub fn invalidate_audio_route(&self, reason: &str) -> Result<(), PlayError> {
            self.exec_ok(Cmd::InvalidateAudioRoute {
                reason: reason.to_owned(),
            })
            .map(|_| ())
        }

        pub fn sample_rate(&self) -> Result<SessionSampleRate, PlayError> {
            match self.exec_ok(Cmd::QuerySampleRate)? {
                Reply::SampleRate(sample_rate) => Ok(sample_rate),
                _ => Err(PlayError::Internal(
                    "unexpected reply for session sample rate query".into(),
                )),
            }
        }

        pub fn register_player(
            &self,
            bus: EventBus,
            eq_layout: Vec<EqBandConfig>,
            pcm_pool: PcmPool,
            sample_rate: u32,
        ) -> Result<PlayerId, PlayError> {
            match self.exec_ok(Cmd::RegisterPlayer {
                bus,
                eq_layout,
                pcm_pool,
                sample_rate,
            })? {
                Reply::PlayerRegistered(id) => Ok(id),
                _ => Err(PlayError::Internal(
                    "unexpected reply for session player registration".into(),
                )),
            }
        }

        pub fn release_slot(&self, player_id: PlayerId, slot: SlotId) -> Result<(), PlayError> {
            self.exec_ok(Cmd::ReleaseSlot { player_id, slot })
                .map(|_| ())
        }

        pub fn set_player_eq_gain(
            &self,
            player_id: PlayerId,
            band: usize,
            gain_db: f32,
        ) -> Result<(), PlayError> {
            self.exec_ok(Cmd::SetPlayerEqGain {
                band,
                gain_db,
                player_id,
            })
            .map(|_| ())
        }

        pub fn set_player_master_volumes(&self, levels: Vec<PlayerLevel>) -> Result<(), PlayError> {
            if levels.is_empty() {
                return Ok(());
            }
            self.exec_ok(Cmd::SetPlayerMasterVolumes { levels })
                .map(|_| ())
        }

        pub fn set_player_eq_layout(
            &self,
            player_id: PlayerId,
            eq_layout: Vec<EqBandConfig>,
        ) -> Result<(), PlayError> {
            self.exec_ok(Cmd::SetPlayerEqLayout {
                eq_layout,
                player_id,
            })
            .map(|_| ())
        }

        pub fn set_player_slot_volume(
            &self,
            player_id: PlayerId,
            slot: SlotId,
            volume: f32,
        ) -> Result<(), PlayError> {
            self.exec_ok(Cmd::SetPlayerSlotVolume {
                player_id,
                slot,
                volume,
            })
            .map(|_| ())
        }

        pub fn start_player(
            &self,
            player_id: PlayerId,
            sample_rate: u32,
            master_volume: f32,
        ) -> Result<(), PlayError> {
            self.exec_ok(Cmd::StartPlayer {
                master_volume,
                player_id,
                sample_rate,
            })
            .map(|_| ())
        }

        pub fn stop_player(&self, player_id: PlayerId) -> Result<(), PlayError> {
            self.exec_ok(Cmd::StopPlayer { player_id }).map(|_| ())
        }

        pub fn tick(&self) -> Result<(), PlayError> {
            self.exec_ok(Cmd::Tick).map(|_| ())
        }

        pub fn unregister_player(&self, player_id: PlayerId) -> Result<(), PlayError> {
            self.exec_ok(Cmd::UnregisterPlayer { player_id })
                .map(|_| ())
        }
    }
}

pub use handle::{SessionDispatcher, SessionHandle};
pub use wire::{
    AllocatedSlot, Cmd, CmdMsg, PlayerId, PlayerLevel, Reply, SessionError, SessionSampleRate,
    StartStreamFn,
};

#[cfg(test)]
mod tests {
    use std::num::NonZeroU32;

    use kithara_assets::{AssetSource, AssetStore};
    use kithara_audio::{AssetAxis, ConsumerWakeMode};
    use kithara_platform::sync::Arc;
    use kithara_test_utils::kithara;
    use url::Url;

    use super::{Cmd, Reply, SessionDispatcher, SessionHandle};
    use crate::{PlayError, session::testing::test_session};

    struct DefaultSession;

    impl SessionDispatcher for DefaultSession {
        fn exec(&self, _cmd: Cmd) -> Result<Reply, PlayError> {
            Ok(Reply::Ok)
        }

        fn consumer_wake_mode(&self) -> ConsumerWakeMode {
            ConsumerWakeMode::RealtimeDeferred
        }
    }

    #[kithara::test]
    fn session_handle_delegates_explicit_consumer_wake_mode() {
        let handle = SessionHandle::new(Arc::new(DefaultSession));

        assert_eq!(
            handle.consumer_wake_mode(),
            ConsumerWakeMode::RealtimeDeferred
        );
    }

    #[kithara::test]
    fn handles_wrapping_one_dispatcher_query_one_map_namespace() {
        struct TestAsset;

        let dispatcher = test_session();
        let first = SessionHandle::new(dispatcher.clone());
        let second = SessionHandle::new(dispatcher);
        let store = AssetStore::builder().build();
        let scope = store
            .scope::<TestAsset>(&AssetSource::Remote {
                url: Url::parse("https://example.com/track.wav")
                    .expect("invariant: fixture URL is valid"),
                discriminator: None,
            })
            .expect("invariant: fixture scope is valid");
        let axis = AssetAxis::new(
            NonZeroU32::new(48_000).expect("invariant: sample rate is non-zero"),
            96_000,
        );
        let first_registry = first
            .beat_maps()
            .expect("invariant: session exposes its registry");
        let second_registry = second
            .beat_maps()
            .expect("invariant: second handle reaches the same registry");
        let _registration = first_registry
            .map(&scope, axis)
            .expect("invariant: first map registration is valid");

        assert!(matches!(
            second_registry.map(&scope, axis),
            Err(crate::AssetMapRegistryError::PublisherClaimed)
        ));
    }
}
