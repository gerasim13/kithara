use std::task::Waker;

use kithara_beat::BeatGridModel;
use kithara_command::{Rejection, Seq, When};
use kithara_play::{
    Bound, DeckControl, DeckPass, HostedDeck, Outbox, PlayError, PlayWorker, Player, Position,
    Settled, Track, TrackCommand, TrackReceipt, TrackSettings, TrackSettingsChange, TrackStatus,
};
use kithara_render::{
    bridge::{DeckEvent, DeckPart},
    rt::DeckMixerConfig,
};
use kithara_signal::{FrameCount, SessionFrame};
use kithara_sync::checked_correction;
use tracing::warn;

use super::{LinkConfig, LinkedPlayer, LinkedSnapshot, SyncMode, SyncStatus, Waiting};
use crate::{
    GridAnswer, LinkedConfig, TempoTrajectory, covers, entry, jump_target, phase_error, speed,
};

/// One track decorated with analysis, Host tempo and owner-thread synchronization.
pub struct Linked<P> {
    inner: P,
    config: LinkConfig,
    grid: Option<BeatGridModel>,
    host: TempoTrajectory,
    pub(crate) mode: SyncMode,
    load: Option<Seq>,
    waiting: Option<Waiting>,
    correction: Option<RunningCorrection>,
    alignment: Option<bool>,
    speeds: Vec<(Seq, SpeedOperation)>,
    start: Option<Seq>,
    start_caller: Option<Seq>,
    withdrawn: Vec<Seq>,
    retimes: Vec<(SessionFrame, Seq)>,
    cue: Option<Position>,
}

struct RunningCorrection {
    at: SessionFrame,
    frames: FrameCount,
}

enum SpeedOperation {
    Correction { commanded: bool },
    Alignment,
    Relocation { to: Position },
}

impl<P> Linked<P> {
    fn synced_speed(&self, change: TrackSettingsChange) -> Result<(), PlayError> {
        if self.mode == SyncMode::On
            && let TrackSettingsChange::Speed(speed) = change
        {
            return Err(PlayError::InvalidParameter {
                name: "speed while synchronized".into(),
                value: speed,
            });
        }
        Ok(())
    }

    /// Decorates a track with synchronization initially off.
    #[must_use]
    pub fn new(config: LinkedConfig<P>) -> Self {
        let LinkedConfig {
            inner,
            settings: config,
            trajectory: host,
        } = config;
        Self {
            inner,
            config,
            grid: None,
            host,
            mode: SyncMode::Off,
            load: None,
            waiting: None,
            correction: None,
            alignment: None,
            speeds: Vec::new(),
            start: None,
            start_caller: None,
            withdrawn: Vec::new(),
            retimes: Vec::new(),
            cue: None,
        }
    }
}

impl<P> Linked<P> {
    fn earliest<S>(&self, out: &Outbox<'_, S>) -> Result<SessionFrame, PlayError>
    where
        P: Track<S>,
    {
        let pass = out.pass().ok_or(PlayError::Untimed)?;
        Ok(pass.now + self.lead(pass.delivery).unwrap_or(pass.delivery))
    }

    fn planned<S>(&self, at: SessionFrame) -> Result<(Position, f32), PlayError>
    where
        P: Track<S>,
    {
        self.inner.planned(at, self.host.sample_rate())
    }

    fn jump_frame<S>(&self, out: &Outbox<'_, S>) -> Result<SessionFrame, PlayError>
    where
        P: Track<S>,
    {
        Ok(self.earliest(out)? + self.inner.snapshot().as_ref().declick)
    }

    fn correct<S>(
        &mut self,
        at: SessionFrame,
        commanded: bool,
        out: &mut Outbox<'_, S>,
    ) -> Result<Option<Seq>, PlayError>
    where
        P: Track<S>,
    {
        let Some(grid) = self.grid.as_ref() else {
            return Err(PlayError::NotReady);
        };
        let (position, from) = self.planned::<S>(at)?;
        if !covers(grid, position) {
            return Err(PlayError::NotReady);
        }
        if self.lane_room() == 0 {
            return Err(PlayError::Full("lane"));
        }
        let to = speed(&self.host, grid, at);
        let mut plan = checked_correction(
            if commanded { to } else { from },
            to,
            phase_error(&self.host, grid, position, at),
            self.config.epsilon(),
        )
        .ok_or_else(|| {
            PlayError::Internal("correction needs finite, playable epsilon-bounded steps".into())
        })?;
        if commanded {
            plan.steps.insert(
                0,
                crate::CorrectionStep {
                    speed: to,
                    seconds: 0.0,
                },
            );
        }
        let (curve, frame_count) =
            plan.checked_curve(self.host.sample_rate()).ok_or_else(|| {
                PlayError::Internal("correction duration does not fit the output-frame axis".into())
            })?;
        let frames = usize::try_from(frame_count)
            .map(FrameCount::new)
            .map_err(|error| PlayError::Internal(error.to_string()))?;
        let sent = self.inner.apply(
            TrackCommand::SetSpeed {
                speed: curve,
                at: When::At(at),
            },
            out,
        )?;
        if let Some(seq) = sent {
            self.speeds
                .push((seq, SpeedOperation::Correction { commanded }));
            self.correction = Some(RunningCorrection { at, frames });
        }
        Ok(sent)
    }

    fn realign<S>(&mut self, out: &mut Outbox<'_, S>)
    where
        P: Track<S>,
    {
        let Some(commanded) = self.alignment else {
            return;
        };
        let result = self
            .earliest(out)
            .and_then(|at| self.correct(at, commanded, out));
        match result {
            Ok(_) => self.alignment = None,
            Err(PlayError::Full(_) | PlayError::NotReady | PlayError::Untimed) => {}
            Err(error) => warn!(%error, "track realignment refused"),
        }
    }

    fn resume_wait<S>(&mut self, out: &mut Outbox<'_, S>)
    where
        P: Track<S>,
    {
        let Some(waiting) = self.waiting else {
            return;
        };
        let snapshot = self.inner.snapshot();
        let track = snapshot.as_ref();
        if matches!(
            track.status,
            TrackStatus::Idle | TrackStatus::Loading | TrackStatus::Released
        ) || track.pending_lane
            || self.start.is_some()
        {
            return;
        }
        let required = match waiting {
            Waiting::Sync { .. } => track.position,
            _ => waiting.required(),
        };
        if self.synced()
            && self
                .grid
                .as_ref()
                .is_none_or(|grid| !covers(grid, required))
        {
            if matches!(waiting, Waiting::Sync { .. }) {
                self.waiting = Some(Waiting::Sync { required });
            }
            return;
        }
        self.waiting = None;
        let result = match waiting {
            Waiting::Play { at, .. } => self.apply(TrackCommand::Play { at }, out),
            Waiting::Seek { required } => self.apply(TrackCommand::Seek { to: required }, out),
            Waiting::Sync { .. } if self.synced() => self.sync(true, out),
            Waiting::Load { .. } | Waiting::Sync { .. } => Ok(None),
        };
        match result {
            Ok(_) => {}
            Err(PlayError::Full(_) | PlayError::NotReady | PlayError::Untimed) => {
                self.waiting = Some(waiting);
            }
            Err(error) => warn!(%error, "waiting linked command was refused"),
        }
    }

    fn withdraw_start(&mut self, required: Position) {
        if let Some(start) = self.start.take() {
            self.withdrawn.push(start);
            self.waiting = Some(Waiting::Play {
                required,
                at: When::Next,
            });
        }
    }

    fn prepare_cue<S>(&mut self, out: &mut Outbox<'_, S>)
    where
        P: Track<S>,
    {
        let Some(required) = self.cue else {
            return;
        };
        let Some(grid) = self.grid.as_ref() else {
            return;
        };
        let raw = grid.as_raw();
        let next = raw
            .downbeats
            .iter()
            .filter(|_| raw.meter.is_some())
            .map(|beat| beat.at)
            .chain(
                raw.beats
                    .iter()
                    .filter(|_| raw.meter.is_none())
                    .map(|beat| beat.at),
            )
            .find(|seconds| *seconds >= required.as_secs_f64());
        let Some(position) = next.and_then(|seconds| Position::try_from_secs_f64(seconds).ok())
        else {
            return;
        };
        let Some(pass) = out.pass() else {
            return;
        };
        let value = speed(&self.host, grid, pass.earliest());
        match self.inner.cue(position, value, out) {
            Ok(_) => {
                self.cue = None;
                self.withdraw_start(position);
                self.waiting = self.waiting.map(|waiting| match waiting {
                    Waiting::Load { .. } => Waiting::Load { required: position },
                    Waiting::Play { at, .. } => Waiting::Play {
                        required: position,
                        at,
                    },
                    other => other,
                });
            }
            Err(error) => warn!(%error, "synchronized cue waits for its segment"),
        }
    }
}

impl<S, P: Track<S>> Player<S> for Linked<P> {
    type Command = TrackCommand<S>;
    type Snapshot = LinkedSnapshot<P::Snapshot>;

    fn entry(&self, bound: Bound) -> Option<SessionFrame> {
        if !self.synced() {
            return self.inner.entry(bound);
        }
        let track = self.inner.snapshot();
        if matches!(
            track.as_ref().status,
            TrackStatus::Idle | TrackStatus::Loading | TrackStatus::Released
        ) {
            return None;
        }
        self.grid
            .as_ref()
            .and_then(|grid| entry(&self.host, grid, track.as_ref().position, bound))
    }

    fn apply(
        &mut self,
        command: TrackCommand<S>,
        out: &mut Outbox<'_, S>,
    ) -> Result<Option<Seq>, PlayError> {
        if self.synced() && matches!(&command, TrackCommand::Play { at: When::Deferred }) {
            return Err(PlayError::Internal(
                "a linked Play has no deferred executor moment".into(),
            ));
        }
        if self.synced()
            && let TrackCommand::Play {
                at: When::At(frame),
            } = &command
            && *frame < self.earliest(out)?
        {
            return Err(PlayError::Late);
        }
        if let TrackCommand::Load { item, position } = command {
            let seq = self
                .inner
                .apply(TrackCommand::Load { item, position }, out)?;
            self.load = seq;
            self.grid = None;
            self.correction = None;
            self.alignment = None;
            self.speeds.clear();
            self.start = None;
            self.start_caller = None;
            self.withdrawn.clear();
            self.retimes.clear();
            self.cue = self.synced().then_some(position);
            self.waiting = self
                .synced()
                .then_some(Waiting::Load { required: position });
            return Ok(seq);
        }
        if !self.synced() {
            return self.inner.apply(command, out);
        }
        match command {
            TrackCommand::Load { .. } => unreachable!("load was handled before the SYNC dispatch"),
            TrackCommand::Configure(change, at) => {
                self.synced_speed(change)?;
                self.inner.apply(TrackCommand::Configure(change, at), out)
            }
            TrackCommand::Play { at } => {
                let track = self.inner.snapshot();
                let required = track.as_ref().position;
                let Some(grid) = self.grid.as_ref().filter(|grid| covers(grid, required)) else {
                    self.waiting = Some(Waiting::Play { required, at });
                    return Ok(None);
                };
                if matches!(
                    track.as_ref().status,
                    TrackStatus::Idle | TrackStatus::Loading
                ) || track.as_ref().pending_lane
                    || self.cue.is_some()
                {
                    self.waiting = Some(Waiting::Play { required, at });
                    return Ok(None);
                }
                let earliest = self.earliest(out)?;
                let bound = match at {
                    When::Next => earliest,
                    When::At(frame) if frame < earliest => return Err(PlayError::Late),
                    When::At(frame) => frame,
                    When::Deferred => {
                        return Err(PlayError::Internal(
                            "a linked Play has no deferred executor moment".into(),
                        ));
                    }
                };
                let frame = entry(&self.host, grid, required, Bound::AtOrAfter(bound))
                    .unwrap_or_else(|| unreachable!("the grid covers the requested entry"));
                let sent = self.inner.apply(
                    TrackCommand::Play {
                        at: When::At(frame),
                    },
                    out,
                )?;
                self.waiting = None;
                self.start = sent;
                Ok(sent)
            }
            TrackCommand::Seek { to } => {
                if !matches!(
                    self.inner.snapshot().as_ref().status,
                    TrackStatus::Playing { .. }
                ) {
                    let sent = self.inner.apply(TrackCommand::Seek { to }, out)?;
                    if let Some(start) = self.start {
                        self.start_caller.get_or_insert(start);
                    }
                    self.withdraw_start(to);
                    if self.cue.is_some() {
                        self.cue = Some(to);
                    }
                    return Ok(sent);
                }
                let Some(grid) = self.grid.as_ref().filter(|grid| covers(grid, to)) else {
                    self.waiting = Some(Waiting::Seek { required: to });
                    return Ok(None);
                };
                let at = self.jump_frame(out)?;
                let target = jump_target(to, phase_error(&self.host, grid, to, at));
                let speed = speed(&self.host, grid, at);
                let synchronized = self.inner.projected().speed() == speed;
                let command = if synchronized {
                    TrackCommand::Jump { to: target, at }
                } else {
                    TrackCommand::Align {
                        to: target,
                        speed,
                        at,
                    }
                };
                let sent = self.inner.apply(command, out)?;
                if !synchronized && let Some(seq) = sent {
                    self.speeds.push((seq, SpeedOperation::Relocation { to }));
                }
                self.waiting = None;
                Ok(sent)
            }
            TrackCommand::Pause { at } => {
                self.waiting = None;
                self.inner.apply(TrackCommand::Pause { at }, out)
            }
            TrackCommand::Jump { to, at } => self.inner.apply(TrackCommand::Jump { to, at }, out),
            TrackCommand::Align { to, speed, at } => {
                self.inner.apply(TrackCommand::Align { to, speed, at }, out)
            }
            TrackCommand::SetSpeed { speed, at } => {
                self.inner.apply(TrackCommand::SetSpeed { speed, at }, out)
            }
            TrackCommand::SetHostRate { rate } => {
                self.inner.apply(TrackCommand::SetHostRate { rate }, out)
            }
            TrackCommand::Fade { at, settings, dir } => self
                .inner
                .apply(TrackCommand::Fade { at, settings, dir }, out),
            TrackCommand::PlayAfter { track } => {
                self.inner.apply(TrackCommand::PlayAfter { track }, out)
            }
            TrackCommand::Supersede => self.inner.apply(TrackCommand::Supersede, out),
            TrackCommand::Release => self.inner.apply(TrackCommand::Release, out),
            TrackCommand::Seat { slot, at } => {
                self.inner.apply(TrackCommand::Seat { slot, at }, out)
            }
        }
    }

    fn settle(&mut self, receipt: TrackReceipt<'_, S>, out: &mut Outbox<'_, S>) -> Settled {
        let snapshot = self.inner.snapshot();
        let track = snapshot.as_ref();
        let slot = track.slot;
        let underrun = track.attached
            && matches!(track.status, TrackStatus::Playing { .. })
            && matches!(&receipt,
                TrackReceipt::Event(DeckEvent::Underrun { slot: named, .. }) if Some(*named) == slot
            );
        let mut settled = self.inner.settle(receipt, out);
        if let Settled::Rejected {
            seq,
            reason: Rejection::Stale,
        } = &settled
            && let Some(index) = self.withdrawn.iter().position(|withdrawn| withdrawn == seq)
        {
            self.withdrawn.remove(index);
            return Settled::Pending;
        }
        if matches!(&settled,
            Settled::Applied { seq, .. } | Settled::Rejected { seq, .. }
                if self.start == Some(*seq)
        ) {
            self.start = None;
            if let Some(caller) = self.start_caller.take() {
                match &mut settled {
                    Settled::Applied { seq, .. } | Settled::Rejected { seq, .. } => {
                        *seq = caller;
                    }
                    Settled::Pending => {}
                }
            }
        }
        if self.synced() && underrun {
            self.alignment = Some(false);
            self.realign(out);
        }
        if self.cue.is_none() && matches!(self.waiting, Some(Waiting::Load { .. })) {
            self.resume_wait(out);
        }
        settled
    }

    fn tick(&mut self, now: SessionFrame, out: &mut Outbox<'_, S>) {
        if !self.speeds.is_empty() {
            while let Some(receipt) = self.inner.speed_receipt() {
                let seq = match &receipt {
                    Settled::Pending => continue,
                    Settled::Applied { seq, .. } | Settled::Rejected { seq, .. } => *seq,
                };
                let Some(index) = self.speeds.iter().position(|(pending, _)| *pending == seq)
                else {
                    continue;
                };
                let (_, operation) = self.speeds.remove(index);
                if self.synced()
                    && matches!(
                        receipt,
                        Settled::Rejected {
                            reason: Rejection::Late | Rejection::Refused(_),
                            ..
                        }
                    )
                {
                    self.correction = None;
                    match operation {
                        SpeedOperation::Correction { commanded } => {
                            self.alignment = Some(commanded);
                        }
                        SpeedOperation::Alignment => {
                            self.waiting = Some(Waiting::Sync {
                                required: self.inner.snapshot().as_ref().position,
                            });
                        }
                        SpeedOperation::Relocation { to } => {
                            self.waiting = Some(Waiting::Seek { required: to });
                        }
                    }
                }
            }
        }
        self.inner.tick(now, out);
        if let Some(correction) = &self.correction
            && now
                .frames_since(correction.at)
                .is_some_and(|elapsed| elapsed >= correction.frames.get() as u64)
        {
            self.correction = None;
        }
        if self.synced() {
            self.realign(out);
        }
        if self.synced() {
            self.prepare_cue(out);
        }
        if self.cue.is_none() {
            self.resume_wait(out);
        }
    }

    fn snapshot(&self) -> Self::Snapshot {
        let sync = match self.mode {
            SyncMode::Off => SyncStatus::Off,
            SyncMode::Unsyncable => SyncStatus::Unsyncable,
            SyncMode::On => match self.waiting {
                Some(waiting) => SyncStatus::WaitingForGrid {
                    required: waiting.required(),
                },
                None if self.correction.is_some() => {
                    let remaining = self.correction.as_ref().map_or(0, |correction| {
                        let elapsed = self
                            .inner
                            .snapshot()
                            .as_ref()
                            .mark
                            .and_then(|mark| mark.session.frames_since(correction.at))
                            .unwrap_or(0);
                        usize::try_from(elapsed)
                            .map_or(0, |elapsed| correction.frames.get().saturating_sub(elapsed))
                    });
                    SyncStatus::Correcting {
                        remaining: FrameCount::new(remaining),
                    }
                }
                None => SyncStatus::On,
            },
        };
        LinkedSnapshot {
            track: self.inner.snapshot(),
            sync,
        }
    }
}

impl<S, P: Track<S>> Track<S> for Linked<P> {
    fn admit(
        &mut self,
        change: TrackSettingsChange,
        at: When<SessionFrame>,
        out: &Outbox<'_, S>,
    ) -> Result<(), PlayError> {
        self.synced_speed(change)?;
        self.inner.admit(change, at, out)
    }

    delegate::delegate! {
        to self.inner {
            fn projected(&self) -> TrackSettings;
            fn planned(
                &self,
                at: SessionFrame,
                sample_rate: std::num::NonZeroU32,
            ) -> Result<(Position, f32), PlayError>;
            fn planned_end(
                &self,
                sample_rate: std::num::NonZeroU32,
            ) -> Result<Option<SessionFrame>, PlayError>;
            fn speed_receipt(&mut self) -> Option<Settled>;
            fn speed_applied(&mut self, seq: Seq) -> Option<bool>;
            fn finish_group(&mut self, result: Result<Seq, &mut Vec<DeckPart>>);
            fn cue(
                &mut self,
                position: Position,
                speed: f32,
                out: &mut Outbox<'_, S>,
            ) -> Result<Option<Seq>, PlayError>;
        }
    }
}

impl<P: DeckControl> DeckControl for Linked<P> {
    type Control = P::Control;

    fn control(&self) -> Self::Control {
        self.inner.control()
    }
}

impl<S, P> HostedDeck<S> for Linked<P>
where
    P: Track<S> + HostedDeck<S>,
{
    delegate::delegate! {
        to self.inner {
            fn worker(&self) -> Option<&PlayWorker<S>>;
            fn resource_prep(&self) -> Option<&kithara_play::ResourcePrep<S>>;
            fn mixer_config(&self) -> DeckMixerConfig;
            fn drain(&mut self, pass: DeckPass<'_>, out: &mut Outbox<'_, S>);
            fn close(&mut self, out: &mut Outbox<'_, S>) -> Result<(), PlayError>;
            fn hold(&mut self, waker: Waker);
            fn release(&mut self);
        }
    }

    fn settle(
        &mut self,
        receipt: TrackReceipt<'_, S>,
        _pass: DeckPass<'_>,
        out: &mut Outbox<'_, S>,
    ) {
        Player::settle(self, receipt, out);
    }

    fn tick(&mut self, pass: DeckPass<'_>, out: &mut Outbox<'_, S>) {
        Player::tick(self, pass.now, out);
    }
}

impl<P> Linked<P> {
    fn align<S>(&mut self, out: &mut Outbox<'_, S>) -> Result<Option<Seq>, PlayError>
    where
        P: Track<S>,
    {
        let track = self.inner.snapshot();
        let required = track.as_ref().position;
        let Some(grid) = self.grid.as_ref().filter(|grid| covers(grid, required)) else {
            self.waiting = Some(Waiting::Sync { required });
            return Ok(None);
        };
        if matches!(track.as_ref().status, TrackStatus::Playing { .. }) {
            let at = self.jump_frame(out)?;
            let (position, _speed) = self.planned::<S>(at)?;
            if !covers(grid, position) {
                self.waiting = Some(Waiting::Sync { required: position });
                return Ok(None);
            }
            if self.lane_room() == 0 {
                return Err(PlayError::Full("lane"));
            }
            if out.deck_available() == 0 {
                return Err(PlayError::Full("deck"));
            }
            let to = jump_target(position, phase_error(&self.host, grid, position, at));
            let sent = self.inner.apply(
                TrackCommand::Align {
                    to,
                    speed: speed(&self.host, grid, at),
                    at,
                },
                out,
            )?;
            if let Some(seq) = sent {
                self.speeds.push((seq, SpeedOperation::Alignment));
            }
            self.correction = None;
            self.waiting = None;
            return Ok(sent);
        }
        let at = self.earliest(out)?;
        self.inner.cue(required, speed(&self.host, grid, at), out)
    }
}

impl<S, P: Track<S>> LinkedPlayer<S> for Linked<P> {
    fn sync(&mut self, on: bool, out: &mut Outbox<'_, S>) -> Result<Option<Seq>, PlayError> {
        let previous = self.mode;
        self.mode = if on { SyncMode::On } else { SyncMode::Off };
        if !on {
            self.waiting = None;
            self.cue = None;
            self.alignment = None;
            return Ok(None);
        }
        let result = self.align(out);
        if result.is_err() {
            self.mode = previous;
        }
        result
    }

    fn retime(&mut self, trajectory: &TempoTrajectory, at: SessionFrame, out: &mut Outbox<'_, S>) {
        self.host = trajectory.clone();
        if !self.synced() {
            return;
        }
        let Some(grid) = self.grid.as_ref() else {
            return;
        };
        if matches!(
            self.inner.snapshot().as_ref().status,
            TrackStatus::Playing { .. }
        ) {
            match self.correct(at, true, out) {
                Ok(Some(seq)) => self.retimes.push((at, seq)),
                Ok(None) => {}
                Err(PlayError::Late) => {
                    self.alignment = Some(true);
                    self.realign(out);
                }
                Err(PlayError::Full(_) | PlayError::NotReady | PlayError::Untimed) => {
                    self.alignment = Some(true);
                }
                Err(error) => warn!(?at, %error, "track retime refused"),
            }
            return;
        }
        let required = self.inner.snapshot().as_ref().position;
        match self.inner.cue(required, speed(&self.host, grid, at), out) {
            Ok(Some(seq)) => {
                self.retimes.push((at, seq));
                if let Some(start) = self.start {
                    self.start_caller.get_or_insert(start);
                }
                self.withdraw_start(required);
            }
            Ok(None) => {}
            Err(error) => warn!(?at, %error, "track retime refused"),
        }
    }

    fn grid(&mut self, answer: GridAnswer, out: &mut Outbox<'_, S>) {
        if self.load != Some(answer.load) || self.inner.snapshot().as_ref().item != answer.item {
            return;
        }
        match answer.model {
            Ok(model) => {
                if self.grid.as_ref() == Some(&model) {
                    return;
                }
                self.grid = Some(model);
                if self.synced() {
                    if matches!(
                        self.inner.snapshot().as_ref().status,
                        TrackStatus::Playing { .. }
                    ) && self.waiting.is_none()
                    {
                        self.alignment = Some(false);
                        self.realign(out);
                        return;
                    }
                    if self.start.is_some() {
                        self.cue = Some(self.inner.snapshot().as_ref().position);
                    }
                    self.prepare_cue(out);
                    if self.cue.is_none() {
                        self.resume_wait(out);
                    }
                }
            }
            Err(refusal) => {
                warn!(item = ?answer.item, load = ?answer.load, %refusal, "track grid unavailable");
                self.grid = None;
                if self.synced() {
                    self.mode = SyncMode::Unsyncable;
                    self.cue = None;
                    self.alignment = None;
                    self.resume_wait(out);
                }
            }
        }
    }

    fn synced(&self) -> bool {
        self.mode == SyncMode::On
    }

    fn lead(&self, delivery: FrameCount) -> Option<FrameCount> {
        if !self.synced()
            || !matches!(
                self.inner.snapshot().as_ref().status,
                TrackStatus::Playing { .. }
            )
        {
            return None;
        }
        let snapshot = self.inner.snapshot();
        let track = snapshot.as_ref();
        Some(FrameCount::new(
            delivery
                .get()
                .saturating_add(track.ring_depth.get())
                .saturating_add(track.engine_latency.get()),
        ))
    }

    fn lane_room(&self) -> usize {
        if !self.synced() || self.grid.is_none() {
            usize::MAX
        } else {
            self.inner.snapshot().as_ref().lane_room
        }
    }

    fn scope_parts(&self) -> usize {
        let snapshot = self.inner.snapshot();
        usize::from(
            self.synced()
                && self.grid.is_some()
                && snapshot.as_ref().attached
                && !matches!(snapshot.as_ref().status, TrackStatus::Playing { .. }),
        )
    }

    fn retime_applied(&mut self, at: SessionFrame) -> Option<bool> {
        let mut awaiting = false;
        for &(frame, seq) in &self.retimes {
            if frame != at {
                continue;
            }
            match self.inner.speed_applied(seq) {
                Some(true) => return Some(true),
                None => awaiting = true,
                Some(false) => {}
            }
        }
        (!awaiting).then_some(false)
    }

    fn realign(&mut self, trajectory: &TempoTrajectory, at: SessionFrame, out: &mut Outbox<'_, S>) {
        if !matches!(
            self.inner.snapshot().as_ref().status,
            TrackStatus::Playing { .. }
        ) {
            self.retime(trajectory, at, out);
            return;
        }
        self.host = trajectory.clone();
        if self.synced()
            && let Err(error) = self.correct(at, false, out)
        {
            self.alignment = Some(false);
            warn!(?at, %error, "tempo phase correction awaits a lane");
        }
    }
}
