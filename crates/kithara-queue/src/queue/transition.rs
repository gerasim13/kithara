use std::num::NonZeroU32;

use kithara_bufpool::HasPool;
use kithara_command::{Rejection, Seq, When};
use kithara_events::TrackId;
use kithara_platform::time::Duration;
use kithara_play::{
    Bound, FadeDir, Outbox, OutputSnapshot, PlayError, Player, Settled, Track, TrackCommand, TrackFactory,
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
        output: Option<&OutputSnapshot>,
        out: &mut Outbox<'_, S>,
    ) -> Result<Option<Seq>, QueueError> {
        let ids = self.track_ids();
        let wrap = self.navigation.repeat_mode() == RepeatMode::All;
        match self.navigation.next(&ids, auto, wrap) {
            Some(id) => self.request_transition(id, transition, reason, auto, output, out),
            None => Ok(None),
        }
    }

    pub(super) fn request_transition(
        &mut self,
        id: TrackId,
        transition: Transition,
        reason: AdvanceReason,
        auto: bool,
        output: Option<&OutputSnapshot>,
        out: &mut Outbox<'_, S>,
    ) -> Result<Option<Seq>, QueueError> {
        let settings = transition
            .settings(self.config.settings.crossfade())
            .validate()
            .map_err(PlayError::from)?;
        let bound = if auto {
            let duration = if self.config.settings.gapless() {
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
            retry: None,
            repeat: None,
            chained: false,
        });
        if !auto {
            self.navigation.select(id, &self.track_ids());
        }
        match self.load_track(id, Role::Incoming { batch: None }, output, out) {
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

    /// Step 2: an attached or prepared replacement can enter; an open in flight waits.
    pub(super) fn transition_loaded(
        &mut self,
        out: &mut Outbox<'_, S>,
    ) -> Result<Option<Seq>, PlayError> {
        let Some(target) = self
            .target
            .filter(|target| target.stale.is_none() && target.repeat.is_none())
        else {
            return Ok(None);
        };
        let Some(index) = self.incoming_index(target.to) else {
            return Ok(None);
        };
        let Some(active) = self.active.get(index) else {
            return Ok(None);
        };
        if !matches!(active.role, Role::Incoming { batch: None })
            || (active.load.is_some() && !self.active.is_replacement(index))
        {
            return Ok(None);
        }
        if !matches!(
            active.track.snapshot().as_ref().status,
            PlayingStatus::Loaded | PlayingStatus::Paused { .. }
        ) {
            return Ok(None);
        }
        if let Some(load) = active.load {
            if !self.finish_load(index)? {
                self.cancel_target(out).map_err(play_error)?;
                return Ok(None);
            }
            if let Some(target) = &mut self.target {
                target.retry = Some(load);
            }
        }
        let Some(frame) = self.transition_entry(index, target.bound)? else {
            return Ok(None);
        };
        let sent = self.send_transition(index, frame, out)?;
        if let Some(sent) = sent
            && let Some(previous) = self.target.and_then(|target| target.retry)
        {
            self.retarget_answers(previous, sent);
            if let Some(target) = &mut self.target {
                target.retry = None;
            }
        }
        Ok(sent)
    }

    fn finish_load(&mut self, index: usize) -> Result<bool, PlayError> {
        let active = self.active.get_mut(index).ok_or(PlayError::NoActiveSlot)?;
        let id = active.item;
        let metadata = active.track.snapshot().as_ref().metadata.clone();
        active.load = None;
        if !self.tracks.loaded(id, &metadata) {
            return Ok(false);
        }
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
        Ok(true)
    }

    /// Step 3: a deadline entry may not precede what this pass can deliver.
    fn transition_entry(
        &self,
        index: usize,
        bound: Bound,
    ) -> Result<Option<SessionFrame>, PlayError> {
        let earliest = self.earliest()?;
        let active = self.active.get(index).ok_or(PlayError::NoActiveSlot)?;
        let Some(frame) = active.track.entry(bound) else {
            return Ok(None);
        };
        Ok(if frame < earliest {
            active.track.entry(Bound::AtOrAfter(earliest))
        } else {
            Some(frame)
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
        if out.deck_available() == 0 {
            return Err(PlayError::Full("deck"));
        }
        let slot = self
            .active
            .get(incoming)
            .ok_or(PlayError::NoActiveSlot)?
            .slot;
        let replacement = self.active.is_replacement(incoming);
        let current = self.active_current_index();
        let outgoing = current.filter(|index| {
            self.active
                .get(*index)
                .is_some_and(|current| current.slot != slot)
        });
        let predecessor = current
            .and_then(|index| self.active.get(index))
            .filter(|active| {
                matches!(
                    active.track.snapshot().as_ref().status,
                    PlayingStatus::Playing { .. }
                )
            })
            .map(|active| active.slot);
        let chain =
            target.auto && self.config.settings.gapless() && predecessor.is_some() && !replacement;
        let at = When::At(frame);
        let sent = if chain {
            Some(out.chain(predecessor.ok_or(PlayError::NoActiveSlot)?, slot)?)
        } else {
            let result = out.together_owned(at, |out| {
                let active = self
                    .active
                    .get_mut(incoming)
                    .ok_or(PlayError::NoActiveSlot)?;
                if replacement {
                    active.track.apply(TrackCommand::Evict { at }, out)?;
                }
                active.track.apply(
                    TrackCommand::Fade {
                        at,
                        settings: target.settings,
                        dir: FadeDir::In,
                    },
                    out,
                )?;
                if let Some(index) = outgoing {
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
                Ok(())
            });
            let track = &mut self
                .active
                .get_mut(incoming)
                .ok_or(PlayError::NoActiveSlot)?
                .track;
            match result {
                Ok(((), Some(seq))) => {
                    track.finish_group(Ok(seq));
                    Some(seq)
                }
                Ok(((), None)) => None,
                Err((error, mut parts)) => {
                    track.finish_group(Err(&mut parts));
                    return Err(error);
                }
            }
        };
        if let Some(batch) = sent {
            self.active
                .get_mut(incoming)
                .ok_or(PlayError::NoActiveSlot)?
                .role = Role::Incoming { batch: Some(batch) };
            if let Some(target) = &mut self.target {
                target.chained = chain;
            }
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
            if !target.chained {
                self.active
                    .fade_out(slot, at + self.fade_frames(target.settings.duration)?);
            }
        }
        self.active
            .get_mut(index)
            .ok_or(PlayError::NoActiveSlot)?
            .role = Role::Current;
        self.active.activate_replacement(index);
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
        if target.settings.duration > 0.0 && !target.chained {
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
        if self.target.is_some_and(|target| target.repeat == Some(seq)) {
            match settled {
                Settled::Applied { .. } => {
                    self.target = None;
                    self.announce(QueueEvent::CurrentTrackAdvance {
                        id: self.current,
                        reason: AdvanceReason::NaturalEof,
                    });
                }
                Settled::Rejected {
                    reason: Rejection::Late | Rejection::Stale,
                    ..
                } => {
                    if let Some(target) = &mut self.target {
                        target.repeat = None;
                        target.retry = Some(seq);
                    }
                }
                _ => self.cancel_target(out).map_err(play_error)?,
            }
            return Ok(());
        }
        if active.load == Some(seq) {
            let id = active.item;
            if applied && self.finish_load(index)? {
                if let Some(sent) = self.transition_loaded(out)? {
                    self.retarget_answers(seq, sent);
                }
            } else if !applied {
                self.active
                    .get_mut(index)
                    .ok_or(PlayError::NoActiveSlot)?
                    .load = None;
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
                    target.settings = target.transition.settings(self.config.settings.crossfade());
                    let auto = target.auto;
                    let duration = if self.config.settings.gapless() {
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
                    if let Some(target) = &mut self.target {
                        target.retry = Some(seq);
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
            Settled::Rejected { reason, .. }
                if self
                    .active
                    .get(index)
                    .is_some_and(|active| active.role == (Role::Incoming { batch: Some(seq) })) =>
            {
                if matches!(reason, Rejection::Late | Rejection::Stale) {
                    if let Some(active) = self.active.get_mut(index) {
                        active.role = Role::Incoming { batch: None };
                    }
                    let earliest = self.earliest()?;
                    if let Some(target) = &mut self.target {
                        target.retry = Some(seq);
                        target.bound = Bound::AtOrAfter(earliest);
                    }
                    Ok(())
                } else {
                    self.cancel_target(out).map_err(play_error)
                }
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
        if target.auto && self.single_slot_repeat(target.to) {
            self.cancel_repeat(out)?;
            self.target = None;
            return Ok(());
        }
        let seq = if let Some(index) = self.incoming_index(target.to) {
            let seq = self.active.get(index).and_then(|active| match active.role {
                Role::Incoming { batch } => batch.or(active.load),
                _ => None,
            });
            self.release_track(index, out)?;
            seq
        } else {
            None
        }
        .or(target.retry);
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
            let settings = target.transition.settings(self.config.settings.crossfade());
            self.target.as_mut().ok_or(PlayError::NotReady)?.settings = settings;
            if target.auto {
                let duration = if self.config.settings.gapless() {
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
        output: Option<&OutputSnapshot>,
        out: &mut Outbox<'_, S>,
    ) -> Result<(), QueueError> {
        if self.shutdown.is_cancelled() {
            return Ok(());
        }
        self.release_tails(out)?;
        self.reap_released();
        self.loaded_tracks(out)?;
        if self.config.action_at_item_end != ActionAtItemEnd::Advance {
            return self.cancel_auto(out);
        }
        let Some(end) = self.current_end() else {
            return Ok(());
        };
        if let Some(target) = self.target {
            if target.auto && self.single_slot_repeat(target.to) {
                if target.repeat.is_none() {
                    self.repeat_one(end, out)?;
                }
                return Ok(());
            }
            let duration = if self.config.settings.gapless() {
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
            if self.single_slot_repeat(id) {
                self.repeat_one(end, out)?;
                return Ok(());
            }
            self.load_track(id, Role::Preloaded, output, out)?;
        }
        let duration = if self.config.settings.gapless() {
            FrameCount::new(0)
        } else {
            self.fade_frames(self.config.settings.crossfade().duration)?
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
                output,
                out,
            )?;
        }
        Ok(())
    }

    fn incoming_index(&self, id: TrackId) -> Option<usize> {
        self.active
            .position(|active| active.item == id && matches!(active.role, Role::Incoming { .. }))
    }

    fn single_slot_repeat(&self, id: TrackId) -> bool {
        self.config.mixer.slots().get() == 1
            && self.navigation.repeat_mode() == RepeatMode::One
            && self.current == Some(id)
    }

    fn repeat_one(&mut self, end: SessionFrame, out: &mut Outbox<'_, S>) -> Result<(), PlayError> {
        if out.deck_available() == 0 {
            return Err(PlayError::Full("deck"));
        }
        let previous = self.target.and_then(|target| target.retry);
        let seq = self.repeat_segment(out)?;
        let to = self.current.ok_or(PlayError::NoActiveSlot)?;
        self.target = Some(Target {
            to,
            bound: Bound::AtOrBefore(end),
            settings: Transition::None.settings(self.config.settings.crossfade()),
            transition: Transition::None,
            reason: AdvanceReason::NaturalEof,
            auto: true,
            stale: None,
            retry: None,
            repeat: Some(seq),
            chained: false,
        });
        if let Some(previous) = previous {
            self.retarget_answers(previous, seq);
        }
        Ok(())
    }

    fn repeat_segment(&mut self, out: &mut Outbox<'_, S>) -> Result<Seq, PlayError> {
        let index = self.active_current_index().ok_or(PlayError::NoActiveSlot)?;
        let active = self.active.get_mut(index).ok_or(PlayError::NoActiveSlot)?;
        if active.track.snapshot().as_ref().lane_room == 0 {
            return Err(PlayError::Full("lane"));
        }
        active
            .track
            .apply(TrackCommand::PlayAfter { track: active.slot }, out)?
            .ok_or_else(|| {
                PlayError::Internal("a repeated segment must name its Adopt batch".into())
            })
    }

    fn cancel_repeat(&mut self, _out: &mut Outbox<'_, S>) -> Result<(), PlayError> {
        todo!(
            "kithara-render::DeckProtocol cancellation of a parked Deferred Adopt, exposed by kithara-play Track without Detach or Stop of its incumbent segment (contract §8.6; ruling D6)"
        )
    }

    fn loaded_tracks(&mut self, out: &mut Outbox<'_, S>) -> Result<(), PlayError> {
        let ready = self.active.indices(|active| {
            active.load.is_some()
                && matches!(
                    active.track.snapshot().as_ref().status,
                    PlayingStatus::Loaded | PlayingStatus::Paused { .. }
                )
        });
        for index in ready {
            let Some(active) = self.active.get_mut(index) else {
                continue;
            };
            let previous = active.load.take();
            let id = active.item;
            let metadata = active.track.snapshot().as_ref().metadata.clone();
            if self.tracks.loaded(id, &metadata)
                && let Some(index) = self
                    .tracks
                    .records()
                    .iter()
                    .position(|record| record.id == id)
            {
                self.announce(QueueEvent::NextTrackReady { id, index });
            }
            if let Some(previous) = previous
                && let Some(sent) = self.transition_loaded(out)?
            {
                self.retarget_answers(previous, sent);
            }
        }
        Ok(())
    }

    fn current_end(&self) -> Option<SessionFrame> {
        let track = self.current_track()?;
        let snapshot = track.snapshot();
        let snapshot = snapshot.as_ref();
        if let PlayingStatus::Ended { at } | PlayingStatus::Failed { at, .. } = snapshot.status {
            return Some(at);
        }
        if !matches!(snapshot.status, PlayingStatus::Playing { .. }) {
            return None;
        }
        snapshot.duration?;
        let sample_rate = NonZeroU32::new(self.deck.sample_rate)?;
        track.planned_end(sample_rate).ok().flatten()
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
