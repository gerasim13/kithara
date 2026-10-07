use kithara_command::{Seq, When};
use kithara_config::Config;
use kithara_play::{
    Bound, Outbox, PlayError, Player, Position, Settled, Track, TrackCommand, TrackReceipt,
    TrackSettings, TrackSettingsChange, TrackSnapshot, TrackStatus,
};
use kithara_signal::{FrameCount, SessionFrame};
use kithara_warp::SpeedCurve;
use tracing::warn;

use crate::{
    CorrectionPlan, GridAnswer, LinkError, TempoTrajectory, TrackGrid, entry, jump_target,
    phase_error, speed,
};

/// A player that can align itself and receive the Host's planned tempo trajectory.
pub trait LinkedPlayer<S>: Player<S> {
    /// Enables alignment, including an explicit phase jump while sounding;
    /// disabling alignment sends nothing and preserves in-flight speeds.
    ///
    /// # Errors
    /// Returns the refusal of the speed or jump batch, if one is sent.
    fn sync(&mut self, on: bool, out: &mut Outbox<'_, S>) -> Result<Option<Seq>, PlayError>;

    /// Replaces the Host trajectory, changing sounding lanes on `at`, silent
    /// lanes on Next, and withdrawing and replanning any waiting start.
    fn retime(&mut self, trajectory: &TempoTrajectory, at: SessionFrame, out: &mut Outbox<'_, S>);

    /// Accepts analysis for the current load only: resumes grid waits, applies
    /// silent refinements immediately, and corrects sounding phase without a jump.
    fn grid(&mut self, answer: GridAnswer, out: &mut Outbox<'_, S>);

    /// Whether the Host should send this player tempo changes.
    fn synced(&self) -> bool;

    /// The lane's required lead while sounding in SYNC; silent players return None.
    fn lead(&self) -> Option<FrameCount>;

    /// Available room in all lane queues that a retime would send to.
    fn lane_room(&self) -> usize;
}

/// Maximum adjacent speed step used for inaudible phase correction.
#[derive(Clone, Copy, Debug, PartialEq, Config)]
#[config(default, check(error = LinkError), fields(value, get(copy)))]
pub struct LinkConfig {
    #[config(check = check_epsilon, builder(default = 0.001))]
    epsilon: f32,
}

fn check_epsilon(epsilon: f32) -> Result<f32, LinkError> {
    if epsilon.is_finite() && epsilon > 0.0 {
        Ok(epsilon)
    } else {
        Err(LinkError::Epsilon { epsilon })
    }
}

/// Whether Host synchronization owns speed and phase for this track.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SyncMode {
    Off,
    On,
    /// Analysis refused; playback continues without synchronization.
    Unsyncable,
}

/// Synchronization progress beside the underlying track snapshot.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SyncStatus {
    Off,
    On,
    WaitingForGrid { required: Position },
    Unsyncable,
    Correcting { remaining: FrameCount },
}

/// An owner command waiting for a grid that covers its media position.
#[derive(Clone, Copy, Debug)]
pub enum Waiting {
    Load {
        required: Position,
    },
    Play {
        required: Position,
        at: When<SessionFrame>,
    },
    Sync {
        required: Position,
    },
    Seek {
        required: Position,
    },
}

impl Waiting {
    fn required(self) -> Position {
        match self {
            Self::Load { required }
            | Self::Play { required, .. }
            | Self::Sync { required }
            | Self::Seek { required } => required,
        }
    }
}

/// Track state and its synchronization admission or correction state.
#[derive(Clone, Debug)]
pub struct LinkedSnapshot<T> {
    pub track: T,
    pub sync: SyncStatus,
}

impl<T: AsRef<TrackSnapshot>> AsRef<TrackSnapshot> for LinkedSnapshot<T> {
    fn as_ref(&self) -> &TrackSnapshot {
        self.track.as_ref()
    }
}

/// One track decorated with analysis, Host tempo and owner-thread synchronization.
pub struct Linked<P> {
    inner: P,
    config: LinkConfig,
    grid: Option<TrackGrid>,
    host: TempoTrajectory,
    pub(crate) mode: SyncMode,
    load: Option<Seq>,
    waiting: Option<Waiting>,
    correction: Option<CorrectionPlan>,
}

impl<P> Linked<P> {
    /// Decorates a track with synchronization initially off.
    #[must_use]
    pub fn new(inner: P, config: LinkConfig, host: TempoTrajectory) -> Self {
        Self {
            inner,
            config,
            grid: None,
            host,
            mode: SyncMode::Off,
            load: None,
            waiting: None,
            correction: None,
        }
    }

    fn earliest<S>(&self, out: &Outbox<'_, S>) -> SessionFrame {
        let _ = out;
        todo!(
            "Read now + delivery, or now + lead_lane while sounding, from the owner/outbox clock seam (spec §3.4)"
        )
    }
}

impl<S, P: Track<S>> Player<S> for Linked<P> {
    type Command = TrackCommand<S>;
    type Snapshot = LinkedSnapshot<P::Snapshot>;

    fn entry(&self, bound: Bound) -> SessionFrame {
        if !self.synced() {
            return self.inner.entry(bound);
        }
        let track = self.inner.snapshot();
        match self
            .grid
            .as_ref()
            .and_then(|grid| entry(&self.host, grid, track.as_ref().position, bound))
        {
            Some(frame) => frame,
            None => todo!(
                "entry(bound): wait inside Linked for a grid covering the required position (spec §4.8)"
            ),
        }
    }

    fn apply(
        &mut self,
        command: TrackCommand<S>,
        out: &mut Outbox<'_, S>,
    ) -> Result<Option<Seq>, PlayError> {
        if let TrackCommand::Load { item, position } = command {
            let seq = self
                .inner
                .apply(TrackCommand::Load { item, position }, out)?;
            self.load = seq;
            self.grid = None;
            self.correction = None;
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
            TrackCommand::Configure(TrackSettingsChange::Speed(_), _) => {
                todo!(
                    "Configure(Speed): Rejected(Synced); add the missing synced-refusal variant to PlayError in its owning subtask (spec §4.5)"
                )
            }
            TrackCommand::Configure(change, at) => {
                self.inner.apply(TrackCommand::Configure(change, at), out)
            }
            TrackCommand::Play { at } => {
                let track = self.inner.snapshot();
                let required = track.as_ref().position;
                let Some(grid) = self.grid.as_ref().filter(|grid| grid.covers(required)) else {
                    self.waiting = Some(Waiting::Play { required, at });
                    return Ok(None);
                };
                if matches!(
                    track.as_ref().status,
                    TrackStatus::Idle | TrackStatus::Loading
                ) {
                    self.waiting = Some(Waiting::Play { required, at });
                    return Ok(None);
                }
                let earliest = self.earliest(out);
                let bound = match at {
                    When::Next => earliest,
                    When::At(frame) if frame < earliest => return Err(PlayError::Late),
                    When::At(frame) => frame,
                };
                let frame = match entry(&self.host, grid, required, Bound::AtOrAfter(bound)) {
                    Some(frame) => frame,
                    None => unreachable!("the grid covers the requested entry"),
                };
                self.inner.apply(
                    TrackCommand::Play {
                        at: When::At(frame),
                    },
                    out,
                )
            }
            TrackCommand::Seek { to } => {
                if !matches!(
                    self.inner.snapshot().as_ref().status,
                    TrackStatus::Playing { .. }
                ) {
                    return self.inner.apply(TrackCommand::Seek { to }, out);
                }
                let Some(grid) = self.grid.as_ref().filter(|grid| grid.covers(to)) else {
                    self.waiting = Some(Waiting::Seek { required: to });
                    return Ok(None);
                };
                let at = self.earliest(out);
                let to = jump_target(to, phase_error(&self.host, grid, to, at));
                self.inner.apply(TrackCommand::Jump { to, at }, out)
            }
            TrackCommand::Pause { at } => self.inner.apply(TrackCommand::Pause { at }, out),
            TrackCommand::Jump { to, at } => self.inner.apply(TrackCommand::Jump { to, at }, out),
            TrackCommand::SetSpeed { .. } => todo!(
                "Retime: sounding -> SetSpeed on F; silent -> SetSpeed on Next; replace pending starts (spec §4.5)"
            ),
            TrackCommand::Fade { at, settings, dir } => self
                .inner
                .apply(TrackCommand::Fade { at, settings, dir }, out),
            TrackCommand::PlayAfter { .. } => todo!(
                "entry(bound): use the same in-phase entry family for a successor; queue transitions never jump (spec §4.5)"
            ),
            TrackCommand::Release => self.inner.apply(TrackCommand::Release, out),
            TrackCommand::Evict { at } => self.inner.apply(TrackCommand::Evict { at }, out),
        }
    }

    fn settle(&mut self, receipt: TrackReceipt<'_, S>, out: &mut Outbox<'_, S>) -> Settled {
        if !self.synced() {
            return self.inner.settle(receipt, out);
        }
        let _ = (receipt, out);
        todo!(
            "Underrun: same phase jump as Sync(on) while playing; lane Late: correction without changing Host trajectory; readiness: resume waiting Play (spec §4.5/§4.6)"
        )
    }

    fn tick(&mut self, now: SessionFrame, out: &mut Outbox<'_, S>) {
        self.inner.tick(now, out);
        if self.synced() && (self.waiting.is_some() || self.correction.is_some()) {
            todo!(
                "Advance grid/readiness waits and the remaining correction from lane receipts (spec §4.8)"
            )
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
                    todo!("Correcting: remaining frames from the executing correction (spec §4.8)")
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
    fn projected(&self) -> TrackSettings {
        self.inner.projected()
    }
}

impl<S, P: Track<S>> LinkedPlayer<S> for Linked<P> {
    fn sync(&mut self, on: bool, out: &mut Outbox<'_, S>) -> Result<Option<Seq>, PlayError> {
        self.mode = if on { SyncMode::On } else { SyncMode::Off };
        if !on {
            return Ok(None);
        }
        let track = self.inner.snapshot();
        let required = track.as_ref().position;
        let Some(grid) = self.grid.as_ref().filter(|grid| grid.covers(required)) else {
            self.waiting = Some(Waiting::Sync { required });
            return Ok(None);
        };
        if matches!(track.as_ref().status, TrackStatus::Playing { .. }) {
            todo!(
                "Sync(on) while playing, including repeated Sync(on): SetSpeed on F and Jump to the nearest phase match with declick, not speed correction (spec §4.5)"
            )
        }
        self.inner.apply(
            TrackCommand::SetSpeed {
                speed: SpeedCurve::Constant(speed(&self.host, grid, self.earliest(out))),
                at: When::Next,
            },
            out,
        )
    }

    fn retime(&mut self, trajectory: &TempoTrajectory, at: SessionFrame, out: &mut Outbox<'_, S>) {
        self.host = trajectory.clone();
        if !self.synced() {
            return;
        }
        let Some(grid) = self.grid.as_ref() else {
            return;
        };
        if self.waiting.is_some() {
            todo!(
                "Retime: withdraw and replan the waiting start using the new trajectory (spec §4.5)"
            )
        }
        let when = if matches!(
            self.inner.snapshot().as_ref().status,
            TrackStatus::Playing { .. }
        ) {
            When::At(at)
        } else {
            When::Next
        };
        if let Err(error) = self.inner.apply(
            TrackCommand::SetSpeed {
                speed: SpeedCurve::Constant(speed(&self.host, grid, at)),
                at: when,
            },
            out,
        ) {
            warn!(?at, %error, "track retime refused");
            todo!(
                "Retime refusal: correct the affected lane without changing the Host trajectory (spec §4.6)"
            )
        }
    }

    fn grid(&mut self, answer: GridAnswer, out: &mut Outbox<'_, S>) {
        if self.load != Some(answer.load) || self.inner.snapshot().as_ref().item != answer.item {
            return;
        }
        match answer.model {
            Ok(model) => {
                self.grid = Some(TrackGrid::from(model));
                if self.synced() {
                    let _ = out;
                    todo!(
                        "Grid: fulfill the wait; loaded cue -> first downbeat not before cue plus speed on Next; sounding -> bounded speed correction replacing its remainder (spec §4.5/§4.8)"
                    )
                }
            }
            Err(refusal) => {
                warn!(item = ?answer.item, load = ?answer.load, %refusal, "track grid unavailable");
                self.grid = None;
                if self.synced() {
                    self.mode = SyncMode::Unsyncable;
                    todo!(
                        "Grid refusal: end the wait and execute its user command without SYNC (spec §4.8)"
                    )
                }
            }
        }
    }

    fn synced(&self) -> bool {
        self.mode == SyncMode::On
    }

    fn lead(&self) -> Option<FrameCount> {
        if !self.synced()
            || !matches!(
                self.inner.snapshot().as_ref().status,
                TrackStatus::Playing { .. }
            )
        {
            return None;
        }
        todo!("Read lead_lane from the sounding track's render-lane owner seam (spec §3.4)")
    }

    fn lane_room(&self) -> usize {
        todo!("Read Sender::available on every lane a Retime would use (spec §4.6 step 2)")
    }
}
