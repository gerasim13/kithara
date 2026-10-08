//! One loaded track: one worker lane and one slot of its deck's mixer.

use std::{collections::VecDeque, marker::PhantomData, num::NonZeroU32};

use kithara_abr::AbrHandle;
use kithara_command::{Batch, Live, Outcome, Receipt, Rejection, SendError, Sender, Seq, Target, When};
use kithara_config::{ConfigOwner, LiveConfig};
use kithara_decode::TrackMetadata;
use kithara_events::TrackId;
use kithara_platform::time::Duration;
use kithara_render::{
    CrossfadeSettings, Dispatched, DispatcherProtocol, LaneCommand, LaneFrame, LaneId,
    LaneProtocol, LoadRequest,
    bridge::{DeckEvent, DeckPart, DeckProtocol, DeckRefusal, Fade, FadeDir, Returned, Slot, SlotMark, SlotState},
};
use kithara_signal::{FrameCount, SegmentId, SessionFrame};
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
    Load { item: ResourceLoad<S>, position: Position },
    Play { at: When<SessionFrame> },
    Pause { at: When<SessionFrame> },
    Seek { to: Position },
    /// Places the new position on this session frame, after the lane's declick.
    Jump { to: Position, at: SessionFrame },
    Configure(TrackSettingsChange, When<SessionFrame>),
    SetSpeed { speed: SpeedCurve, at: When<SessionFrame> },
    /// Opens a new segment at the output route's rate.
    SetHostRate { rate: NonZeroU32 },
    Fade { at: When<SessionFrame>, settings: CrossfadeSettings, dir: FadeDir },
    PlayAfter { track: Slot },
    Release,
    Evict { at: When<SessionFrame> },
}

/// Where a track stands, as the receipts of its executors left it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TrackStatus {
    Idle,
    Loading,
    Loaded,
    Playing { since: SessionFrame },
    Paused { at: Position },
    Faded { at: SessionFrame },
    Ended { at: SessionFrame },
    Released,
}

/// The owner's settled track state and the latest mixer observation.
#[derive(Clone)]
pub struct TrackSnapshot {
    pub item: TrackId,
    pub slot: Slot,
    pub status: TrackStatus,
    pub speed: f32,
    pub position: Position,
    pub duration: Option<Duration>,
    pub abr: Option<AbrHandle>,
    pub metadata: TrackMetadata,
    pub mark: Option<SlotMark>,
    pub engine_latency: FrameCount,
    pub ring_depth: FrameCount,
    pub lane_room: usize,
    pub pending_lane: bool,
    pub attached: bool,
    pub declick: FrameCount,
}

impl std::fmt::Debug for TrackSnapshot {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("TrackSnapshot")
            .field("item", &self.item).field("slot", &self.slot)
            .field("status", &self.status).field("speed", &self.speed)
            .field("position", &self.position).field("duration", &self.duration)
            .field("abr", &self.abr.is_some()).field("metadata", &self.metadata)
            .field("mark", &self.mark).field("engine_latency", &self.engine_latency)
            .field("ring_depth", &self.ring_depth).field("lane_room", &self.lane_room)
            .field("pending_lane", &self.pending_lane).field("attached", &self.attached)
            .field("declick", &self.declick).finish()
    }
}

impl AsRef<TrackSnapshot> for TrackSnapshot {
    fn as_ref(&self) -> &TrackSnapshot { self }
}

struct Loading {
    seq: Seq,
    evict: Option<When<SessionFrame>>,
    opened: Option<OpenedTrack>,
}

struct Adoption {
    seq: Seq,
    caller: Seq,
    segment: SegmentId,
}

#[derive(Clone, Copy)]
struct Attaching {
    seq: Option<Seq>,
    caller: Seq,
    at: When<SessionFrame>,
    replacement: bool,
    play: Option<When<SessionFrame>>,
}

struct LaneOperation {
    seq: Seq,
    when: When<LaneFrame>,
    segment: SegmentId,
    session: Option<SessionFrame>,
    command: LaneCommand,
    applied: Option<bool>,
}

/// The sole segment issuer and command producer for one track.
pub struct PlayerImpl<S> {
    item: TrackId,
    slot: Slot,
    lane: Option<Sender<LaneProtocol>>,
    lane_id: Option<LaneId>,
    settings: Live<TrackSettings, LaneProtocol>,
    status: TrackStatus,
    loading: Option<Loading>,
    attaching: Option<Attaching>,
    segment: SegmentId,
    ready: Option<SegmentId>,
    adopting: Vec<Adoption>,
    adopt_retry: Option<Seq>,
    lane_commands: Vec<LaneOperation>,
    speed_answers: VecDeque<(Seq, Result<(LaneFrame, Option<SessionFrame>), Rejection<PlayError>>)>,
    segment_speed: f32,
    play: Option<When<SessionFrame>>,
    repeat: bool,
    resume: Option<SlotMark>,
    mark: Option<SlotMark>,
    attach_at: Option<SessionFrame>,
    ring_depth: FrameCount,
    engine_latency: FrameCount,
    declick: FrameCount,
    position: Position,
    duration: Option<Duration>,
    abr: Option<AbrHandle>,
    metadata: TrackMetadata,
    release: Option<Seq>,
    marker: PhantomData<fn() -> S>,
}

impl<S> PlayerImpl<S> {
    /// Builds an idle track for its assigned item and mixer slot.
    pub(crate) fn new(config: PlayerConfig) -> Result<Self, PlayError> {
        Ok(Self {
            item: config.item, slot: config.slot, lane: None, lane_id: None,
            settings: Live::new(config.settings)?, status: TrackStatus::Idle,
            loading: None, attaching: None, segment: SegmentId::FIRST, ready: None,
            adopting: Vec::new(), adopt_retry: None, lane_commands: Vec::new(),
            speed_answers: VecDeque::new(), segment_speed: config.settings.speed(), play: None, repeat: false,
            resume: None, mark: None, attach_at: None, ring_depth: FrameCount::new(0),
            engine_latency: FrameCount::new(0), declick: FrameCount::new(0),
            position: Position::ZERO, duration: None, abr: None,
            metadata: TrackMetadata::default(), release: None, marker: PhantomData,
        })
    }

    #[must_use]
    pub fn status(&self) -> TrackStatus { self.status }

    #[must_use]
    pub fn attached(&self) -> bool {
        self.attaching.is_some() || self.loading.is_none()
            && !matches!(self.status, TrackStatus::Idle | TrackStatus::Loading | TrackStatus::Released)
    }

    fn observe(&mut self, out: &Outbox<'_, S>) {
        if self.loading.is_some() { return; }
        if let Some(pass) = out.pass()
            && let Some(slot) = pass.deck.slots.get(self.slot.index())
        {
            self.mark = slot.mark;
            if slot.position.is_finite() && slot.position >= 0.0 {
                self.position = Position::from_secs_f64(slot.position);
            }
        }
    }

    fn planned_changes(&self, mark: SlotMark) -> Result<Vec<(u64, Seq, PlannedChange<'_>)>, PlayError> {
        let mut commands = Vec::new();
        for operation in &self.lane_commands {
            if operation.applied == Some(false) { continue; }
            let frame = match operation.when {
                When::At(frame) if frame.segment == mark.lane.segment => frame.frame,
                When::At(_) => continue,
                When::Next | When::Deferred if operation.segment == mark.lane.segment && matches!(operation.command, LaneCommand::SetSpeed(_) | LaneCommand::Jump { .. }) => return Err(PlayError::NotReady),
                When::Next | When::Deferred => continue,
            };
            match &operation.command {
                LaneCommand::SetSpeed(curve) => commands.push((frame, operation.seq, PlannedChange::Speed(curve))),
                LaneCommand::Jump { to } => {
                    let landing = frame.checked_add(self.declick.get() as u64).ok_or(PlayError::Untimed)?;
                    let superseded = self.lane_commands.iter().any(|next| next.seq != operation.seq && next.applied != Some(false)
                        && matches!(next.command, LaneCommand::Jump { .. })
                        && matches!(next.when, When::At(at) if at.segment == mark.lane.segment && (at.frame > frame || at.frame == frame && next.seq > operation.seq) && at.frame < landing));
                    if !superseded { commands.push((landing, operation.seq, PlannedChange::Jump(*to))); }
                }
                _ => {}
            }
        }
        commands.sort_by_key(|&(frame, seq, _)| (frame, seq));
        Ok(commands)
    }

    fn check_when(&self, at: When<SessionFrame>, out: &Outbox<'_, S>) -> Result<(), PlayError> {
        if let When::At(frame) = at {
            let pass = out.pass().ok_or(PlayError::Untimed)?;
            let origin = self.attach_at.map_or(pass.now, |attach| attach.max(pass.now));
            if frame < origin + pass.delivery { return Err(PlayError::Late); }
            if self.attach_at.is_none() { return Err(PlayError::Untimed); }
        }
        Ok(())
    }

    fn lane_when(&self, at: When<SessionFrame>, out: &Outbox<'_, S>) -> Result<When<LaneFrame>, PlayError> {
        match at {
            When::Next => Ok(When::Next),
            When::Deferred => Err(PlayError::Internal("the lane has no end-marker clock".into())),
            When::At(frame) => {
                self.check_when(at, out)?;
                let pass = out.pass().ok_or(PlayError::Untimed)?;
                if frame < pass.earliest() + self.ring_depth + self.engine_latency { return Err(PlayError::Late); }
                let mark = self.mark.ok_or(PlayError::Untimed)?;
                if mark.lane.segment != self.segment || !matches!(self.status, TrackStatus::Playing { .. }) {
                    return Err(PlayError::Untimed);
                }
                mark.lane_at(frame).map(When::At).ok_or(PlayError::Untimed)
            }
        }
    }

    fn send_lane(&mut self, command: LaneCommand, when: When<LaneFrame>) -> Result<Seq, PlayError> {
        let session = match when {
            When::At(frame) => self.mark.and_then(|mark| session_at(mark, frame)),
            When::Next | When::Deferred => None,
        };
        let lane = self.lane.as_mut().ok_or(PlayError::NotReady)?;
        let seq = lane.send(when, Batch { basis: Vec::new(), commands: vec![command.clone()] }).map_err(lane_refusal)?;
        self.lane_commands.push(LaneOperation { seq, when, segment: self.segment, session, command, applied: None });
        Ok(seq)
    }

    fn load(&mut self, item: ResourceLoad<S>, position: Position, out: &mut Outbox<'_, S>) -> Result<Option<Seq>, PlayError> {
        if self.status != TrackStatus::Idle { return Err(PlayError::Internal("a track loads once".into())); }
        if out.dispatcher_available() == 0 { return Err(PlayError::Full("dispatcher")); }
        let (lane, inbox) = item.lane_channel()?;
        let (ring_depth, declick) = item.lane_geometry()?;
        let seq = out.load(LoadRequest { item, position, start: self.settings.config().lane_start(), inbox })?;
        self.lane = Some(lane);
        self.ring_depth = ring_depth;
        self.declick = declick;
        self.position = position;
        self.loading = Some(Loading { seq, evict: None, opened: None });
        self.status = TrackStatus::Loading;
        Ok(Some(seq))
    }

    fn opened(&mut self, receipt: Receipt<DispatcherProtocol<ResourceLoad<S>>>, out: &mut Outbox<'_, S>) -> Settled {
        let seq = receipt.seq();
        if self.release == Some(seq) {
            self.release = None;
            match receipt.outcome() {
                Outcome::Applied { data: Dispatched::Released, .. } => {
                    self.lane = None;
                    self.lane_id = None;
                }
                Outcome::Rejected(reason) => warn!(?reason, "dispatcher refused lane release"),
                _ => warn!(?seq, "lane release received a non-release answer"),
            }
            return Settled::Pending;
        }
        if !self.loading.as_ref().is_some_and(|loading| loading.seq == seq) { return Settled::Pending; }
        let (outcome, _) = receipt.into();
        match outcome {
            Outcome::Applied { data: Dispatched::Loaded(loaded), .. } => {
                self.lane_id = Some(loaded.lane);
                self.engine_latency = loaded.engine_latency;
                if self.segment == SegmentId::FIRST {
                    self.ready = Some(SegmentId::FIRST);
                }
                self.duration = loaded.opened.duration;
                self.abr = loaded.opened.abr.clone();
                self.metadata = loaded.opened.metadata.clone();
                if let Some(loading) = self.loading.as_mut() { loading.opened = Some(loaded.opened); }
                if self.status == TrackStatus::Released {
                    self.loading = None;
                    if let Err(error) = self.release_lane(out) { warn!(%error, "released open waits for dispatcher room"); }
                    return Settled::Pending;
                }
                self.reserved_ready();
                self.attach(out);
                Settled::Pending
            }
            Outcome::Rejected(refused) => {
                self.loading = None;
                self.lane = None;
                self.status = TrackStatus::Idle;
                Settled::Rejected { seq, reason: rejection(&refused, |refusal| PlayError::ItemFailed { reason: refusal.to_string() }) }
            }
            Outcome::Applied { .. } => Settled::Rejected { seq, reason: Rejection::Refused(PlayError::Internal("a load received a non-load answer".into())) },
        }
    }

    fn attach(&mut self, out: &mut Outbox<'_, S>) {
        if out.is_grouped() || out.deck_available() == 0 || self.attaching.is_some() { return; }
        let Some(loading) = self.loading.as_mut() else { return; };
        if loading.evict.is_some() { return; }
        let Some(opened) = loading.opened.take() else { return; };
        let OpenedTrack { pcm, duration, abr, metadata } = opened;
        let seq = loading.seq;
        let at = When::Next;
        let slot = self.slot;
        let mut parts = vec![DeckPart::Attach { slot, pcm, segment: self.segment }];
        let start = self.play.is_some() && self.ready == Some(self.segment);
        if start {
            parts.push(DeckPart::Start { slot, fade: Fade::Declick });
        }
        match out.deck_owned(at, parts) {
            Ok(attach) => {
                self.attaching = Some(Attaching { seq: attach, caller: seq, at, replacement: false, play: if start { self.play.take() } else { None } });
            }
            Err((error, parts)) => {
                let pcm = parts.into_iter().find_map(|part| match part {
                    DeckPart::Attach { pcm, .. } | DeckPart::Replace { pcm, .. } => Some(pcm),
                    _ => None,
                });
                if let Some(pcm) = pcm {
                    loading.opened = Some(OpenedTrack { pcm, duration, abr, metadata });
                } else {
                    warn!(%error, "attachment refusal lost its original PCM-bearing part");
                    self.status = TrackStatus::Idle;
                    self.loading = None;
                    return;
                }
                warn!(%error, "the deck refused attachment");
            }
        }
    }

    fn restore_attachment(&mut self, parts: &mut Vec<DeckPart>) -> bool {
        let Some(loading) = self.loading.as_mut() else { return false; };
        let Some(index) = parts.iter().position(|part| matches!(part,
            DeckPart::Attach { slot, .. } | DeckPart::Replace { slot, .. } if *slot == self.slot
        )) else { return false; };
        let pcm = match parts.remove(index) {
            DeckPart::Attach { pcm, .. } | DeckPart::Replace { pcm, .. } => pcm,
            _ => unreachable!("the selected attachment carries PCM"),
        };
        loading.opened = Some(OpenedTrack {
            pcm, duration: self.duration, abr: self.abr.clone(), metadata: self.metadata.clone(),
        });
        true
    }

    fn reserved_ready(&mut self) {
        if self.ready == Some(self.segment)
            && self.loading.as_ref().is_some_and(|loading|
                loading.evict == Some(When::Next) && loading.opened.is_some()
            )
        {
            self.status = TrackStatus::Loaded;
        }
    }

    fn evict(&mut self, at: When<SessionFrame>, out: &mut Outbox<'_, S>) -> Result<Option<Seq>, PlayError> {
        if matches!(at, When::Deferred) {
            return Err(PlayError::Internal("a replacement needs a timed deck batch".into()));
        }
        if at == When::Next {
            let loading = self.loading.as_mut().ok_or(PlayError::NotReady)?;
            loading.evict = Some(When::Next);
            self.reserved_ready();
            return Ok(None);
        }
        if self.attaching.is_some() || self.ready != Some(self.segment) {
            return Err(PlayError::NotReady);
        }
        let pass = out.pass().ok_or(PlayError::Untimed)?;
        if !pass.deck.slots.get(self.slot.index()).is_some_and(|slot| slot.state != SlotState::Empty) {
            return Err(PlayError::NotReady);
        }
        let loading = self.loading.as_mut().ok_or(PlayError::NotReady)?;
        let opened = loading.opened.take().ok_or(PlayError::NotReady)?;
        let OpenedTrack { pcm, duration, abr, metadata } = opened;
        match out.deck_owned(at, vec![DeckPart::Replace { slot: self.slot, pcm, segment: self.segment }]) {
            Ok(seq) => {
                loading.evict = Some(at);
                self.attaching = Some(Attaching {
                    seq, caller: seq.map_or(loading.seq, |seq| seq), at, replacement: true,
                    play: self.play.take(),
                });
                Ok(seq)
            }
            Err((error, parts)) => {
                if let Some(pcm) = parts.into_iter().find_map(|part| match part {
                    DeckPart::Replace { slot, pcm, .. } if slot == self.slot => Some(pcm),
                    _ => None,
                }) {
                    loading.opened = Some(OpenedTrack { pcm, duration, abr, metadata });
                    loading.evict = Some(When::Next);
                } else {
                    return Err(PlayError::Internal("replacement refusal did not return its original PCM".into()));
                }
                self.reserved_ready();
                Err(error)
            }
        }
    }

    fn segment(&mut self, from: Position, rate: Option<NonZeroU32>, out: &mut Outbox<'_, S>) -> Result<Option<Seq>, PlayError> {
        self.segment_with(from, rate, self.settings.projected().speed(), out)
    }

    fn segment_with(&mut self, from: Position, rate: Option<NonZeroU32>, speed: f32, out: &mut Outbox<'_, S>) -> Result<Option<Seq>, PlayError> {
        let change = TrackSettings::check(TrackSettingsChange::Speed(speed))?;
        let attached = self.attached();
        if attached && out.is_grouped() { return Err(PlayError::Internal("a segment adoption cannot join a timed group".into())); }
        let lane = self.lane.as_ref().ok_or(PlayError::NotReady)?;
        if lane.available() == 0 { return Err(PlayError::Full("lane")); }
        if attached && out.deck_available() == 0 { return Err(PlayError::Full("deck")); }
        let segment = self.segment.next();
        let command = match rate {
            Some(rate) => LaneCommand::SetHostRate { id: segment, rate },
            None => LaneCommand::Segment { id: segment, from, speed: SpeedCurve::Constant(speed) },
        };
        let seq = self.send_lane(command, When::Next)?;
        if rate.is_none() { self.settings.track(seq, When::Next, change); }
        self.segment = segment;
        self.segment_speed = speed;
        self.ready = None;
        self.mark = None;
        self.position = from;
        if self.loading.as_ref().is_some_and(|loading| loading.evict == Some(When::Next)) {
            self.status = TrackStatus::Loading;
        }
        if attached {
            let adopt = out.deck_owned(When::Next, vec![DeckPart::Adopt { slot: self.slot, segment }]).map_err(|(error, _parts)| error)?;
            if let Some(adopt) = adopt {
                self.adopting.push(Adoption { seq: adopt, caller: adopt, segment });
            }
            return Ok(adopt);
        }
        Ok(Some(seq))
    }

    fn applied(&mut self, seq: Seq, outcome: &Outcome<DeckProtocol>, batch: &mut Batch<DeckProtocol>, out: &mut Outbox<'_, S>) -> Settled {
        let attaching = self.attaching.filter(|attach| attach.seq == Some(seq));
        let mut answered = attaching.map_or(seq, |attach| attach.caller);
        if let Some(index) = self.adopting.iter().position(|adopt| adopt.seq == seq) {
            let adoption = self.adopting.remove(index);
            answered = adoption.caller;
            if matches!(outcome, Outcome::Rejected(Rejection::Stale | Rejection::Refused(DeckRefusal::Outdated { .. }))) {
                self.adopt_retry = Some(adoption.caller);
                self.retry_adopt(out);
                return Settled::Pending;
            }
            if matches!(outcome, Outcome::Applied { .. }) && adoption.segment == self.segment {
                self.repeat = false;
            }
        }
        let at = match outcome {
            Outcome::Applied { at, .. } => *at,
            Outcome::Rejected(reason) => {
                if attaching.is_some() {
                    self.attaching = None;
                    if self.restore_attachment(&mut batch.commands) {
                        if self.play.is_none() { self.play = attaching.and_then(|attach| attach.play); }
                        if attaching.is_some_and(|attach| attach.replacement) {
                            if let Some(loading) = self.loading.as_mut() { loading.evict = Some(When::Next); }
                            self.reserved_ready();
                        } else if !matches!(reason, Rejection::Unanswered) {
                            return Settled::Pending;
                        }
                    } else {
                        self.loading = None;
                        self.status = TrackStatus::Idle;
                        return Settled::Rejected { seq: answered, reason: Rejection::Refused(PlayError::Internal("rejected attachment did not return its original PCM".into())) };
                    }
                }
                return Settled::Rejected { seq: answered, reason: rejection(reason, |refusal| PlayError::Deck(*refusal)) };
            }
        };
        if attaching.is_some() {
            self.attaching = None;
            self.loading = None;
            self.attach_at = Some(at);
            self.status = TrackStatus::Loaded;
        }
        for part in &batch.commands {
            match part {
                DeckPart::Start { slot, .. } | DeckPart::Chain { to: slot, .. } if *slot == self.slot => {
                    self.status = TrackStatus::Playing { since: at };
                    self.resume = None;
                }
                DeckPart::Returned(Returned::Stopped { slot, resume }) if *slot == self.slot => {
                    self.position = resume.position;
                    self.resume = Some(*resume);
                    self.status = TrackStatus::Paused { at: self.position };
                }
                DeckPart::Returned(Returned::Pcm { slot, .. }) if *slot == self.slot && attaching.is_none() => {
                    self.status = TrackStatus::Released;
                    self.mark = None;
                    if let Err(error) = self.release_lane(out) { warn!(%error, "lane release waits for dispatcher room"); }
                }
                _ => {}
            }
        }
        Settled::Applied { seq: answered, at }
    }

    fn retry_adopt(&mut self, out: &mut Outbox<'_, S>) {
        let Some(caller) = self.adopt_retry else { return; };
        if out.deck_available() == 0 { return; }
        match out.deck_owned(When::Next, vec![DeckPart::Adopt { slot: self.slot, segment: self.segment }]) {
            Ok(Some(seq)) => {
                self.adopting.push(Adoption { seq, caller, segment: self.segment });
                self.adopt_retry = None;
            }
            Ok(None) => {}
            Err((error, _parts)) => warn!(%error, "the latest segment's adoption waits for room"),
        }
    }

    fn release_lane(&mut self, out: &mut Outbox<'_, S>) -> Result<(), PlayError> {
        if self.release.is_none() && let Some(lane) = self.lane_id {
            self.release = Some(out.release(lane)?);
        }
        Ok(())
    }

    fn play(&mut self, at: When<SessionFrame>, out: &mut Outbox<'_, S>) -> Result<Option<Seq>, PlayError> {
        self.check_when(at, out)?;
        if let Some(resume) = self.resume {
            let pending = self.lane_commands.iter().filter(|operation| operation.applied != Some(false)).any(|operation| match operation.when {
                When::At(frame) => frame.segment == resume.lane.segment && frame.frame > resume.lane.frame,
                When::Next | When::Deferred => operation.segment == resume.lane.segment,
            });
            if pending {
                self.segment(resume.position, None, out)?;
                self.resume = None;
                self.play = Some(at);
                return Ok(None);
            }
        }
        if !self.attached() || self.ready != Some(self.segment) {
            self.play = Some(at);
            return Ok(None);
        }
        out.deck(at, vec![DeckPart::Start { slot: self.slot, fade: Fade::Declick }])
    }

    fn configure(&mut self, change: TrackSettingsChange, at: When<SessionFrame>, out: &mut Outbox<'_, S>) -> Result<Option<Seq>, PlayError> {
        if self.lane.is_none() {
            if !matches!(at, When::Next) { return Err(PlayError::Untimed); }
            self.settings.apply(change)?;
            return Ok(None);
        }
        let when = self.lane_when(at, out)?;
        let change = TrackSettings::check(change)?;
        let seq = self.send_lane(LaneCommand::from(change), when)?;
        self.settings.track(seq, when, change);
        Ok(Some(seq))
    }

    fn settle_lane(&mut self) {
        let Some(lane) = self.lane.as_mut() else { return; };
        for receipt in lane.receipts() {
            self.settings.settle(&receipt);
            let operation = self.lane_commands.iter().position(|operation| operation.seq == receipt.seq());
            if let Outcome::Applied { at, data } = receipt.outcome() {
                if let Some(index) = operation {
                    let operation = &mut self.lane_commands[index];
                    let session = match operation.when {
                        When::At(requested) if requested == *at => operation.session,
                        _ => self.mark.and_then(|mark| session_at(mark, *at)),
                    };
                    operation.when = When::At(*at);
                    operation.applied = Some(true);
                    if matches!(&operation.command, LaneCommand::SetSpeed(_)) {
                        self.speed_answers.push_back((receipt.seq(), Ok((*at, session))));
                    }
                }
                self.engine_latency = data.engine_latency;
                if let Some(ready) = data.ready { self.ready = Some(ready); }
            } else if let Outcome::Rejected(reason) = receipt.outcome() {
                if let Some(index) = operation {
                    let operation = &mut self.lane_commands[index];
                    operation.applied = Some(false);
                    if matches!(operation.command, LaneCommand::SetSpeed(_)) {
                        self.speed_answers.push_back((receipt.seq(), Err(rejection(reason, |never| match *never {}))));
                    }
                }
                warn!(?reason, "lane command was rejected");
            }
        }
        self.reserved_ready();
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

    fn entry(&self, bound: Bound) -> Option<SessionFrame> {
        match bound { Bound::AtOrAfter(frame) | Bound::AtOrBefore(frame) => Some(frame) }
    }

    fn apply(&mut self, command: TrackCommand<S>, out: &mut Outbox<'_, S>) -> Result<Option<Seq>, PlayError> {
        self.observe(out);
        self.settle_lane();
        let slot = self.slot;
        match command {
            TrackCommand::Load { item, position } => self.load(item, position, out),
            TrackCommand::Play { at } => self.play(at, out),
            TrackCommand::Pause { at } => {
                self.check_when(at, out)?;
                self.play = None;
                if let Some(attaching) = self.attaching.as_mut() { attaching.play = None; }
                if !self.attached() { return Ok(None); }
                out.deck(at, vec![DeckPart::Stop { slot, fade: Fade::Declick }])
            }
            TrackCommand::Seek { to } => self.segment(to, None, out),
            TrackCommand::SetHostRate { rate } => self.segment(self.position, Some(rate), out),
            TrackCommand::Jump { to, at } => {
                let start = i64::from(at).checked_sub(i64::try_from(self.declick.get()).map_err(|error| PlayError::Internal(error.to_string()))?).map(SessionFrame::new).ok_or(PlayError::Late)?;
                let When::At(frame) = self.lane_when(When::At(start), out)? else { return Err(PlayError::Untimed); };
                self.send_lane(LaneCommand::Jump { to }, When::At(frame)).map(Some)
            }
            TrackCommand::Configure(change, at) => self.configure(change, at, out),
            TrackCommand::SetSpeed { speed, at } => {
                if let SpeedCurve::Constant(speed) = speed { return self.configure(TrackSettingsChange::Speed(speed), at, out); }
                let final_speed = match &speed {
                    SpeedCurve::Ramp { to, .. } => *to,
                    SpeedCurve::Steps(steps) => {
                        let Some(&(_, last)) = steps.last() else { return Err(PlayError::Internal("an empty speed curve has no target".into())); };
                        let mut previous = None;
                        for &(frame, value) in steps.iter() {
                            TrackSettings::check(TrackSettingsChange::Speed(value))?;
                            if previous.is_some_and(|prior| frame <= prior) { return Err(PlayError::Internal("speed steps must increase on the output axis".into())); }
                            previous = Some(frame);
                        }
                        last
                    }
                    _ => return Err(PlayError::Internal("unsupported speed curve".into())),
                };
                let change = TrackSettings::check(TrackSettingsChange::Speed(final_speed))?;
                let when = self.lane_when(at, out)?;
                let seq = self.send_lane(LaneCommand::SetSpeed(speed), when)?;
                self.settings.track(seq, when, change);
                Ok(Some(seq))
            }
            TrackCommand::Fade { at, settings, dir } => {
                let staged = dir == FadeDir::In && out.is_grouped()
                    && self.attaching.is_some_and(|attach|
                        attach.seq.is_none() && attach.replacement && attach.at == at
                    );
                if !staged { self.check_when(at, out)?; }
                let part = match (self.status, dir) {
                    (TrackStatus::Playing { .. }, _) => DeckPart::Fade { slot, settings, dir },
                    (_, FadeDir::In) => DeckPart::Start { slot, fade: Fade::Crossfade(settings) },
                    (_, FadeDir::Out) => return Ok(None),
                };
                out.deck(at, vec![part])
            }
            TrackCommand::PlayAfter { track } if track == slot => {
                if out.is_grouped() { return Err(PlayError::Internal("a repeat adoption cannot join a timed group".into())); }
                if self.lane.as_ref().is_none_or(|lane| lane.available() == 0) { return Err(PlayError::Full("lane")); }
                if out.deck_available() == 0 { return Err(PlayError::Full("deck")); }
                let segment = self.segment.next();
                self.send_lane(LaneCommand::Segment { id: segment, from: Position::ZERO, speed: SpeedCurve::Constant(self.settings.projected().speed()) }, When::Next)?;
                let seq = out.deferred(vec![DeckPart::Adopt { slot, segment }])?;
                self.segment = segment;
                self.segment_speed = self.settings.projected().speed();
                self.ready = None;
                self.repeat = true;
                self.adopting.push(Adoption { seq, caller: seq, segment });
                Ok(Some(seq))
            }
            TrackCommand::PlayAfter { track } => out.chain(track, slot).map(Some),
            TrackCommand::Release => {
                self.play = None;
                if let Some(attaching) = self.attaching.as_mut() { attaching.play = None; }
                if self.status == TrackStatus::Released { self.release_lane(out)?; return Ok(None); }
                if self.attached() { return out.deck(When::Next, vec![DeckPart::Detach { slot }]); }
                self.status = TrackStatus::Released;
                self.release_lane(out)?;
                Ok(None)
            }
            TrackCommand::Evict { at } => self.evict(at, out),
        }
    }

    fn settle(&mut self, receipt: TrackReceipt<'_, S>, out: &mut Outbox<'_, S>) -> Settled {
        self.observe(out);
        match receipt {
            TrackReceipt::Loaded(receipt) => self.opened(receipt, out),
            TrackReceipt::Deck { seq, outcome, batch } if batch.basis.iter().any(|&(slot, _)| slot == self.slot) => self.applied(seq, outcome, batch, out),
            TrackReceipt::Deck { .. } => Settled::Pending,
            TrackReceipt::Event(event) => {
                match event {
                    DeckEvent::Ended { slot, at } if slot == self.slot && !self.repeat => self.status = TrackStatus::Ended { at },
                    DeckEvent::Faded { slot, at } if slot == self.slot => self.status = TrackStatus::Faded { at },
                    DeckEvent::Underrun { slot, .. } if slot == self.slot && self.loading.is_none() => {
                        self.mark = out.pass().and_then(|pass| pass.deck.slots.get(slot.index())).and_then(|slot| slot.mark);
                    }
                    _ => {}
                }
                Settled::Pending
            }
        }
    }

    fn tick(&mut self, _now: SessionFrame, out: &mut Outbox<'_, S>) {
        self.observe(out);
        self.settle_lane();
        self.retry_adopt(out);
        self.attach(out);
        if self.status == TrackStatus::Released {
            if let Err(error) = self.release_lane(out) { warn!(%error, "lane release waits for room"); }
        } else if self.attached() && self.ready == Some(self.segment) && let Some(at) = self.play {
            match self.play(at, out) {
                Ok(Some(_)) => self.play = None,
                Ok(None) => {}
                Err(error) => warn!(%error, "ready start was rejected"),
            }
        }
    }

    fn snapshot(&self) -> TrackSnapshot {
        TrackSnapshot {
            item: self.item, slot: self.slot, status: self.status,
            speed: self.settings.config().speed(), position: self.position,
            duration: self.duration, abr: self.abr.clone(), metadata: self.metadata.clone(),
            mark: self.mark, engine_latency: self.engine_latency, ring_depth: self.ring_depth,
            lane_room: self.lane.as_ref().map_or(0, Sender::available), pending_lane: self.lane_commands.iter().any(|operation| operation.applied.is_none()),
            attached: self.attached(), declick: self.declick,
        }
    }
}

impl<S> Track<S> for PlayerImpl<S> {
    fn projected(&self) -> TrackSettings { self.settings.projected() }

    fn finish_group(&mut self, result: Result<Seq, &mut Vec<DeckPart>>) {
        let Some(attaching) = self.attaching.filter(|attach| attach.seq.is_none()) else { return; };
        match result {
            Ok(seq) => {
                if let Some(attach) = self.attaching.as_mut() {
                    attach.seq = Some(seq);
                    if attach.replacement { attach.caller = seq; }
                }
            }
            Err(parts) => {
                self.attaching = None;
                if self.restore_attachment(parts) {
                    if self.play.is_none() { self.play = attaching.play; }
                    if attaching.replacement {
                        if let Some(loading) = self.loading.as_mut() { loading.evict = Some(When::Next); }
                        self.reserved_ready();
                    }
                } else {
                    warn!("an aborted group did not return its original attachment PCM");
                }
            }
        }
    }

    fn planned(&self, at: SessionFrame, sample_rate: NonZeroU32) -> Result<(Position, f32), PlayError> {
        let mark = self.mark.ok_or(PlayError::Untimed)?;
        if mark.lane.segment != self.segment || !matches!(self.status, TrackStatus::Playing { .. }) { return Err(PlayError::Untimed); }
        let elapsed = at.frames_since(mark.session).ok_or(PlayError::Untimed)?;
        let target = mark.lane.frame.checked_add(elapsed).ok_or(PlayError::Untimed)?;
        let commands = self.planned_changes(mark)?;
        let base = SpeedCurve::Constant(self.segment_speed);
        let mut curve = &base;
        let mut origin = self.segment_speed;
        let mut start = 0;
        let mut cursor = mark.lane.frame;
        let mut position = mark.position.as_secs_f64();
        for (frame, _, change) in commands.into_iter().filter(|(frame, _, _)| *frame <= target) {
            if frame > cursor {
                position += curve_area(curve, origin, cursor.saturating_sub(start), frame.saturating_sub(start))? / f64::from(sample_rate.get());
                cursor = frame;
            }
            match change {
                PlannedChange::Speed(next) => {
                    origin = curve_speed(curve, origin, frame.saturating_sub(start))?;
                    start = frame;
                    curve = next;
                }
                PlannedChange::Jump(to) if frame > mark.lane.frame => position = to.as_secs_f64(),
                PlannedChange::Jump(_) => {}
            }
        }
        position += curve_area(curve, origin, cursor.saturating_sub(start), target.saturating_sub(start))? / f64::from(sample_rate.get());
        let position = Position::try_from_secs_f64(position).map_err(|error| PlayError::Internal(error.to_string()))?;
        Ok((position, curve_speed(curve, origin, target.saturating_sub(start))?))
    }

    fn planned_end(&self, sample_rate: NonZeroU32) -> Result<Option<SessionFrame>, PlayError> {
        if let TrackStatus::Ended { at } = self.status { return Ok(Some(at)); }
        let Some(duration) = self.duration else { return Ok(None); };
        if !matches!(self.status, TrackStatus::Playing { .. }) { return Ok(None); }
        let mark = self.mark.ok_or(PlayError::Untimed)?;
        if mark.lane.segment != self.segment { return Err(PlayError::Untimed); }
        let commands = self.planned_changes(mark)?;
        let base = SpeedCurve::Constant(self.segment_speed);
        let mut curve = &base;
        let mut origin = self.segment_speed;
        let mut start = 0;
        let mut cursor = mark.lane.frame;
        let rate = f64::from(sample_rate.get());
        let mut position = mark.position.as_secs_f64();
        for (frame, _, change) in commands {
            if frame > cursor {
                let remaining = (duration.as_secs_f64() - position).max(0.0) * rate;
                if let Some(offset) = curve_end(curve, origin, cursor.saturating_sub(start), remaining, Some(frame.saturating_sub(start)))? {
                    let end = start.checked_add(offset).ok_or(PlayError::Untimed)?;
                    return session_at(mark, LaneFrame { segment: self.segment, frame: end }).map(Some).ok_or(PlayError::Untimed);
                }
                position += curve_area(curve, origin, cursor.saturating_sub(start), frame.saturating_sub(start))? / rate;
                cursor = frame;
            }
            match change {
                PlannedChange::Speed(next) => {
                    origin = curve_speed(curve, origin, frame.saturating_sub(start))?;
                    start = frame;
                    curve = next;
                }
                PlannedChange::Jump(to) if frame > mark.lane.frame => position = to.as_secs_f64(),
                PlannedChange::Jump(_) => {}
            }
        }
        let remaining = (duration.as_secs_f64() - position).max(0.0) * rate;
        let offset = curve_end(curve, origin, cursor.saturating_sub(start), remaining, None)?.ok_or(PlayError::Untimed)?;
        let end = start.checked_add(offset).ok_or(PlayError::Untimed)?;
        session_at(mark, LaneFrame { segment: self.segment, frame: end }).map(Some).ok_or(PlayError::Untimed)
    }

    fn speed_receipt(&mut self) -> Option<Settled> {
        self.settle_lane();
        let (index, settled) = self.speed_answers.iter().enumerate().find_map(|(index, (seq, outcome))| {
            let settled = match outcome {
                Ok((lane, session)) => {
                    let at = (*session).or_else(|| self.mark.and_then(|mark| session_at(mark, *lane)))?;
                    Settled::Applied { seq: *seq, at }
                }
                Err(reason) => Settled::Rejected { seq: *seq, reason: reason.clone() },
            };
            Some((index, settled))
        })?;
        self.speed_answers.remove(index);
        Some(settled)
    }

    fn speed_applied(&mut self, seq: Seq) -> Option<bool> {
        self.settle_lane();
        self.lane_commands.iter().find(|operation| operation.seq == seq).and_then(|operation| operation.applied)
    }

    fn cue(&mut self, position: Position, speed: f32, out: &mut Outbox<'_, S>) -> Result<Option<Seq>, PlayError> {
        self.observe(out);
        self.settle_lane();
        self.segment_with(position, None, speed, out)?;
        Ok(self.lane_commands.last().map(|operation| operation.seq))
    }
}

enum PlannedChange<'a> {
    Speed(&'a SpeedCurve),
    Jump(Position),
}

fn session_at(mark: SlotMark, lane: LaneFrame) -> Option<SessionFrame> {
    if lane.segment != mark.lane.segment { return None; }
    let distance = i64::try_from(lane.frame.abs_diff(mark.lane.frame)).ok()?;
    let frame = if lane.frame >= mark.lane.frame {
        i64::from(mark.session).checked_add(distance)
    } else {
        i64::from(mark.session).checked_sub(distance)
    }?;
    Some(SessionFrame::new(frame))
}

fn curve_speed(curve: &SpeedCurve, origin: f32, frame: u64) -> Result<f32, PlayError> {
    match curve {
        SpeedCurve::Constant(speed) => Ok(*speed),
        SpeedCurve::Ramp { to, frames } => Ok((f64::from(origin) + (f64::from(*to) - f64::from(origin)) * (frame.min(frames.get()) as f64 / frames.get() as f64)) as f32),
        SpeedCurve::Steps(steps) => Ok(steps.iter().rev().find(|(at, _)| *at <= frame).map_or(origin, |(_, speed)| *speed)),
        _ => Err(PlayError::Internal("unsupported planned speed curve".into())),
    }
}

fn curve_area(curve: &SpeedCurve, origin: f32, start: u64, end: u64) -> Result<f64, PlayError> {
    match curve {
        SpeedCurve::Constant(speed) => Ok((end - start) as f64 * f64::from(*speed)),
        SpeedCurve::Ramp { to, frames } => {
            let duration = frames.get() as f64;
            let area = |frame: u64| {
                let ramp = frame.min(frames.get()) as f64;
                f64::from(origin) * ramp + (f64::from(*to) - f64::from(origin)) * ramp * ramp / (2.0 * duration)
                    + frame.saturating_sub(frames.get()) as f64 * f64::from(*to)
            };
            Ok(area(end) - area(start))
        }
        SpeedCurve::Steps(steps) => {
            let mut area = 0.0;
            let mut cursor = start;
            let mut speed = f64::from(origin);
            for &(frame, next) in steps.iter() {
                if frame > end { break; }
                if frame > cursor { area += (frame - cursor) as f64 * speed; cursor = frame; }
                speed = f64::from(next);
            }
            Ok(area + (end - cursor) as f64 * speed)
        }
        _ => Err(PlayError::Internal("unsupported planned speed curve".into())),
    }
}

fn curve_end(curve: &SpeedCurve, origin: f32, start: u64, mut remaining: f64, limit: Option<u64>) -> Result<Option<u64>, PlayError> {
    if remaining <= 0.0 { return Ok(Some(start)); }
    let mut cursor = start;
    let end = limit.unwrap_or(u64::MAX);
    while cursor < end {
        let speed = f64::from(curve_speed(curve, origin, cursor)?);
        if !speed.is_finite() || speed <= 0.0 { return Err(PlayError::Internal("planned speed must be finite and positive".into())); }
        let (boundary, slope) = match curve {
            SpeedCurve::Constant(_) => (end, 0.0),
            SpeedCurve::Ramp { to, frames } if cursor < frames.get() => (end.min(frames.get()), (f64::from(*to) - f64::from(origin)) / frames.get() as f64),
            SpeedCurve::Ramp { .. } => (end, 0.0),
            SpeedCurve::Steps(steps) => (steps.iter().find(|(frame, _)| *frame > cursor).map_or(end, |(frame, _)| end.min(*frame)), 0.0),
            _ => return Err(PlayError::Internal("unsupported planned speed curve".into())),
        };
        let area = curve_area(curve, origin, cursor, boundary)?;
        if remaining <= area {
            let distance = if slope == 0.0 { remaining / speed } else {
                2.0 * remaining / (speed + (speed * speed + 2.0 * slope * remaining).max(0.0).sqrt())
            };
            let rounded = distance.ceil();
            if !rounded.is_finite() || rounded < 0.0 || rounded >= u64::MAX as f64 { return Err(PlayError::Untimed); }
            return cursor.checked_add(rounded as u64).map(|frame| Some(frame.min(boundary))).ok_or(PlayError::Untimed);
        }
        remaining -= area;
        cursor = boundary;
    }
    Ok(None)
}

impl<S> TrackSettingsExec<()> for PlayerImpl<S> {
    type At = When<SessionFrame>;
    type Output = Result<Option<Seq>, PlayError>;

    fn exec_live(&mut self, change: TrackSettingsChange, at: Self::At, _cx: &mut ()) -> Self::Output {
        if !matches!(at, When::Next) { return Err(PlayError::Untimed); }
        if self.lane.is_some() {
            let change = TrackSettings::check(change)?;
            let seq = self.send_lane(LaneCommand::from(change), When::Next)?;
            self.settings.track(seq, When::Next, change);
            Ok(Some(seq))
        } else { self.settings.apply(change)?; Ok(None) }
    }

    fn exec_speed(&mut self, speed: f32, at: Self::At, cx: &mut ()) -> Self::Output {
        self.exec_live(TrackSettingsChange::Speed(speed), at, cx)
    }
}

#[cfg(test)]
mod tests;
