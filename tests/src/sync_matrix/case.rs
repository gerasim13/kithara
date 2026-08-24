use std::fmt;

pub(super) const CHANNELS: u16 = 2;
pub(super) const COCHLEA_PHASE_SPREAD_BUDGET_FRAMES: u64 = 512;
pub(super) const MAX_LOCKED_PHASE_ERROR_FRAMES: u64 = 1;
pub(super) const RENDER_FRAMES: usize = 512;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum InitialDeckState {
    Paused,
    RunningStaggered,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Operation {
    Play,
    Seek,
    Sync,
}

impl Operation {
    #[must_use]
    pub(super) const fn label(self) -> &'static str {
        match self {
            Self::Play => "play",
            Self::Seek => "seek",
            Self::Sync => "sync",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum OperationOrder {
    PlaySyncSeek,
    PlaySeekSync,
    SeekPlaySync,
    SeekSyncPlay,
    SyncPlaySeek,
    SyncSeekPlay,
    SequentialSync,
}

impl OperationOrder {
    #[must_use]
    pub(super) const fn label(self) -> &'static str {
        match self {
            Self::PlaySyncSeek => "play-sync-seek",
            Self::PlaySeekSync => "play-seek-sync",
            Self::SeekPlaySync => "seek-play-sync",
            Self::SeekSyncPlay => "seek-sync-play",
            Self::SyncPlaySeek => "sync-play-seek",
            Self::SyncSeekPlay => "sync-seek-play",
            Self::SequentialSync => "sequential-sync",
        }
    }

    #[must_use]
    pub(super) const fn operations(self) -> &'static [Operation] {
        match self {
            Self::PlaySyncSeek | Self::SequentialSync => {
                &[Operation::Play, Operation::Sync, Operation::Seek]
            }
            Self::PlaySeekSync => &[Operation::Play, Operation::Seek, Operation::Sync],
            Self::SeekPlaySync => &[Operation::Seek, Operation::Play, Operation::Sync],
            Self::SeekSyncPlay => &[Operation::Seek, Operation::Sync, Operation::Play],
            Self::SyncPlaySeek => &[Operation::Sync, Operation::Play, Operation::Seek],
            Self::SyncSeekPlay => &[Operation::Sync, Operation::Seek, Operation::Play],
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum TempoRide {
    Down,
    Triangle,
    Up,
}

impl TempoRide {
    #[must_use]
    pub(super) const fn points(self) -> &'static [f64] {
        match self {
            Self::Down => &[116.0, 112.0, 108.0],
            Self::Triangle => &[116.0, 112.0, 116.0, 120.0],
            Self::Up => &[122.0, 125.0, 127.0],
        }
    }

    #[must_use]
    pub(super) const fn final_bpm(self) -> f64 {
        match self {
            Self::Down => 108.0,
            Self::Triangle => 120.0,
            Self::Up => 127.0,
        }
    }

    #[must_use]
    pub(super) const fn update_count(self, updates_hz: u32) -> usize {
        let updates_per_leg = if updates_hz / 2 == 0 {
            1
        } else {
            updates_hz / 2
        };
        self.points().len() * updates_per_leg as usize
    }
}

#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub struct SyncCase {
    pub(super) capture_beats: usize,
    pub(super) decks: usize,
    pub id: &'static str,
    pub(super) initial: InitialDeckState,
    pub(super) order: OperationOrder,
    pub(super) sample_rate: u32,
    pub(super) seek_seconds: f64,
    pub(super) session_bpm: f64,
    pub(super) stagger_beats: f64,
    pub(super) tempo_ride: TempoRide,
    pub(super) tempo_updates_hz: u32,
}

impl SyncCase {
    #[must_use]
    pub const fn running(
        id: &'static str,
        decks: usize,
        sample_rate: u32,
        order: OperationOrder,
    ) -> Self {
        Self {
            capture_beats: 6,
            decks,
            id,
            initial: InitialDeckState::RunningStaggered,
            order,
            sample_rate,
            seek_seconds: 5.25,
            session_bpm: 120.0,
            stagger_beats: 3.0 / 8.0,
            tempo_ride: TempoRide::Triangle,
            tempo_updates_hz: 60,
        }
    }

    #[must_use]
    pub const fn paused(
        id: &'static str,
        decks: usize,
        sample_rate: u32,
        order: OperationOrder,
    ) -> Self {
        Self {
            capture_beats: 6,
            decks,
            id,
            initial: InitialDeckState::Paused,
            order,
            sample_rate,
            seek_seconds: 5.25,
            session_bpm: 120.0,
            stagger_beats: 3.0 / 8.0,
            tempo_ride: TempoRide::Triangle,
            tempo_updates_hz: 60,
        }
    }

    #[must_use]
    pub const fn with_tempo_ride(mut self, ride: TempoRide, updates_hz: u32) -> Self {
        self.tempo_ride = ride;
        self.tempo_updates_hz = updates_hz;
        self
    }

    #[must_use]
    pub(super) fn capture_frames(self) -> usize {
        (f64::from(self.sample_rate) * 60.0 / self.session_bpm * self.capture_beats as f64).round()
            as usize
    }
}

impl fmt::Display for SyncCase {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} ({} decks, {} Hz, {})",
            self.id,
            self.decks,
            self.sample_rate,
            self.order.label(),
        )
    }
}
