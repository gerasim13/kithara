use kithara_bufpool::HasPool;
use kithara_command::{Mailbox, Post, Postbox};
use kithara_events::TrackId;
use kithara_play::{CrossfadeSettings, EqBandConfig, InterruptionKind, PlayError, player::Player};

use super::{Queue, Transition};
use crate::{
    error::QueueError,
    loading::LoadReport,
    navigation::{ActionAtItemEnd, PlaybackOrder, RepeatMode},
    track::TrackSource,
};

/// Where a [`QueueControl`](super::QueueControl) and the tasks beside track
/// loads post to the queue; each post is answered applied or refused with
/// the queue's error.
pub(crate) type QueuePostbox<S> = Postbox<QueueCommand<S>, QueueError>;

/// What the queue drains its posts from.
pub(crate) type QueueMailbox<S> = Mailbox<QueueCommand<S>, QueueError>;

/// What a [`QueueControl`](super::QueueControl) or a task beside a track's
/// load asks the queue to do. The executor that holds the queue runs commands
/// one at a time, in the order they were posted, and answers each one.
pub(crate) enum QueueCommand<S>
where
    S: HasPool<u8> + HasPool<f32> + Send + Sync + 'static,
{
    Append {
        id: TrackId,
        source: TrackSource<S>,
    },
    Insert {
        id: TrackId,
        source: TrackSource<S>,
        after: Option<TrackId>,
    },
    Remove(TrackId),
    Clear,
    SetTracks(Vec<TrackSource<S>>),
    Select {
        id: TrackId,
        transition: Transition,
    },
    Next(Transition),
    Previous(Transition),
    Play,
    Pause,
    Seek(f64),
    Tick,
    SetActionAtItemEnd(ActionAtItemEnd),
    SetPlaybackOrder(PlaybackOrder),
    SetRepeat(RepeatMode),
    SetCrossfadeSettings(CrossfadeSettings),
    /// A setting or notice the queue hands its player unchanged.
    Player(PlayerCall),
    Close,
    /// A task beside a track's load reports what it found.
    Load(LoadReport),
}

/// A player setting or platform notice the queue passes to its player.
pub(crate) enum PlayerCall {
    NotifyInterruption(InterruptionKind),
    ResetEq,
    SetDefaultRate(f32),
    SetEqGain { band: usize, gain_db: f32 },
    SetEqLayout(Vec<EqBandConfig>),
    SetLevel(f32),
    SetMuted(bool),
    SetRate(f32),
    SetVolume(f32),
}

impl<S> Queue<S>
where
    S: HasPool<u8> + HasPool<f32> + Send + Sync + 'static,
{
    /// Settles the loads the dispatcher answered and runs every command
    /// posted since the last drain, answering each, then publishes what they
    /// changed.
    pub(super) fn drain_commands(&mut self) {
        self.settle_loads();
        for Post { command, answer } in self.mailbox.drain() {
            answer.answer(self.run(command));
        }
        self.publish();
    }

    fn run(&mut self, command: QueueCommand<S>) -> Result<(), QueueError> {
        match command {
            QueueCommand::Append { id, source } => self.append_with_id(id, source).map(drop),
            QueueCommand::Insert { id, source, after } => {
                self.insert_with_id(id, source, after).map(drop)
            }
            QueueCommand::Remove(id) => self.remove(id),
            QueueCommand::Clear => self.clear(),
            QueueCommand::SetTracks(sources) => self.set_tracks(sources),
            QueueCommand::Select { id, transition } => self.select(id, transition),
            QueueCommand::Next(transition) => self.next(transition).map(drop),
            QueueCommand::Previous(transition) => self.previous(transition).map(drop),
            QueueCommand::Play => {
                self.play();
                Ok(())
            }
            QueueCommand::Pause => {
                self.pause();
                Ok(())
            }
            QueueCommand::Seek(seconds) => self.seek(seconds).map(drop),
            QueueCommand::Tick => self.tick(),
            QueueCommand::SetActionAtItemEnd(action) => {
                self.set_action_at_item_end(action);
                Ok(())
            }
            QueueCommand::SetPlaybackOrder(order) => {
                self.set_playback_order(order);
                Ok(())
            }
            QueueCommand::SetRepeat(mode) => {
                self.set_repeat(mode);
                Ok(())
            }
            QueueCommand::SetCrossfadeSettings(settings) => {
                Ok(self.set_crossfade_settings(settings)?)
            }
            QueueCommand::Player(call) => Ok(self.call_player(call)?),
            QueueCommand::Close => Ok(Player::close(self)?),
            QueueCommand::Load(report) => {
                self.tracks.apply_report(report);
                Ok(())
            }
        }
    }

    fn call_player(&self, call: PlayerCall) -> Result<(), PlayError> {
        self.ensure_open()?;
        let player = &self.player;
        match call {
            PlayerCall::NotifyInterruption(kind) => {
                player.notify_interruption(kind);
                Ok(())
            }
            PlayerCall::ResetEq => player.reset_eq(),
            PlayerCall::SetDefaultRate(rate) => player.set_default_rate(rate),
            PlayerCall::SetEqGain { band, gain_db } => player.set_eq_gain(band, gain_db),
            PlayerCall::SetEqLayout(layout) => player.set_eq_layout(layout),
            PlayerCall::SetLevel(level) => player.set_level(level),
            PlayerCall::SetMuted(muted) => player.set_muted(muted),
            PlayerCall::SetRate(rate) => player.set_rate(rate),
            PlayerCall::SetVolume(volume) => player.set_volume(volume),
        }
    }
}
