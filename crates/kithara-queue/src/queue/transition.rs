use std::num::NonZeroU32;

use kithara_bufpool::HasPool;
use kithara_command::{Rejection, Seq, When};
use kithara_events::TrackId;
use kithara_platform::time::Duration;
use kithara_play::{
    Bound, FadeDir, Outbox, PlayError, Player, Settled, TrackCommand, TrackFactory,
    TrackStatus as PlayingStatus,
};
use kithara_signal::{AudioSpec, FrameCount, SessionFrame};

use super::{Queue, Transition, command::play_error, slots::Role, types::Target};
use crate::{ActionAtItemEnd, AdvanceReason, QueueError, QueueEvent, RepeatMode, TrackStatus};

impl<S, F> Queue<S, F>
where
    S: HasPool<u8> + HasPool<f32> + Send + Sync + 'static,
    F: TrackFactory<S>,
{
    pub(super) fn next_target(
        &mut self,
        transition: Transition,
        reason: AdvanceReason,
        auto: bool,
        out: &mut Outbox<'_, S>,
    ) -> Result<Option<Seq>, QueueError> {
        let ids = self.track_ids();
        let wrap = self.navigation.repeat_mode() == RepeatMode::All;
        match self.navigation.next(&ids, auto, wrap) {
            Some(id) => self.request_transition(id, transition, reason, auto, out),
            None => Ok(None),
        }
    }

    pub(super) fn request_transition(
        &mut self,
        id: TrackId,
        transition: Transition,
        reason: AdvanceReason,
        auto: bool,
        out: &mut Outbox<'_, S>,
    ) -> Result<Option<Seq>, QueueError> {
        let settings = transition
            .settings(self.settings.config().crossfade())
            .validate()?;
        let bound = if auto {
            let duration = if self.settings.config().gapless() {
                0.0
            } else {
                settings.duration
            };
            Bound::AtOrBefore(
                self.current_end().ok_or(PlayError::Untimed)? - self.fade_frames(duration)?,
            )
        } else {
            Bound::AtOrAfter(self.earliest()?)
        };
        self.cancel_target(out)?;
        self.target = Some(Target {
            to: id,
            bound,
            settings,
            transition,
            reason,
            auto,
            stale: None,
        });
        if !auto {
            self.navigation.select(id, &self.track_ids());
        }
        match self.load_track(id, Role::Incoming { batch: None }, out) {
            Ok(load) => match self.transition_loaded(out) {
                Ok(sent) => Ok(sent.or(load)),
                Err(error) => {
                    self.cancel_target(out)?;
                    Err(error.into())
                }
            },
            Err(error) => {
                self.target = None;
                self.tracks.fail(id, &error);
                self.announce(QueueEvent::TrackLoadFailed {
                    id,
                    reason: error.to_string(),
                    auto_skipped: false,
                });
                Err(error)
            }
        }
    }

    /// Step 2: an attached target can enter; an open in flight only waits.
    pub(super) fn transition_loaded(
        &mut self,
        out: &mut Outbox<'_, S>,
    ) -> Result<Option<Seq>, PlayError> {
        let Some(target) = self.target.filter(|target| target.stale.is_none()) else {
            return Ok(None);
        };
        let Some(index) = self.incoming_index(target.to) else {
            return Ok(None);
        };
        let Some(active) = self.active.get(index) else {
            return Ok(None);
        };
        if !matches!(active.role, Role::Incoming { batch: None }) || active.load.is_some() {
            return Ok(None);
        }
        if !matches!(
            active.track.snapshot().as_ref().status,
            PlayingStatus::Loaded | PlayingStatus::Paused { .. }
        ) {
            return Ok(None);
        }
        let frame = self.transition_entry(index, target.bound)?;
        self.send_transition(index, frame, out)
    }

    /// Step 3: a deadline entry may not precede what this pass can deliver.
    fn transition_entry(&self, index: usize, bound: Bound) -> Result<SessionFrame, PlayError> {
        let earliest = self.earliest()?;
        let active = self.active.get(index).ok_or(PlayError::NoActiveSlot)?;
        let frame = active.track.entry(bound);
        Ok(if frame < earliest {
            active.track.entry(Bound::AtOrAfter(earliest))
        } else {
            frame
        })
    }

    /// Step 4: both envelopes, or the gapless chain, share one deck batch.
    fn send_transition(
        &mut self,
        incoming: usize,
        frame: SessionFrame,
        out: &mut Outbox<'_, S>,
    ) -> Result<Option<Seq>, PlayError> {
        let target = self.target.ok_or(PlayError::NotReady)?;
        let current = self.active_current_index();
        let predecessor = current
            .and_then(|index| self.active.get(index))
            .filter(|active| {
                matches!(
                    active.track.snapshot().as_ref().status,
                    PlayingStatus::Playing { .. }
                )
            })
            .map(|active| active.slot);
        let chain = target.auto && self.settings.config().gapless() && predecessor.is_some();
        let at = if chain { When::Next } else { When::At(frame) };
        let (_, sent) = out.together(at, |out| {
            let active = self
                .active
                .get_mut(incoming)
                .ok_or(PlayError::NoActiveSlot)?;
            if chain {
                active.track.apply(
                    TrackCommand::PlayAfter {
                        track: predecessor.ok_or(PlayError::NoActiveSlot)?,
                    },
                    out,
                )?;
            } else {
                active.track.apply(
                    TrackCommand::Fade {
                        at,
                        settings: target.settings,
                        dir: FadeDir::In,
                    },
                    out,
                )?;
                if let Some(index) = current {
                    self.active
                        .get_mut(index)
                        .ok_or(PlayError::NoActiveSlot)?
                        .track
                        .apply(
                            TrackCommand::Fade {
                                at,
                                settings: target.settings,
                                dir: FadeDir::Out,
                            },
                            out,
                        )?;
                }
            }
            Ok(())
        })?;
        if let Some(batch) = sent {
            self.active
                .get_mut(incoming)
                .ok_or(PlayError::NoActiveSlot)?
                .role = Role::Incoming { batch: Some(batch) };
        }
        Ok(sent)
    }

    /// Step 5: only the batch that starts the target changes the sounding item.
    fn transition_applied(&mut self, seq: Seq, at: SessionFrame) -> Result<(), PlayError> {
        let Some(target) = self.target else {
            return Ok(());
        };
        let Some(index) = self.incoming_index(target.to) else {
            return Ok(());
        };
        if !self
            .active
            .get(index)
            .is_some_and(|active| active.role == (Role::Incoming { batch: Some(seq) }))
        {
            return Ok(());
        }
        if let Some(current) = self.active_current_index() {
            let slot = self
                .active
                .get(current)
                .ok_or(PlayError::NoActiveSlot)?
                .slot;
            self.active
                .get_mut(current)
                .ok_or(PlayError::NoActiveSlot)?
                .role = Role::Outgoing;
            if !(target.auto && self.settings.config().gapless()) {
                self.active
                    .fade_out(slot, at + self.fade_frames(target.settings.duration)?);
            }
        }
        self.active
            .get_mut(index)
            .ok_or(PlayError::NoActiveSlot)?
            .role = Role::Current;
        self.current = Some(target.to);
        if target.auto {
            self.navigation.select(target.to, &self.track_ids());
        }
        self.target = None;
        self.announce(QueueEvent::CurrentTrackChanged { id: self.current });
        self.announce(QueueEvent::CurrentTrackAdvance {
            id: self.current,
            reason: target.reason,
        });
        if target.settings.duration > 0.0 && !(target.auto && self.settings.config().gapless()) {
            self.announce(QueueEvent::CrossfadeStarted {
                settings: target.settings,
            });
        }
        Ok(())
    }

    pub(super) fn transition_settled(
        &mut self,
        index: usize,
        settled: &Settled,
        out: &mut Outbox<'_, S>,
    ) -> Result<(), PlayError> {
        let (seq, applied) = match settled {
            Settled::Pending => return Ok(()),
            Settled::Applied { seq, .. } => (*seq, true),
            Settled::Rejected { seq, .. } => (*seq, false),
        };
        let Some(active) = self.active.get(index) else {
            return Ok(());
        };
        if active.load == Some(seq) {
            let id = active.item;
            let metadata = active.track.snapshot().as_ref().metadata.clone();
            self.active
                .get_mut(index)
                .ok_or(PlayError::NoActiveSlot)?
                .load = None;
            if applied && self.tracks.loaded(id, &metadata) {
                if let Some(position) = self
                    .tracks
                    .records()
                    .iter()
                    .position(|record| record.id == id)
                {
                    self.announce(QueueEvent::NextTrackReady {
                        id,
                        index: position,
                    });
                }
                if let Some(sent) = self.transition_loaded(out)? {
                    self.retarget_answers(seq, sent);
                }
            } else if !applied {
                if let Settled::Rejected { reason, .. } = settled
                    && self.tracks.records().iter().any(|record| {
                        record.id == id
                            && matches!(record.status, TrackStatus::Loading | TrackStatus::Slow)
                    })
                {
                    let error = QueueError::Play(super::hosted::refusal(reason));
                    self.tracks.fail(id, &error);
                    self.announce(QueueEvent::TrackLoadFailed {
                        id,
                        reason: error.to_string(),
                        auto_skipped: false,
                    });
                }
                self.release_track(index, out)?;
                if self.target.is_some_and(|target| target.to == id) {
                    self.target = None;
                }
            } else {
                self.cancel_target(out).map_err(play_error)?;
            }
            return Ok(());
        }
        if self.target.is_some_and(|target| target.stale == Some(seq))
            && matches!(active.role, Role::Incoming { .. })
        {
            match settled {
                Settled::Rejected {
                    reason: Rejection::Stale,
                    ..
                } => {
                    let target = self.target.as_mut().ok_or(PlayError::NotReady)?;
                    target.stale = None;
                    target.settings = target
                        .transition
                        .settings(self.settings.config().crossfade());
                    let auto = target.auto;
                    let duration = if self.settings.config().gapless() {
                        0.0
                    } else {
                        target.settings.duration
                    };
                    if auto {
                        let bound = Bound::AtOrBefore(
                            self.current_end().ok_or(PlayError::Untimed)?
                                - self.fade_frames(duration)?,
                        );
                        self.target.as_mut().ok_or(PlayError::NotReady)?.bound = bound;
                    }
                    self.active
                        .get_mut(index)
                        .ok_or(PlayError::NoActiveSlot)?
                        .role = Role::Incoming { batch: None };
                    if let Some(sent) = self.transition_loaded(out)? {
                        self.retarget_answers(seq, sent);
                    }
                }
                Settled::Applied { at, .. } => self.transition_applied(seq, *at)?,
                _ => {
                    self.cancel_target(out).map_err(play_error)?;
                }
            }
            return Ok(());
        }
        match settled {
            Settled::Applied { at, .. } => self.transition_applied(seq, *at),
            Settled::Rejected { .. }
                if self
                    .active
                    .get(index)
                    .is_some_and(|active| active.role == (Role::Incoming { batch: Some(seq) })) =>
            {
                self.cancel_target(out).map_err(play_error)
            }
            _ => Ok(()),
        }
    }

    /// Step 6: outgoing tracks keep their original tails until RT stops them.
    pub(super) fn release_tails(&mut self, out: &mut Outbox<'_, S>) -> Result<(), PlayError> {
        let ended = self.active.indices(|active| {
            active.role == Role::Outgoing
                && !matches!(
                    active.track.snapshot().as_ref().status,
                    PlayingStatus::Playing { .. }
                )
        });
        for index in ended {
            self.release_track(index, out)?;
        }
        Ok(())
    }

    pub(super) fn cancel_target(&mut self, out: &mut Outbox<'_, S>) -> Result<(), QueueError> {
        let Some(target) = self.target else {
            return Ok(());
        };
        let seq = if let Some(index) = self.incoming_index(target.to) {
            let seq = self.active.get(index).and_then(|active| match active.role {
                Role::Incoming { batch } => batch.or(active.load),
                _ => None,
            });
            self.release_track(index, out)?;
            seq
        } else {
            None
        };
        self.tracks.set_status(target.to, TrackStatus::Cancelled);
        self.target = None;
        self.reap_released();
        if let Some(seq) = seq {
            self.finish_answers(seq, Err(PlayError::NotReady));
        }
        Ok(())
    }

    pub(super) fn cancel_auto(&mut self, out: &mut Outbox<'_, S>) -> Result<(), QueueError> {
        if self.target.is_some_and(|target| target.auto) {
            self.cancel_target(out)?;
        }
        for index in self.active.indices(|active| active.role == Role::Preloaded) {
            let id = self.active.get(index).ok_or(PlayError::NoActiveSlot)?.item;
            self.release_track(index, out)?;
            self.tracks.set_status(id, TrackStatus::Cancelled);
        }
        self.reap_released();
        Ok(())
    }

    pub(super) fn withdraw_auto(&mut self, out: &mut Outbox<'_, S>) -> Result<(), QueueError> {
        if self.target.is_some_and(|target| target.auto) {
            self.withdraw_transition(out)?;
        }
        Ok(())
    }

    /// A Stop supersedes B's slot basis; only Stale allows the new entry.
    pub(super) fn withdraw_transition(
        &mut self,
        out: &mut Outbox<'_, S>,
    ) -> Result<(), QueueError> {
        let Some(target) = self.target.filter(|target| target.stale.is_none()) else {
            return Ok(());
        };
        let Some(index) = self.incoming_index(target.to) else {
            return Ok(());
        };
        if let Some(batch) = self.active.get(index).and_then(|active| match active.role {
            Role::Incoming { batch } => batch,
            _ => None,
        }) {
            self.active
                .get_mut(index)
                .ok_or(PlayError::NoActiveSlot)?
                .track
                .apply(TrackCommand::Pause { at: When::Next }, out)?;
            self.target.as_mut().ok_or(PlayError::NotReady)?.stale = Some(batch);
        } else {
            let settings = target
                .transition
                .settings(self.settings.config().crossfade());
            self.target.as_mut().ok_or(PlayError::NotReady)?.settings = settings;
            if target.auto {
                let duration = if self.settings.config().gapless() {
                    0.0
                } else {
                    settings.duration
                };
                let bound = Bound::AtOrBefore(
                    self.current_end().ok_or(PlayError::Untimed)? - self.fade_frames(duration)?,
                );
                self.target.as_mut().ok_or(PlayError::NotReady)?.bound = bound;
            }
        }
        Ok(())
    }

    pub(super) fn tick_deadlines(
        &mut self,
        now: SessionFrame,
        out: &mut Outbox<'_, S>,
    ) -> Result<(), QueueError> {
        if self.shutdown.is_cancelled() {
            return Ok(());
        }
        self.release_tails(out)?;
        self.reap_released();
        if self.config.action_at_item_end != ActionAtItemEnd::Advance {
            return self.cancel_auto(out);
        }
        let Some(end) = self.current_end() else {
            return Ok(());
        };
        if let Some(target) = self.target {
            let duration = if self.settings.config().gapless() {
                0.0
            } else {
                target.settings.duration
            };
            let bound = Bound::AtOrBefore(end - self.fade_frames(duration)?);
            if target.auto && target.bound != bound {
                self.withdraw_auto(out)?;
            }
            return Ok(());
        }
        let ids = self.track_ids();
        let wrap = self.navigation.repeat_mode() == RepeatMode::All;
        let Some(id) = self.navigation.next(&ids, true, wrap) else {
            return Ok(());
        };
        if now >= end - self.frames(self.config.preload_lead)? {
            self.load_track(id, Role::Preloaded, out)?;
        }
        let duration = if self.settings.config().gapless() {
            FrameCount::new(0)
        } else {
            self.fade_frames(self.settings.config().crossfade().duration)?
        };
        let ready = self.active.iter().any(|active| {
            active.item == id && active.role == Role::Preloaded && active.load.is_none()
        });
        if ready || self.earliest()? >= end - duration {
            self.request_transition(
                id,
                Transition::Crossfade,
                AdvanceReason::NaturalEof,
                true,
                out,
            )?;
        }
        Ok(())
    }

    fn incoming_index(&self, id: TrackId) -> Option<usize> {
        self.active
            .position(|active| active.item == id && matches!(active.role, Role::Incoming { .. }))
    }

    fn current_end(&self) -> Option<SessionFrame> {
        let track = self.current_track()?.snapshot();
        let track = track.as_ref();
        if let PlayingStatus::Ended { at } = track.status {
            return Some(at);
        }
        if !matches!(track.status, PlayingStatus::Playing { .. }) {
            return None;
        }
        let slot = self.deck.slots.get(usize::from(track.slot.get()))?;
        if slot.rate <= 0.0 {
            return None;
        }
        if slot.duration <= 0.0 && track.duration.is_none() {
            return None;
        }
        todo!(
            "Project A_end from the current track's frontier and lane speed onto the session clock, including pending speed/tempo changes; a slot position/rate snapshot alone has no frontier-to-frame mapping (spec 4.4, 8)"
        )
    }

    fn frames(&self, duration: Duration) -> Result<FrameCount, PlayError> {
        let sample_rate = NonZeroU32::new(self.deck.sample_rate).ok_or(PlayError::Untimed)?;
        let spec = AudioSpec::new(1, sample_rate);
        spec.frames_for(duration)
            .map_err(|error| PlayError::Internal(error.to_string()))
    }

    fn fade_frames(&self, duration: f32) -> Result<FrameCount, PlayError> {
        let time =
            Duration::try_from_secs_f32(duration).map_err(|_| PlayError::InvalidParameter {
                name: "crossfade duration".into(),
                value: duration,
            })?;
        self.frames(time)
    }
}
