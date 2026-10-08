//! One loaded track: one render lane in the worker and one slot of its deck's mixer.

use std::marker::PhantomData;

use kithara_abr::AbrHandle;
use kithara_command::{Batch, Live, LiveError, Outcome, Receipt, Rejection, SendError, Sender, Seq, When};
use kithara_config::ConfigOwner;
use kithara_decode::TrackMetadata;
use kithara_events::TrackId;
use kithara_platform::time::Duration;
use kithara_render::{
    CrossfadeSettings, DispatcherProtocol, LaneCommand, LaneProtocol,
    bridge::{PlaybackFault, DeckEvent, DeckPart, DeckProtocol, Fade, FadeDir, Released, Slot},
};
use kithara_signal::SessionFrame;
use kithara_warp::SpeedCurve;
use tracing::warn;

use super::{
    factory::Track,
    outbox::{Bound, Outbox, Player, Settled, TrackReceipt, rejection},
    settings::{PlayerConfig, TrackSettings, TrackSettingsChange, TrackSettingsExec},
};
use crate::{OpenedTrack, PlayError, ResourceLoad};

/// A position in a track's media.
pub type Position = Duration;

/// What a track is told to do.
pub enum TrackCommand<S> {
    /// Open `item` through the dispatcher, the track's one open; once it
    /// opened, the track attaches its consumer to its slot on the next block,
    /// standing at `position`.
    Load {
        item: ResourceLoad<S>,
        position: Position,
    },
    /// Let the slot sound from `at`, entering with the mixer's declick.
    Play { at: When<SessionFrame> },
    /// Stop the slot at `at`; the position it stopped at comes in the receipt.
    Pause { at: When<SessionFrame> },
    /// Move the slot's track to `to` on the next block.
    Seek { to: Position },
    /// Jump within the loaded segment to `to` on `at` (phase jump from SYNC), with the mixer's declick.
    Jump { to: Position, at: SessionFrame },
    /// Change one setting; a moment the track has no lane frame for is
    /// refused as untimed.
    Configure(TrackSettingsChange, When<SessionFrame>),
    /// Play the lane along `speed`.
    SetSpeed {
        speed: SpeedCurve,
        at: When<SessionFrame>,
    },
    /// Ramp the slot along one half of `settings` from `at`; a silent track
    /// fading in starts there.
    Fade {
        at: When<SessionFrame>,
        settings: CrossfadeSettings,
        dir: FadeDir,
    },
    /// Start on the frame after the last of the track in `track`.
    PlayAfter { track: Slot },
    /// Take the track out of its slot; its lane closes once the mixer let go.
    Release,
    /// Take the slot over from the track that holds it at `at` once this one
    /// opened: the old one's tail fades out and this one plays on from there.
    Evict { at: When<SessionFrame> },
}

/// Where a track stands, as the receipts of its executors left it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TrackStatus {
    /// Nothing loaded.
    Idle,
    /// The dispatcher is opening the track's source, or the deck has not yet
    /// put it in its slot.
    Loading,
    /// In its slot, silent.
    Loaded,
    Playing { since: SessionFrame },
    Paused { at: Position },
    /// Faded to silence on `at`; the slot stopped.
    Faded { at: SessionFrame },
    /// Played to its natural end marker on `at`.
    Ended { at: SessionFrame },
    Failed { at: SessionFrame, fault: PlaybackFault },
    /// Out of its slot.
    Released,
}

/// What a track shows of itself.
#[derive(Clone, Debug)]
pub struct TrackSnapshot {
    pub item: TrackId,
    pub slot: Slot,
    pub status: TrackStatus,
    /// The speed its lane applied.
    pub speed: f32,
    /// Where it stood on the last receipt that placed it.
    pub position: Position,
    pub duration: Option<Duration>,
    pub abr: Option<AbrHandle>,
    /// The tags its decoder read.
    pub metadata: TrackMetadata,
}

impl AsRef<TrackSnapshot> for TrackSnapshot {
    fn as_ref(&self) -> &TrackSnapshot {
        self
    }
}

/// The open in flight and what the track does once it opened.
struct Loading {
    seq: Seq,
    position: Position,
    evict: Option<When<SessionFrame>>,
}

/// The one implementation of [`Player`] for a single track.
pub struct PlayerImpl<S> {
    item: TrackId,
    slot: Slot,
    lane: Option<Sender<LaneProtocol>>,
    settings: Live<TrackSettings, LaneProtocol>,
    status: TrackStatus,
    loading: Option<Loading>,
    /// The deck batch that puts the opened track in its slot, and the load it
    /// answers.
    attaching: Option<(Seq, Seq)>,
    position: Position,
    duration: Option<Duration>,
    abr: Option<AbrHandle>,
    metadata: TrackMetadata,
    seek_epoch: u64,
    _schema: PhantomData<fn() -> S>,
}

impl<S> PlayerImpl<S> {
    /// A track not yet loaded.
    ///
    /// # Errors
    ///
    /// Returns the settings check's refusal.
    pub fn new(config: PlayerConfig) -> Result<Self, PlayError> {
        Ok(Self {
            item: config.item,
            slot: config.slot,
            lane: None,
            settings: Live::new(config.settings)?,
            status: TrackStatus::Idle,
            loading: None,
            attaching: None,
            position: Position::ZERO,
            duration: None,
            abr: None,
            metadata: TrackMetadata::default(),
            seek_epoch: 0,
            _schema: PhantomData,
        })
    }

    /// Where the track stands.
    #[must_use]
    pub fn status(&self) -> TrackStatus {
        self.status
    }

    /// Whether the track stands in its slot.
    #[must_use]
    pub fn attached(&self) -> bool {
        !matches!(
            self.status,
            TrackStatus::Idle | TrackStatus::Loading | TrackStatus::Released
        )
    }

    fn deck(
        &self,
        at: When<SessionFrame>,
        parts: Vec<DeckPart>,
        out: &mut Outbox<'_, S>,
    ) -> Result<Option<Seq>, PlayError> {
        if !self.attached() {
            return Err(PlayError::NotReady);
        }
        out.deck(at, parts)
    }

    fn load(
        &mut self,
        mut item: ResourceLoad<S>,
        position: Position,
        out: &mut Outbox<'_, S>,
    ) -> Result<Option<Seq>, PlayError> {
        if self.status != TrackStatus::Idle {
            return Err(PlayError::Internal(format!(
                "a track loads once; it stands {:?}",
                self.status
            )));
        }
        item.start_at(*self.settings.config());
        let seq = out.load(item)?;
        self.loading = Some(Loading {
            seq,
            position,
            evict: None,
        });
        self.status = TrackStatus::Loading;
        Ok(Some(seq))
    }

    /// Takes in what the dispatcher opened and puts it in the slot.
    fn opened(
        &mut self,
        receipt: Receipt<DispatcherProtocol<ResourceLoad<S>>>,
        out: &mut Outbox<'_, S>,
    ) -> Settled {
        let seq = receipt.seq();
        let Some(loading) = self.loading.take_if(|loading| loading.seq == seq) else {
            return Settled::Pending;
        };
        let (outcome, _) = receipt.into();
        let opened = match outcome {
            Outcome::Applied { data, .. } => data,
            Outcome::Rejected(refused) => {
                self.status = TrackStatus::Idle;
                return Settled::Rejected {
                    seq,
                    reason: rejection(&refused, |refusal| PlayError::ItemFailed {
                        reason: refusal.to_string(),
                    }),
                };
            }
        };
        let OpenedTrack {
            pcm,
            lane,
            duration,
            abr,
            metadata,
        } = opened;
        self.lane = lane;
        self.duration = duration;
        self.abr = abr;
        self.metadata = metadata;
        self.position = loading.position;
        let slot = self.slot;
        let (at, mut parts) = match loading.evict {
            Some(at) => (
                at,
                vec![
                    DeckPart::Replace { slot, pcm },
                    DeckPart::Start {
                        slot,
                        fade: Fade::Declick,
                    },
                ],
            ),
            None => (When::Next, vec![DeckPart::Attach { slot, pcm }]),
        };
        if !loading.position.is_zero() {
            self.seek_epoch += 1;
            parts.push(DeckPart::Seek {
                slot,
                seconds: loading.position.as_secs_f64(),
                seek_epoch: self.seek_epoch,
            });
        }
        let refused = match out.deck(at, parts) {
            Ok(Some(attach)) => {
                self.attaching = Some((attach, seq));
                return Settled::Pending;
            }
            Ok(None) => PlayError::Internal("a track's attach joined a batch it does not own".into()),
            Err(error) => error,
        };
        self.status = TrackStatus::Idle;
        self.lane = None;
        Settled::Rejected {
            seq,
            reason: Rejection::Refused(refused),
        }
    }

    /// Takes in a deck receipt whose basis names this track's slot. Once the
    /// slot let this track go, what the batch holds after belongs to the next
    /// track there.
    fn applied(&mut self, receipt: &Receipt<DeckProtocol>) -> Settled {
        if self.status == TrackStatus::Released {
            return Settled::Pending;
        }
        let seq = receipt.seq();
        let attaching = self.attaching.filter(|&(attach, _)| attach == seq);
        let answered = attaching.map_or(seq, |(_, load)| load);
        if attaching.is_some() {
            self.attaching = None;
        }
        let (at, data) = match receipt.outcome() {
            Outcome::Applied { at, data } => (*at, *data),
            Outcome::Rejected(refused) => {
                if attaching.is_some() {
                    self.status = TrackStatus::Idle;
                    self.lane = None;
                }
                return Settled::Rejected {
                    seq: answered,
                    reason: rejection(refused, |refusal| PlayError::Deck(*refusal)),
                };
            }
        };
        if attaching.is_some() {
            self.status = TrackStatus::Loaded;
        }
        let slot = self.slot;
        for part in &receipt.batch().commands {
            match *part {
                DeckPart::Start { slot: named, .. } | DeckPart::Chain { to: named, .. }
                    if named == slot =>
                {
                    self.status = TrackStatus::Playing { since: at };
                }
                DeckPart::Stop { slot: named, .. } if named == slot => {
                    if let Some(seconds) = data.stopped_at {
                        self.position = Position::from_secs_f64(seconds.max(0.0));
                    }
                    self.status = TrackStatus::Paused { at: self.position };
                }
                DeckPart::Seek {
                    slot: named,
                    seconds,
                    ..
                } if named == slot => {
                    self.position = Position::from_secs_f64(seconds.max(0.0));
                }
                DeckPart::Released(Released::Pcm { slot: named, .. })
                    if named == slot && attaching.is_none() =>
                {
                    self.status = TrackStatus::Released;
                    self.lane = None;
                    break;
                }
                _ => {}
            }
        }
        Settled::Applied { seq: answered, at }
    }

    /// Takes in an event of this track's slot.
    fn event(&mut self, event: DeckEvent) {
        if !self.attached() || matches!(self.status, TrackStatus::Failed { .. }) {
            return;
        }
        match event {
            DeckEvent::Ended { at, .. } => self.status = TrackStatus::Ended { at },
            DeckEvent::Failed { at, fault, .. } => {
                if let TrackStatus::Playing { since } = self.status
                    && at.frames_since(since).is_some()
                {
                    self.status = TrackStatus::Failed { at, fault };
                }
            }
            DeckEvent::Faded { at, .. } => self.status = TrackStatus::Faded { at },
            DeckEvent::Underrun { at, frames, .. } => {
                warn!(item = ?self.item, ?at, frames, "track underran");
            }
        }
    }

    fn fade(
        &self,
        at: When<SessionFrame>,
        settings: CrossfadeSettings,
        dir: FadeDir,
        out: &mut Outbox<'_, S>,
    ) -> Result<Option<Seq>, PlayError> {
        let slot = self.slot;
        let part = match (self.status, dir) {
            (TrackStatus::Playing { .. }, _) => DeckPart::Fade {
                slot,
                settings,
                dir,
            },
            (_, FadeDir::In) => DeckPart::Start {
                slot,
                fade: Fade::Crossfade(settings),
            },
            (_, FadeDir::Out) => return Ok(None),
        };
        self.deck(at, vec![part], out)
    }

    fn release(&mut self, out: &mut Outbox<'_, S>) -> Result<Option<Seq>, PlayError> {
        match self.status {
            TrackStatus::Released => Ok(None),
            TrackStatus::Idle | TrackStatus::Loading if self.attaching.is_none() => {
                self.loading = None;
                self.status = TrackStatus::Released;
                Ok(None)
            }
            _ => out.deck(When::Next, vec![DeckPart::Detach { slot: self.slot }]),
        }
    }

    /// Sends `change` for the lane to apply on its next block; before the
    /// track has a lane, the change applies at once.
    fn configure(
        &mut self,
        change: TrackSettingsChange,
        at: When<SessionFrame>,
    ) -> Result<Option<Seq>, PlayError> {
        if matches!(at, When::At(_)) {
            return Err(PlayError::Untimed);
        }
        if self.lane.is_none() {
            self.settings.apply(change)?;
            return Ok(None);
        }
        self.exec(change, at, &mut ())
    }

    fn set_speed(
        &mut self,
        speed: SpeedCurve,
        at: When<SessionFrame>,
    ) -> Result<Option<Seq>, PlayError> {
        if let SpeedCurve::Constant(speed) = speed {
            return self.configure(TrackSettingsChange::Speed(speed), at);
        }
        if matches!(at, When::At(_)) {
            return Err(PlayError::Untimed);
        }
        let Some(lane) = &mut self.lane else {
            return Err(PlayError::NotReady);
        };
        let batch = Batch {
            basis: Vec::new(),
            commands: vec![LaneCommand::SetSpeed(speed)],
        };
        lane.send(When::Next, batch).map(Some).map_err(lane_refusal)
    }

    /// Settles the receipts the lane returned.
    fn settle_lane(&mut self) {
        let Some(lane) = &mut self.lane else {
            return;
        };
        for receipt in lane.receipts() {
            if let Some(settled) = self.settings.settle(&receipt)
                && let Outcome::Rejected(rejection) = receipt.outcome()
            {
                warn!(?rejection, change = ?settled.change, "the lane refused a track settings change");
            }
        }
    }

    /// Sends `change` to the lane for its next block.
    fn send_to_lane(&mut self, change: TrackSettingsChange) -> Result<Option<Seq>, PlayError> {
        self.settle_lane();
        let Some(lane) = &mut self.lane else {
            return Err(PlayError::NotReady);
        };
        self.settings
            .send(lane, When::Next, change, LaneCommand::from)
            .map(Some)
            .map_err(|error| match error {
                LiveError::Invalid(error) => error,
                LiveError::Send(error) => lane_refusal(error),
            })
    }
}

fn lane_refusal(error: SendError<LaneProtocol>) -> PlayError {
    match error {
        SendError::Full(_) => PlayError::Full("lane"),
        SendError::Target(_) | SendError::Closed(_) => PlayError::Closed,
    }
}

impl<S> Player<S> for PlayerImpl<S> {
    type Command = TrackCommand<S>;
    type Snapshot = TrackSnapshot;

    /// A single track enters on any frame.
    fn entry(&self, bound: Bound) -> SessionFrame {
        match bound {
            Bound::AtOrAfter(frame) | Bound::AtOrBefore(frame) => frame,
        }
    }

    fn apply(
        &mut self,
        command: TrackCommand<S>,
        out: &mut Outbox<'_, S>,
    ) -> Result<Option<Seq>, PlayError> {
        let slot = self.slot;
        match command {
            TrackCommand::Load { item, position } => self.load(item, position, out),
            TrackCommand::Play { at } => self.deck(
                at,
                vec![DeckPart::Start {
                    slot,
                    fade: Fade::Declick,
                }],
                out,
            ),
            TrackCommand::Pause { at } => self.deck(
                at,
                vec![DeckPart::Stop {
                    slot,
                    fade: Fade::Declick,
                }],
                out,
            ),
            TrackCommand::Seek { to } => {
                if let Some(loading) = &mut self.loading {
                    loading.position = to;
                    return Ok(None);
                }
                let sent = self.deck(
                    When::Next,
                    vec![DeckPart::Seek {
                        slot,
                        seconds: to.as_secs_f64(),
                        seek_epoch: self.seek_epoch + 1,
                    }],
                    out,
                )?;
                self.seek_epoch += 1;
                Ok(sent)
            }
            TrackCommand::Jump { to, at } => {
                let sent = self.deck(
                    When::At(at),
                    vec![DeckPart::Seek {
                        slot,
                        seconds: to.as_secs_f64(),
                        seek_epoch: self.seek_epoch + 1,
                    }],
                    out,
                )?;
                self.seek_epoch += 1;
                Ok(sent)
            }
            TrackCommand::Configure(change, at) => self.configure(change, at),
            TrackCommand::SetSpeed { speed, at } => self.set_speed(speed, at),
            TrackCommand::Fade { at, settings, dir } => self.fade(at, settings, dir, out),
            TrackCommand::PlayAfter { track } => self.deck(
                When::Next,
                vec![DeckPart::Chain {
                    from: track,
                    to: slot,
                }],
                out,
            ),
            TrackCommand::Release => self.release(out),
            TrackCommand::Evict { at } => {
                let Some(loading) = &mut self.loading else {
                    return Err(PlayError::NotReady);
                };
                loading.evict = Some(at);
                Ok(None)
            }
        }
    }

    fn settle(&mut self, receipt: TrackReceipt<'_, S>, out: &mut Outbox<'_, S>) -> Settled {
        if !receipt.names(self.slot) {
            return match receipt {
                TrackReceipt::Loaded(receipt) => self.opened(receipt, out),
                TrackReceipt::Deck(_) | TrackReceipt::Event(_) => Settled::Pending,
            };
        }
        match receipt {
            TrackReceipt::Deck(receipt) => self.applied(receipt),
            TrackReceipt::Event(event) => {
                self.event(event);
                Settled::Pending
            }
            TrackReceipt::Loaded(receipt) => self.opened(receipt, out),
        }
    }

    fn tick(&mut self, _now: SessionFrame, _out: &mut Outbox<'_, S>) {
        self.settle_lane();
    }

    fn snapshot(&self) -> TrackSnapshot {
        TrackSnapshot {
            item: self.item,
            slot: self.slot,
            status: self.status,
            speed: self.settings.config().speed(),
            position: self.position,
            duration: self.duration,
            abr: self.abr.clone(),
            metadata: self.metadata.clone(),
        }
    }
}

impl<S> Track<S> for PlayerImpl<S> {
    fn projected(&self) -> TrackSettings {
        self.settings.projected()
    }
}

impl<S> TrackSettingsExec<()> for PlayerImpl<S> {
    type At = When<SessionFrame>;
    type Output = Result<Option<Seq>, PlayError>;

    fn exec_live(&mut self, change: TrackSettingsChange, _at: Self::At, _cx: &mut ()) -> Self::Output {
        self.send_to_lane(change)
    }

    fn exec_speed(&mut self, speed: f32, _at: Self::At, _cx: &mut ()) -> Self::Output {
        self.send_to_lane(TrackSettingsChange::Speed(speed))
    }
}

#[cfg(test)]
mod tests;
