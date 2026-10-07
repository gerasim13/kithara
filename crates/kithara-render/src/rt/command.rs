use kithara_command::{Due, Inbox, Seq, Target};
use kithara_config::LiveConfig;
use kithara_signal::SessionFrame;

use super::{processor::Deck, track::PlayerTrack};
use crate::bridge::{
    DeckApplied, DeckEqChange, DeckPart, DeckProtocol, DeckRefusal, Fade, Released, Slot,
    SlotState,
};

impl Deck {
    /// Answer a batch due now: refused as a whole when a part names a slot it cannot apply to,
    /// parked until its slot ends when it chains a playing track, applied otherwise.
    pub(super) fn take_due(&mut self, mut due: Due<'_, DeckProtocol>) {
        if let Err(refusal) = self.validate(due.commands()) {
            due.refuse(refusal);
            return;
        }
        let chain = due.commands().iter().find_map(|part| match part {
            DeckPart::Chain { from, to } => Some((*from, *to)),
            _ => None,
        });
        if let Some((from, to)) = chain
            && self
                .tracks
                .at(from)
                .is_some_and(|track| track.state() != SlotState::Ended)
        {
            let seq = due.defer();
            if let Some(entry) = self.chains.get_mut(from.index()) {
                *entry = Some((to, seq));
            }
            return;
        }
        self.apply_due(due);
    }

    /// Answer the batch chained behind `from`, which ended on `at`: its basis is judged again
    /// and its parts apply there.
    pub(super) fn fire_chain(
        &mut self,
        inbox: &mut Inbox<DeckProtocol>,
        from: Slot,
        at: SessionFrame,
    ) -> Option<Slot> {
        let (to, seq) = self.chains.get_mut(from.index())?.take()?;
        let due = inbox.resume(seq, at)?;
        let started = match self.validate(due.commands()) {
            Ok(()) => {
                self.apply_due(due);
                true
            }
            Err(refusal) => {
                due.refuse(refusal);
                false
            }
        };
        self.resolve_orphans(inbox, at);
        started.then_some(to)
    }

    /// Answer every chained batch a detach left without its slot.
    pub(super) fn resolve_orphans(&mut self, inbox: &mut Inbox<DeckProtocol>, at: SessionFrame) {
        while let Some((seq, slot)) = self.orphans.pop() {
            if let Some(due) = inbox.resume(seq, at) {
                due.refuse(DeckRefusal::Empty { slot });
            }
        }
    }

    /// Apply every part of `due` in order; each comes back in the receipt as what it left: itself
    /// when it carried no resource, the resource it let go of, or nothing. One part never leaves
    /// more than one, so the batch never grows on the audio thread.
    fn apply_due(&mut self, mut due: Due<'_, DeckProtocol>) {
        let mut applied = DeckApplied::default();
        let commands = due.commands_mut();
        for _ in 0..commands.len() {
            let part = commands.remove(0);
            if let Some(left) = self.apply(part, &mut applied) {
                commands.push(left);
            }
        }
        due.apply(applied);
    }

    /// Check every part against the slots as the parts before it in the batch leave them.
    fn validate(&mut self, parts: &[DeckPart]) -> Result<(), DeckRefusal> {
        self.held.clear();
        self.held
            .extend(self.tracks.slots().map(|slot| self.tracks.is_held(slot)));
        let held = |held: &[bool], slot: Slot| held.get(slot.index()).copied().unwrap_or(false);
        for part in parts {
            match part {
                DeckPart::Attach { slot, .. } => {
                    if held(&self.held, *slot) {
                        return Err(DeckRefusal::Occupied { slot: *slot });
                    }
                    let Some(entry) = self.held.get_mut(slot.index()) else {
                        return Err(DeckRefusal::Empty { slot: *slot });
                    };
                    *entry = true;
                }
                DeckPart::Detach { slot } => {
                    if !held(&self.held, *slot) {
                        return Err(DeckRefusal::Empty { slot: *slot });
                    }
                    if let Some(entry) = self.held.get_mut(slot.index()) {
                        *entry = false;
                    }
                }
                DeckPart::Start { slot, .. }
                | DeckPart::Stop { slot, .. }
                | DeckPart::Fade { slot, .. }
                | DeckPart::Seek { slot, .. }
                | DeckPart::Rate { slot, .. }
                | DeckPart::Replace { slot, .. } => {
                    if !held(&self.held, *slot) {
                        return Err(DeckRefusal::Empty { slot: *slot });
                    }
                }
                DeckPart::Chain { from, to } => {
                    for slot in [*from, *to] {
                        if !held(&self.held, slot) {
                            return Err(DeckRefusal::Empty { slot });
                        }
                    }
                    if self
                        .chains
                        .get(from.index())
                        .is_some_and(Option::is_some)
                    {
                        return Err(DeckRefusal::Occupied { slot: *from });
                    }
                }
                DeckPart::Mix(_) | DeckPart::Eq(_) | DeckPart::Released(_) => {}
            }
        }
        Ok(())
    }

    /// Apply one part of a due batch, noting in `applied` what the batch reports; answers what
    /// the part leaves in the receipt.
    fn apply(&mut self, part: DeckPart, applied: &mut DeckApplied) -> Option<DeckPart> {
        match part {
            DeckPart::Attach { slot, pcm } => {
                let track = self.track(pcm);
                self.tracks
                    .put(slot, track)
                    .map(|held| released(slot, held.into_resource()))
            }
            DeckPart::Detach { slot } => {
                for (from, chain) in self.chains.iter_mut().enumerate() {
                    if let Some((to, seq)) = *chain
                        && (from == slot.index() || to == slot)
                    {
                        *chain = None;
                        self.orphans.push((seq, slot));
                    }
                }
                self.tracks
                    .take(slot)
                    .map(|track| released(slot, track.into_resource()))
            }
            DeckPart::Start { slot, fade } => {
                if let Some(track) = self.tracks.at_mut(slot) {
                    track.start(fade);
                }
                Some(DeckPart::Start { slot, fade })
            }
            DeckPart::Stop { slot, fade } => {
                if let Some(track) = self.tracks.at_mut(slot) {
                    applied.stopped_at = Some(track.position());
                    track.stop(fade);
                }
                Some(DeckPart::Stop { slot, fade })
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
            DeckPart::Chain { from, to } => {
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
                .map(|old| DeckPart::Released(Released::Eq(old))),
            DeckPart::Seek {
                slot,
                seconds,
                seek_epoch,
            } => {
                if let Some(track) = self.tracks.at_mut(slot) {
                    track.seek(seconds);
                }
                Some(DeckPart::Seek {
                    slot,
                    seconds,
                    seek_epoch,
                })
            }
            DeckPart::Rate { slot, rate } => {
                if let Some(track) = self.tracks.at_mut(slot) {
                    track.set_playback_rate(rate);
                }
                Some(DeckPart::Rate { slot, rate })
            }
            DeckPart::Replace { slot, pcm } => self
                .replace(slot, pcm)
                .map(|old| released(slot, old)),
            DeckPart::Released(released) => Some(DeckPart::Released(released)),
        }
    }

    /// Swap `slot`'s consumer for `pcm` on this frame: the old one's next frames go to the
    /// slot's tail ramped down to silence, the new one takes over its transport state.
    fn replace(
        &mut self,
        slot: Slot,
        pcm: Box<crate::rt::track::PlayerResource>,
    ) -> Option<Box<crate::rt::track::PlayerResource>> {
        let mut track = self.track(pcm);
        let Some(mut old) = self.tracks.take(slot) else {
            self.tracks.put(slot, track);
            return None;
        };
        track.set_playback_rate(old.playback_rate());
        if old.state() == SlotState::Playing {
            track.start(Fade::Declick);
            track.snap_gate();
            if let Some(tail) = self.tails.get_mut(slot.index()).and_then(Option::as_mut) {
                tail.fill(&mut old, &self.metrics);
            }
        }
        self.tracks.put(slot, track);
        Some(old.into_resource())
    }

    fn track(&self, pcm: Box<crate::rt::track::PlayerResource>) -> PlayerTrack {
        pcm.set_host_sample_rate(self.sample_rate);
        PlayerTrack::builder()
            .sample_rate(self.sample_rate)
            .declick(self.declick)
            .build(pcm)
    }
}

/// A chained batch whose number a detach orphaned, with the slot that left.
pub(super) type Orphan = (Seq, Slot);

/// The receipt part for the consumer `slot` let go of.
fn released(slot: Slot, pcm: Box<crate::rt::track::PlayerResource>) -> DeckPart {
    DeckPart::Released(Released::Pcm { slot, pcm })
}
