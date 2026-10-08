use kithara_command::{Due, LevelInbox, Seq, Target};
use kithara_config::LiveConfig;
use kithara_signal::{SegmentId, SessionFrame};

use super::{
    processor::Deck,
    track::{PlayerResource, PlayerTrack},
};
use crate::bridge::{
    DeckEqChange, DeckPart, DeckProtocol, DeckRefusal, Fade, Returned, Slot, SlotMark,
};

#[derive(Clone, Copy)]
pub(super) enum EndAction {
    Chain { to: Slot },
    Adopt { segment: SegmentId },
}

#[derive(Clone, Copy)]
pub(super) struct Armed {
    pub(super) seq: Seq,
    pub(super) action: EndAction,
}

impl Deck {
    pub(super) fn arrivals(&mut self, level: &mut LevelInbox<'_, DeckProtocol>) {
        self.release_armed(level);
        while let Some(deferred) = level.next_deferred() {
            let (from, action) = match deferred.commands() {
                [DeckPart::Chain { from, to }] => (*from, EndAction::Chain { to: *to }),
                [DeckPart::Adopt { slot, segment }] => {
                    (*slot, EndAction::Adopt { segment: *segment })
                }
                _ => {
                    deferred.refuse(DeckRefusal::Deferral);
                    continue;
                }
            };
            if let Err(reason) = self.validate(deferred.commands()) {
                deferred.refuse(reason);
                continue;
            }
            if self.armed[from.index()].is_some() {
                deferred.refuse(DeckRefusal::Occupied { slot: from });
                continue;
            }
            if let Some(seq) = deferred.park() {
                self.armed[from.index()] = Some(Armed { seq, action });
            }
        }
    }

    pub(super) fn take_due(&mut self, mut due: Due<'_, DeckProtocol>, context_valid: bool) {
        if due
            .commands()
            .iter()
            .any(|part| matches!(part, DeckPart::Chain { .. }))
        {
            due.refuse(DeckRefusal::Deferral);
            return;
        }
        if let Err(reason) = self.validate(due.commands()) {
            due.refuse(reason);
            return;
        }
        let at = due.at();
        let seq = due.seq();
        let length = due.commands().len();
        for remaining in (0..length).rev() {
            let part = due.commands_mut().remove(0);
            if let Some(slot) = shifted_slot(&part) {
                self.interrupt_stop(&mut due, slot, remaining);
            }
            if let Some(returned) = self.apply(part, at, seq, context_valid) {
                due.commands_mut().push(returned);
            }
        }
        if due
            .commands()
            .iter()
            .any(|part| matches!(part, DeckPart::Stop { .. }))
        {
            due.defer();
        } else {
            due.apply(());
        }
    }

    pub(super) fn resolve_armed(
        &mut self,
        level: &mut LevelInbox<'_, DeckProtocol>,
        start: SessionFrame,
        at: SessionFrame,
    ) {
        self.release_armed(level);
        for index in 0..self.armed.len() {
            let Some(armed) = self.armed[index] else {
                continue;
            };
            let from = Slot::new(u16::try_from(index).unwrap_or(u16::MAX));
            let refusal = if self.tracks.at(from).is_none() {
                Some(DeckRefusal::Empty { slot: from })
            } else {
                match armed.action {
                    EndAction::Chain { to } if self.tracks.at(to).is_none() => {
                        Some(DeckRefusal::Empty { slot: to })
                    }
                    EndAction::Adopt { segment }
                        if self
                            .tracks
                            .at(from)
                            .is_some_and(|track| segment <= track.segment()) =>
                    {
                        Some(DeckRefusal::Outdated { slot: from })
                    }
                    _ => None,
                }
            };
            if let Some(refusal) = refusal {
                self.armed[index] = None;
                if let Some(due) = level.resume(armed.seq, start, at) {
                    due.refuse(refusal);
                }
            }
        }
    }

    pub(super) fn fire_ended(
        &mut self,
        level: &mut LevelInbox<'_, DeckProtocol>,
        start: SessionFrame,
        at: SessionFrame,
        context_valid: bool,
    ) {
        loop {
            let next =
                self.armed
                    .iter()
                    .enumerate()
                    .filter_map(|(index, armed)| {
                        let armed = (*armed)?;
                        let from = Slot::new(u16::try_from(index).ok()?);
                        (self.ended[index]
                            || self.tracks.at(from).is_some_and(|track| {
                                track.state() == crate::bridge::SlotState::Ended
                            }))
                        .then_some((index, armed))
                    })
                    .min_by_key(|(_, armed)| armed.seq);
            let Some((index, armed)) = next else { break };
            self.armed[index] = None;
            let Some(mut due) = level.resume(armed.seq, start, at) else {
                continue;
            };
            if let Err(refusal) = self.validate(due.commands()) {
                due.refuse(refusal);
                continue;
            }
            let seq = due.seq();
            let part = due.commands_mut().remove(0);
            match &part {
                DeckPart::Chain { from, to } => {
                    self.interrupt_stop(&mut due, *from, 0);
                    if to != from {
                        self.interrupt_stop(&mut due, *to, 0);
                    }
                }
                _ => {
                    if let Some(slot) = shifted_slot(&part) {
                        self.interrupt_stop(&mut due, slot, 0);
                    }
                }
            }
            if let Some(returned) = self.apply(part, at, seq, context_valid) {
                due.commands_mut().push(returned);
            }
            due.apply(());
            self.resolve_armed(level, start, at);
        }
    }

    fn release_armed(&mut self, level: &LevelInbox<'_, DeckProtocol>) {
        for entry in &mut self.armed {
            if entry.is_some_and(|armed| !level.is_parked(armed.seq)) {
                *entry = None;
            }
        }
    }

    fn validate(&mut self, commands: &[DeckPart]) -> Result<(), DeckRefusal> {
        for (entry, slot) in self.held.iter_mut().zip(self.tracks.slots()) {
            *entry = self.tracks.at(slot).map(PlayerTrack::segment);
        }
        for part in commands {
            match part {
                DeckPart::Attach { slot, segment, .. } => {
                    let Some(entry) = self.held.get_mut(slot.index()) else {
                        return Err(DeckRefusal::Empty { slot: *slot });
                    };
                    if entry.is_some() {
                        return Err(DeckRefusal::Occupied { slot: *slot });
                    }
                    *entry = Some(*segment);
                }
                DeckPart::Detach { slot } => {
                    let Some(entry) = self
                        .held
                        .get_mut(slot.index())
                        .filter(|entry| entry.is_some())
                    else {
                        return Err(DeckRefusal::Empty { slot: *slot });
                    };
                    *entry = None;
                }
                DeckPart::Adopt { slot, segment } => {
                    let Some(entry) = self
                        .held
                        .get_mut(slot.index())
                        .filter(|entry| entry.is_some())
                    else {
                        return Err(DeckRefusal::Empty { slot: *slot });
                    };
                    if entry.is_some_and(|current| *segment <= current) {
                        return Err(DeckRefusal::Outdated { slot: *slot });
                    }
                    *entry = Some(*segment);
                }
                DeckPart::Replace { slot, segment, .. } => {
                    let Some(entry) = self
                        .held
                        .get_mut(slot.index())
                        .filter(|entry| entry.is_some())
                    else {
                        return Err(DeckRefusal::Empty { slot: *slot });
                    };
                    *entry = Some(*segment);
                }
                DeckPart::Start { slot, .. }
                | DeckPart::Stop { slot, .. }
                | DeckPart::Fade { slot, .. } => {
                    if !self.held.get(slot.index()).is_some_and(Option::is_some) {
                        return Err(DeckRefusal::Empty { slot: *slot });
                    }
                }
                DeckPart::Chain { from, to } => {
                    for slot in [*from, *to] {
                        if !self.held.get(slot.index()).is_some_and(Option::is_some) {
                            return Err(DeckRefusal::Empty { slot });
                        }
                    }
                }
                DeckPart::Mix(_) | DeckPart::Eq(_) | DeckPart::Returned(_) => {}
            }
        }
        Ok(())
    }

    fn apply(
        &mut self,
        part: DeckPart,
        at: SessionFrame,
        seq: Seq,
        context_valid: bool,
    ) -> Option<DeckPart> {
        if let Some(slot) = shifted_slot(&part) {
            self.ended[slot.index()] = false;
        }
        match part {
            DeckPart::Attach { slot, pcm, segment } => {
                let track = self.track(pcm, segment);
                self.tracks
                    .put(slot, track)
                    .err()
                    .map(|track| returned_pcm(slot, track.into_resource()))
            }
            DeckPart::Detach { slot } => {
                let mut track = self.tracks.take(slot)?;
                if context_valid {
                    self.tails[slot.index()].fill(
                        &mut track,
                        self.declick_frames,
                        &self.metrics,
                        &mut self.recycle[slot.index()],
                    );
                }
                Some(returned_pcm(slot, track.into_resource()))
            }
            DeckPart::Start { slot, fade } => {
                if let Some(track) = self.tracks.at_mut(slot) {
                    track.start(fade);
                }
                Some(DeckPart::Start { slot, fade })
            }
            DeckPart::Stop { slot, fade } => {
                let track = self.tracks.at_mut(slot)?;
                track.stop(fade, at);
                if let Some(resume) = track.stop_resume() {
                    track.clear_stop();
                    Some(DeckPart::Returned(Returned::Stopped { slot, resume }))
                } else {
                    self.stops[slot.index()] = Some(seq);
                    Some(DeckPart::Stop { slot, fade })
                }
            }
            DeckPart::Adopt { slot, segment } => {
                if let Some(track) = self.tracks.at_mut(slot) {
                    if context_valid {
                        self.tails[slot.index()].fill(
                            track,
                            self.declick_frames,
                            &self.metrics,
                            &mut self.recycle[slot.index()],
                        );
                    }
                    track.adopt(segment);
                    track.recycle_obsolete(&mut self.recycle[slot.index()]);
                }
                Some(DeckPart::Adopt { slot, segment })
            }
            DeckPart::Fade {
                slot,
                settings,
                dir,
            } => {
                if let Some(track) = self.tracks.at_mut(slot) {
                    track.fade(settings, dir);
                }
                Some(DeckPart::Fade {
                    slot,
                    settings,
                    dir,
                })
            }
            DeckPart::Replace { slot, pcm, segment } => {
                let mut track = self.track(pcm, segment);
                track.start(Fade::Declick);
                track.snap_gate();
                match self.tracks.replace(slot, track) {
                    Ok(mut old) => {
                        if context_valid {
                            self.tails[slot.index()].fill(
                                &mut old,
                                self.evict_frames,
                                &self.metrics,
                                &mut self.recycle[slot.index()],
                            );
                        }
                        Some(returned_pcm(slot, old.into_resource()))
                    }
                    Err(track) => Some(returned_pcm(slot, track.into_resource())),
                }
            }
            DeckPart::Chain { from, to } => {
                self.ended[to.index()] = false;
                if let Some(track) = self.tracks.at_mut(to) {
                    track.start(Fade::Declick);
                    track.snap_gate();
                }
                Some(DeckPart::Chain { from, to })
            }
            DeckPart::Mix(change) => {
                self.mix.apply_change(change);
                self.render.set_gain(self.mix.gain());
                Some(DeckPart::Mix(change))
            }
            DeckPart::Eq(DeckEqChange::Gain { band, gain }) => {
                self.render.set_eq_gain(band, gain);
                Some(DeckPart::Eq(DeckEqChange::Gain { band, gain }))
            }
            DeckPart::Eq(DeckEqChange::Layout(layout)) => self
                .render
                .take_eq_layout(layout)
                .map(|layout| DeckPart::Returned(Returned::Eq(layout))),
            DeckPart::Returned(returned) => Some(DeckPart::Returned(returned)),
        }
    }

    fn track(&self, pcm: Box<PlayerResource>, segment: SegmentId) -> PlayerTrack {
        PlayerTrack::builder()
            .sample_rate(self.sample_rate)
            .declick(self.declick)
            .segment(segment)
            .build(pcm)
    }

    fn interrupt_stop(&mut self, due: &mut Due<'_, DeckProtocol>, slot: Slot, remaining: usize) {
        let Some(seq) = self.stops[slot.index()].take() else {
            return;
        };
        let resume = self
            .tracks
            .at_mut(slot)
            .and_then(PlayerTrack::interrupt_stop);
        let Some(resume) = resume else { return };
        if seq == due.seq() {
            replace_stop(&mut due.commands_mut()[remaining..], slot, resume);
        }
    }

    pub(super) fn finish_stops(
        &mut self,
        level: &mut LevelInbox<'_, DeckProtocol>,
        start: SessionFrame,
        at: SessionFrame,
    ) {
        for index in 0..self.stops.len() {
            let Some(seq) = self.stops[index] else {
                continue;
            };
            if !level.is_parked(seq) {
                self.clear_stops(seq);
                continue;
            }
            let complete = self
                .stops
                .iter()
                .enumerate()
                .filter(|(_, pending)| **pending == Some(seq))
                .all(|(index, _)| {
                    let slot = Slot::new(u16::try_from(index).unwrap_or(u16::MAX));
                    self.tracks
                        .at(slot)
                        .and_then(PlayerTrack::stop_resume)
                        .is_some()
                });
            if !complete {
                continue;
            }
            if let Some(mut due) = level.resume(seq, start, at) {
                for (index, pending) in self.stops.iter().enumerate() {
                    if *pending != Some(seq) {
                        continue;
                    }
                    let slot = Slot::new(u16::try_from(index).unwrap_or(u16::MAX));
                    if let Some(resume) = self.tracks.at(slot).and_then(PlayerTrack::stop_resume) {
                        replace_stop(due.commands_mut(), slot, resume);
                    }
                }
                if stops_complete(due.commands()) {
                    due.apply(());
                } else {
                    due.defer();
                }
            }
            self.clear_stops(seq);
        }
    }

    fn clear_stops(&mut self, seq: Seq) {
        for (index, pending) in self.stops.iter_mut().enumerate() {
            if *pending != Some(seq) {
                continue;
            }
            *pending = None;
            let slot = Slot::new(u16::try_from(index).unwrap_or(u16::MAX));
            if let Some(track) = self.tracks.at_mut(slot) {
                track.clear_stop();
            }
        }
    }
}

fn shifted_slot(part: &DeckPart) -> Option<Slot> {
    match part {
        DeckPart::Attach { slot, .. }
        | DeckPart::Detach { slot }
        | DeckPart::Start { slot, .. }
        | DeckPart::Stop { slot, .. }
        | DeckPart::Adopt { slot, .. }
        | DeckPart::Fade { slot, .. }
        | DeckPart::Replace { slot, .. } => Some(*slot),
        DeckPart::Chain { .. } | DeckPart::Mix(_) | DeckPart::Eq(_) | DeckPart::Returned(_) => None,
    }
}

fn replace_stop(commands: &mut [DeckPart], slot: Slot, resume: SlotMark) {
    if let Some(part) = commands
        .iter_mut()
        .rev()
        .find(|part| matches!(part, DeckPart::Stop { slot: target, .. } if *target == slot))
    {
        *part = DeckPart::Returned(Returned::Stopped { slot, resume });
    }
}

fn stops_complete(commands: &[DeckPart]) -> bool {
    !commands
        .iter()
        .any(|part| matches!(part, DeckPart::Stop { .. }))
}

fn returned_pcm(slot: Slot, pcm: Box<PlayerResource>) -> DeckPart {
    DeckPart::Returned(Returned::Pcm { slot, pcm })
}
