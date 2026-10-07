use kithara_bufpool::HasPool;
use kithara_events::TrackId;
use kithara_platform::sync::mpsc::Sender;
use kithara_play::{
    CrossfadeSettings, EqBandConfig, InterruptionKind, PlayError, SeekOutcome, player::Player,
};

use super::{Queue, Transition};
use crate::{
    attempts::AttemptReport,
    error::QueueError,
    navigation::{ActionAtItemEnd, PlaybackOrder, RepeatMode},
    track::TrackSource,
};

/// Where the queue sends a command's answer; the caller waits on the other end.
pub(super) type Reply<T> = Sender<T>;

/// What a [`QueueControl`](super::QueueControl) or a load attempt asks the
/// queue to do. The executor that holds the queue runs commands one at a
/// time, in the order they were posted.
pub(crate) enum QueueCommand<S>
where
    S: HasPool<u8> + HasPool<f32> + Send + Sync + 'static,
{
    Append {
        id: TrackId,
        source: TrackSource<S>,
        reply: Reply<Result<TrackId, QueueError>>,
    },
    Insert {
        id: TrackId,
        source: TrackSource<S>,
        after: Option<TrackId>,
        reply: Reply<Result<TrackId, QueueError>>,
    },
    Remove {
        id: TrackId,
        reply: Reply<Result<(), QueueError>>,
    },
    Clear(Reply<()>),
    SetTracks {
        sources: Vec<TrackSource<S>>,
        reply: Reply<()>,
    },
    Select {
        id: TrackId,
        transition: Transition,
        reply: Reply<Result<(), QueueError>>,
    },
    Next {
        transition: Transition,
        reply: Reply<Result<Option<TrackId>, QueueError>>,
    },
    Previous {
        transition: Transition,
        reply: Reply<Result<Option<TrackId>, QueueError>>,
    },
    Play(Reply<()>),
    Pause(Reply<()>),
    Seek {
        seconds: f64,
        reply: Reply<Result<SeekOutcome, QueueError>>,
    },
    Tick(Reply<Result<(), QueueError>>),
    SetActionAtItemEnd {
        action: ActionAtItemEnd,
        reply: Reply<()>,
    },
    SetPlaybackOrder {
        order: PlaybackOrder,
        reply: Reply<()>,
    },
    SetRepeat {
        mode: RepeatMode,
        reply: Reply<()>,
    },
    SetCrossfadeSettings {
        settings: CrossfadeSettings,
        reply: Reply<Result<(), PlayError>>,
    },
    /// A setting or notice the queue hands its player unchanged.
    Player {
        call: PlayerCall,
        reply: Reply<Result<(), PlayError>>,
    },
    Close(Reply<Result<(), PlayError>>),
    /// A track's load attempt reports a transition.
    Attempt(AttemptReport),
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
    /// Runs every command posted since the last drain, then publishes what
    /// the reports among them changed.
    pub(super) fn drain_commands(&mut self) {
        for command in self.mailbox.drain() {
            self.run(command);
        }
        self.publish();
    }

    fn run(&mut self, command: QueueCommand<S>) {
        match command {
            QueueCommand::Append { id, source, reply } => {
                answer(&reply, self.append_with_id(id, source));
            }
            QueueCommand::Insert {
                id,
                source,
                after,
                reply,
            } => answer(&reply, self.insert_with_id(id, source, after)),
            QueueCommand::Remove { id, reply } => answer(&reply, self.remove(id)),
            QueueCommand::Clear(reply) => {
                self.clear();
                answer(&reply, ());
            }
            QueueCommand::SetTracks { sources, reply } => {
                self.set_tracks(sources);
                answer(&reply, ());
            }
            QueueCommand::Select {
                id,
                transition,
                reply,
            } => answer(&reply, self.select(id, transition)),
            QueueCommand::Next { transition, reply } => answer(&reply, self.next(transition)),
            QueueCommand::Previous { transition, reply } => {
                answer(&reply, self.previous(transition));
            }
            QueueCommand::Play(reply) => {
                self.play();
                answer(&reply, ());
            }
            QueueCommand::Pause(reply) => {
                self.pause();
                answer(&reply, ());
            }
            QueueCommand::Seek { seconds, reply } => answer(&reply, self.seek(seconds)),
            QueueCommand::Tick(reply) => answer(&reply, self.tick()),
            QueueCommand::SetActionAtItemEnd { action, reply } => {
                self.set_action_at_item_end(action);
                answer(&reply, ());
            }
            QueueCommand::SetPlaybackOrder { order, reply } => {
                self.set_playback_order(order);
                answer(&reply, ());
            }
            QueueCommand::SetRepeat { mode, reply } => {
                self.set_repeat(mode);
                answer(&reply, ());
            }
            QueueCommand::SetCrossfadeSettings { settings, reply } => {
                answer(&reply, self.set_crossfade_settings(settings));
            }
            QueueCommand::Player { call, reply } => answer(&reply, self.call_player(call)),
            QueueCommand::Close(reply) => answer(&reply, Player::close(self)),
            QueueCommand::Attempt(report) => self.apply_report(report),
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
            PlayerCall::SetDefaultRate(rate) => {
                player.set_default_rate(rate);
                Ok(())
            }
            PlayerCall::SetEqGain { band, gain_db } => player.set_eq_gain(band, gain_db),
            PlayerCall::SetEqLayout(layout) => player.set_eq_layout(layout),
            PlayerCall::SetLevel(level) => player.set_level(level),
            PlayerCall::SetMuted(muted) => {
                player.set_muted(muted);
                Ok(())
            }
            PlayerCall::SetRate(rate) => {
                player.set_rate(rate);
                Ok(())
            }
            PlayerCall::SetVolume(volume) => {
                player.set_volume(volume);
                Ok(())
            }
        }
    }
}

/// Sends `value` to the caller waiting on `reply`. A caller that stopped
/// waiting dropped its end, and the answer goes with it.
fn answer<T>(reply: &Reply<T>, value: T) {
    let _ = reply.send(value);
}
