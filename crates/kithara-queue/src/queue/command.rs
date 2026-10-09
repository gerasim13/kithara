use kithara_bufpool::HasPool;
use kithara_command::{Mailbox, Postbox, Seq, When};
use kithara_config::LiveConfig;
use kithara_events::TrackId;
use kithara_play::{
    EqBandConfig, InterruptionKind, Outbox, OutputSnapshot, PlayError, Player, Position, TrackCommand,
    TrackFactory, TrackSettings, TrackSettingsChange,
};
use kithara_signal::SessionFrame;

use super::{Queue, Transition, types::Placement};
use crate::{
    ActionAtItemEnd, AdvanceReason, PlaybackOrder, QueueError, QueueEvent, QueueRepeatMode,
    QueueSettingsChange, RepeatMode, TrackSource, TrackStatus, loading::LoadReport,
};

pub(crate) type QueuePostbox<S> = Postbox<QueueCommand<S>, QueueError>;
pub(super) type QueueMailbox<S> = Mailbox<QueueCommand<S>, QueueError>;

/// Commands addressed to stable queue item identities, not list positions.
pub enum QueueCommand<S>
where
    S: HasPool<u8> + Send + Sync + 'static,
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
    RemoveAll,
    SetTracks(Vec<TrackSource<S>>),
    Select {
        id: TrackId,
        transition: Transition,
    },
    Next(Transition),
    Previous(Transition),
    Play {
        at: When<SessionFrame>,
    },
    Pause {
        at: When<SessionFrame>,
    },
    Seek {
        to: Position,
    },
    ConfigureTrack(TrackSettingsChange, When<SessionFrame>),
    ConfigureQueue(QueueSettingsChange, When<SessionFrame>),
    SetActionAtItemEnd(ActionAtItemEnd),
    SetPlaybackOrder(PlaybackOrder),
    SetRepeat(RepeatMode),
    SetVolume(f32),
    SetLevel(f32),
    SetMuted(bool),
    SetEqGain {
        band: usize,
        gain_db: f32,
    },
    SetEqLayout(Vec<EqBandConfig>),
    ResetEq,
    NotifyInterruption(InterruptionKind),
    Tick,
    Close,
    #[doc(hidden)]
    Load(LoadReport),
}

impl<S, F> Queue<S, F>
where
    S: HasPool<u8> + HasPool<f32> + Send + Sync + 'static,
    F: TrackFactory<S>,
{
    pub(super) fn validate_command(&self, command: &QueueCommand<S>) -> Result<(), QueueError> {
        self.ensure_open()?;
        if let QueueCommand::Select { transition, .. } = command {
            transition
                .settings(self.config.settings.crossfade())
                .validate()
                .map_err(PlayError::from)?;
        }
        let id = match command {
            QueueCommand::Select { id, .. } | QueueCommand::Remove(id) => Some(*id),
            QueueCommand::Insert { after, .. } => *after,
            _ => None,
        };
        if let Some(id) = id {
            let record = self
                .tracks
                .records()
                .iter()
                .find(|record| record.id == id)
                .ok_or(QueueError::UnknownTrackId(id))?;
            if matches!(command, QueueCommand::Select { .. })
                && matches!(record.status, TrackStatus::Failed(_))
            {
                return Err(QueueError::NotReady(id));
            }
        }
        Ok(())
    }

    pub(super) fn apply_command(
        &mut self,
        command: QueueCommand<S>,
        output: Option<&OutputSnapshot>,
        out: &mut Outbox<'_, S>,
    ) -> Result<Option<Seq>, QueueError> {
        self.validate_command(&command)?;
        match command {
            QueueCommand::Append { id, source } => {
                self.insert_entry(id, source, Placement::Append);
                self.autoplay(output, out)?;
                Ok(None)
            }
            QueueCommand::Insert { id, source, after } => {
                let index = match after {
                    Some(after) => {
                        self.tracks
                            .records()
                            .iter()
                            .position(|record| record.id == after)
                            .ok_or(QueueError::UnknownTrackId(after))?
                            + 1
                    }
                    None => 0,
                };
                self.insert_entry(id, source, Placement::At(index));
                self.autoplay(output, out)?;
                Ok(None)
            }
            QueueCommand::Remove(id) => self.remove_entry(id, output, out),
            QueueCommand::RemoveAll => self.clear_entries(out),
            QueueCommand::SetTracks(sources) => {
                self.clear_entries(out)?;
                for source in sources {
                    self.insert_entry(TrackId::allocate(), source, Placement::Append);
                }
                self.autoplay(output, out)?;
                Ok(None)
            }
            QueueCommand::Select { id, transition } => {
                self.request_transition(id, transition, AdvanceReason::UserSelect, false, output, out)
            }
            QueueCommand::Next(transition) => {
                self.next_target(transition, AdvanceReason::UserNext, false, output, out)
            }
            QueueCommand::Previous(transition) => {
                let ids = self.track_ids();
                match self.navigation.prev(&ids) {
                    Some(id) => {
                        self.request_transition(id, transition, AdvanceReason::UserPrev, false, output, out)
                    }
                    None => Ok(None),
                }
            }
            QueueCommand::Play { at } => {
                if self.active_current_index().is_some() {
                    self.transport(TrackCommand::Play { at }, out)
                        .map_err(Into::into)
                } else if self.target.is_some() {
                    self.target.as_mut().ok_or(PlayError::NotReady)?.playing = true;
                    self.transition_loaded(out).map_err(Into::into)
                } else {
                    self.next_target(Transition::None, AdvanceReason::InitialLoad, false, output, out)
                }
            }
            QueueCommand::Pause { at } => {
                if self.active_current_index().is_none() && self.target.is_some() {
                    self.withdraw_transition(out)?;
                    self.target.as_mut().ok_or(PlayError::NotReady)?.playing = false;
                    return self.transition_loaded(out).map_err(Into::into);
                }
                self.cancel_auto(out)?;
                self.transport(TrackCommand::Pause { at }, out)
                    .map_err(Into::into)
            }
            QueueCommand::Seek { to } => {
                let index = self.active_current_index().or_else(|| {
                    self.target.and_then(|target| self.incoming_index(target.to))
                });
                let Some(index) = index else {
                    self.held_position = Some(to);
                    return Ok(None);
                };
                let sent = self
                    .active
                    .get_mut(index)
                    .ok_or(PlayError::NoActiveSlot)?
                    .track
                    .apply(TrackCommand::Seek { to }, out)?;
                self.withdraw_auto(out)?;
                Ok(sent)
            }
            QueueCommand::ConfigureTrack(change, at) => {
                self.configure_tracks(change, at, out).map_err(Into::into)
            }
            QueueCommand::ConfigureQueue(change, at) => {
                if matches!(at, When::At(_)) {
                    return Err(PlayError::Untimed.into());
                }
                let change = match change {
                    QueueSettingsChange::Crossfade(settings) => QueueSettingsChange::Crossfade(
                        settings.validate().map_err(PlayError::from)?,
                    ),
                    change => change,
                };
                self.config.settings.apply_change(change);
                if let QueueSettingsChange::Crossfade(settings) = change {
                    self.announce(QueueEvent::CrossfadeSettingsChanged { settings });
                }
                self.withdraw_transition(out)?;
                Ok(None)
            }
            QueueCommand::SetActionAtItemEnd(action) => {
                self.config.action_at_item_end = action;
                self.announce(QueueEvent::ActionAtItemEndChanged { action });
                self.cancel_auto(out)?;
                Ok(None)
            }
            QueueCommand::SetPlaybackOrder(order) => {
                let ids = self.track_ids();
                self.navigation.set_playback_order(order, &ids);
                self.config.playback_order = order;
                self.announce(QueueEvent::PlaybackOrderChanged { order });
                self.cancel_auto(out)?;
                Ok(None)
            }
            QueueCommand::SetRepeat(mode) => {
                self.navigation.set_repeat(mode);
                let mode = match mode {
                    RepeatMode::Off => QueueRepeatMode::Off,
                    RepeatMode::One => QueueRepeatMode::One,
                    RepeatMode::All => QueueRepeatMode::All,
                };
                self.announce(QueueEvent::RepeatModeChanged { mode });
                self.cancel_auto(out)?;
                Ok(None)
            }
            QueueCommand::Load(report) => {
                self.tracks.apply_report(report);
                Ok(None)
            }
            QueueCommand::Tick => {
                if let Some((now, _)) = self.clock {
                    Player::tick(self, now, out);
                }
                Ok(None)
            }
            QueueCommand::Close => self.close_tracks(out).map_err(Into::into),
            QueueCommand::SetVolume(_)
            | QueueCommand::SetLevel(_)
            | QueueCommand::SetMuted(_)
            | QueueCommand::SetEqGain { .. }
            | QueueCommand::SetEqLayout(_)
            | QueueCommand::ResetEq
            | QueueCommand::NotifyInterruption(_) => self.forward_host(command),
        }
    }

    pub(super) fn transport(
        &mut self,
        command: TrackCommand<S>,
        out: &mut Outbox<'_, S>,
    ) -> Result<Option<Seq>, PlayError> {
        let index = self.active_current_index().ok_or(PlayError::NoActiveSlot)?;
        self.active
            .get_mut(index)
            .ok_or(PlayError::NoActiveSlot)?
            .track
            .apply(command, out)
    }

    fn configure_tracks(
        &mut self,
        change: TrackSettingsChange,
        at: When<SessionFrame>,
        _out: &mut Outbox<'_, S>,
    ) -> Result<Option<Seq>, PlayError> {
        let change = <TrackSettings as LiveConfig>::check(change)?;
        if let When::At(frame) = at {
            if !self.current_track().is_some_and(|track| {
                matches!(
                    track.snapshot().as_ref().status,
                    kithara_play::TrackStatus::Playing { .. }
                )
            }) {
                return Err(PlayError::Untimed);
            }
            let earliest = self
                .current_track()
                .ok_or(PlayError::Untimed)?
                .entry(kithara_play::Bound::AtOrAfter(self.earliest()?))
                .ok_or(PlayError::Untimed)?;
            if frame < earliest {
                return Err(PlayError::Late);
            }
        }
        if self.active.len() == 0 {
            self.config.track.apply_change(change);
            return Ok(None);
        }
        todo!(
            "Broadcast TrackSettings once: preflight every lane's available room and reachable frame, send nothing on Untimed/Late/Full, send sounding tracks at the requested time and silent tracks on Next, join all receipts, then withdraw an automatic transition whose A_end moved (spec 4.4)"
        )
    }

    fn forward_host(&mut self, _command: QueueCommand<S>) -> Result<Option<Seq>, QueueError> {
        todo!(
            "kithara-host command postbox and published deck mix/EQ owner bridge for preserved queue facade forwarding and joined receipts (contract §8.4; skeleton queue)"
        )
    }
}

pub(super) fn play_error(error: QueueError) -> PlayError {
    match error {
        QueueError::Play(error) => error,
        QueueError::UnknownTrackId(item) => PlayError::ItemConsumed { item },
        QueueError::NotReady(_) => PlayError::NotReady,
        error => PlayError::ItemFailed {
            reason: error.to_string(),
        },
    }
}
