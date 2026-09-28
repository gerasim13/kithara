use kithara_signal::{SessionFrame, TransportRevision};
use kithara_warp::{BeatGridId, RenderSnapshot};
use tracing::{debug, trace};

use super::{ClockRefusal, EntryPort, RootCut, RootPort, SyncRoot};
use crate::{
    AlignmentSource, ControlError, GroupState, LoadGeneration, ReplanCause, SourceRevision,
    SyncError, SyncGroup, SyncIntent, SyncOperation, SyncOperationId, SyncStatusSnapshot,
    owner::entry_earliest,
};

/// Render evidence for the deck's committed load.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub enum ResidentRender {
    /// The load has no published render context yet.
    Missing,
    /// A reader for this item belongs to another load.
    Stale { bound_load: LoadGeneration },
    /// One exact callback context and presentation frontier.
    Snapshot(RenderSnapshot),
}

/// Whether the Sync executor can stage the committed load.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum ResidentStaging<Id> {
    /// The executor has a staging recipe for this exact load.
    Available,
    /// The executor has no staging recipe to run.
    Unavailable,
    /// The executor can stage only another load of this deck.
    DifferentLoad { item_id: Id, load: LoadGeneration },
}

/// The load the player intends to sound, paired with its own render evidence.
#[derive(Clone, Debug, bon::Builder, fieldwork::Fieldwork)]
#[fieldwork(opt_in, get)]
pub struct ResidentLoadObservation<Id: Copy> {
    /// Identity of the committed player item.
    #[field(get, copy)]
    item_id: Id,
    /// Generation minted when this item entered the player slot.
    #[field(get, copy)]
    load: LoadGeneration,
    /// Manual source speed requested by the Player, even while paused.
    #[field(get, copy)]
    requested_speed: f64,
    /// Render evidence bound to this exact item and load.
    #[field(get)]
    render: ResidentRender,
    /// Host-arbitrated source revision this observation was taken at; `None`
    /// when no Host session arbitrates the player.
    #[field(get, copy)]
    #[builder(required)]
    source: Option<SourceRevision>,
    /// Whether staging still targets this committed load.
    #[field(get, copy)]
    staging: ResidentStaging<Id>,
}

/// Why a deck's entry cannot be placed from its Host's observation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EntryRefusal {
    /// The observation lacks the staging, render evidence, transport or
    /// source the entry needs.
    NotReady,
    /// The Host's audio clock has processed no session transport yet.
    TransportNotProcessed,
    /// No such member is registered in the deck.
    MemberNotRegistered(BeatGridId),
    /// The Host's audio clock places no commit boundary.
    Clock(ClockRefusal),
}

/// A deck decision waiting for its Host, and the Host's fresh observation of
/// the deck's track.
pub struct Waiting<Id: Copy> {
    deck: BeatGridId,
    member: BeatGridId,
    operation: SyncOperationId,
    cause: ReplanCause,
    observation: Option<ResidentLoadObservation<Id>>,
}

/// Where a deck's track enters, as one fresh observation of its audible
/// source places it.
struct ObservedEntry {
    load: LoadGeneration,
    transport: TransportRevision,
    source: AlignmentSource,
    activation: SessionFrame,
}

impl<G: SyncGroup<NestedGroup = G>> SyncRoot<G> {
    /// Every deck decision that waits for its Host, each with the
    /// observation `observe` takes of the deck's track outside Control. A
    /// decision a break left waits here, without entering the owner, while
    /// its resident track does not play by hand yet.
    #[must_use]
    pub fn waiting<Id, F>(&self, observe: F) -> Vec<Waiting<Id>>
    where
        Id: Copy + 'static,
        F: Fn(&G) -> Option<ResidentLoadObservation<Id>>,
    {
        let root = self.group();
        self.cells()
            .iter()
            .filter_map(|entry| {
                let (operation, cause) = replanning(root, entry.group())?;
                let observation = root.with_group(entry.group(), &observe).flatten();
                if cause == ReplanCause::Break
                    && observation
                        .as_ref()
                        .is_some_and(|resident| !plays_by_hand(resident))
                {
                    return None;
                }
                Some(Waiting {
                    deck: entry.group(),
                    member: entry.member(),
                    operation,
                    cause,
                    observation,
                })
            })
            .collect()
    }
}

impl<G: SyncGroup<NestedGroup = G>> RootCut<'_, G> {
    /// Plans once more every decision in `waiting` that still waits for its
    /// Host, from the entry its observation places, or ends it when the
    /// observation places none, then publishes the root once. A decision a
    /// break left keeps waiting while its resident track has not sounded
    /// by hand yet.
    ///
    /// # Errors
    ///
    /// Returns the first refusal to end a waiting decision; the next pass
    /// retries it.
    pub fn replan_waiting<P, Id>(
        &mut self,
        port: &mut P,
        waiting: Vec<Waiting<Id>>,
    ) -> Result<(), SyncError>
    where
        P: RootPort<G> + EntryPort,
        Id: Copy,
    {
        let mut failure = None;
        for decision in waiting {
            if replanning(self.group, decision.deck).map(|(operation, _)| operation)
                != Some(decision.operation)
            {
                continue;
            }
            if let Err(error) = self.settle(port, decision) {
                failure.get_or_insert(error);
            }
        }
        port.publish(self.group);
        failure.map_or(Ok(()), Err)
    }

    /// The operation that syncs `member` of the deck `target` as `intent`
    /// asks, entering where the Host's observation `resident` places it. An
    /// intent that starts the deck sounding needs its staging.
    ///
    /// # Errors
    ///
    /// Returns why `resident` places no entry.
    pub fn requested_sync<P, Id>(
        &self,
        port: &mut P,
        target: BeatGridId,
        member: BeatGridId,
        intent: SyncIntent,
        resident: &ResidentLoadObservation<Id>,
    ) -> Result<SyncOperation<G>, EntryRefusal>
    where
        P: EntryPort,
        Id: Copy,
    {
        let stages = matches!(intent, SyncIntent::Enable | SyncIntent::AlignNow);
        let entry = self.observed_entry(port, target, member, resident, stages)?;
        Ok(SyncOperation::Sync {
            target,
            load: entry.load,
            transport: entry.transport,
            source: entry.source,
            activation: entry.activation,
            intent,
        })
    }

    /// The source revision a decision planned for `member` of the deck
    /// `group` may rely on, or `None` when no such member is registered.
    fn current_source(
        &self,
        group: BeatGridId,
        member: BeatGridId,
    ) -> Option<Result<SourceRevision, ControlError>> {
        self.cells
            .iter()
            .find(|entry| entry.member() == member && entry.group == group)
            .map(|entry| entry.cell.current_source(&self.control))
    }

    /// Validates `resident`, observed for `member` of the deck `target`,
    /// against the transport the Host processed and the source the member's
    /// cell holds now, and places its entry after the next commit boundary
    /// and the audio already rendered.
    fn observed_entry<P: EntryPort, Id: Copy>(
        &self,
        port: &mut P,
        target: BeatGridId,
        member: BeatGridId,
        resident: &ResidentLoadObservation<Id>,
        stages: bool,
    ) -> Result<ObservedEntry, EntryRefusal> {
        if stages && !matches!(resident.staging(), ResidentStaging::Available) {
            return Err(EntryRefusal::NotReady);
        }
        let ResidentRender::Snapshot(snapshot) = resident.render() else {
            return Err(EntryRefusal::NotReady);
        };
        let processed = port
            .processed()
            .ok_or(EntryRefusal::TransportNotProcessed)?;
        let output = snapshot.context().output();
        if output.session_epoch() != processed.session_epoch()
            || output.transport_revision() != Some(processed.revision())
            || output.sample_rate() != processed.sample_rate()
        {
            return Err(EntryRefusal::NotReady);
        }
        let current = self
            .current_source(target, member)
            .ok_or(EntryRefusal::MemberNotRegistered(member))?;
        if resident
            .source()
            .is_none_or(|observed| current != Ok(observed))
        {
            return Err(EntryRefusal::NotReady);
        }
        let boundary = port.commit_boundary().map_err(EntryRefusal::Clock)?;
        let activation = entry_earliest(boundary, output.output_frames().end)
            .ok_or(EntryRefusal::Clock(ClockRefusal::FrameExhausted))?;
        Ok(ObservedEntry {
            load: resident.load(),
            transport: processed.revision(),
            source: AlignmentSource::Audible {
                frontier: snapshot.frontier(),
                speed: resident.requested_speed(),
            },
            activation,
        })
    }

    /// Replans one waiting decision from its observation, or ends it. A
    /// break keeps waiting while its resident's evidence does not match the
    /// transport the Host processed yet.
    fn settle<P: EntryPort, Id: Copy>(
        &mut self,
        port: &mut P,
        decision: Waiting<Id>,
    ) -> Result<(), SyncError> {
        let Waiting {
            deck,
            member,
            operation,
            cause,
            observation,
        } = decision;
        let resident = observation.is_some();
        let entry = observation
            .ok_or(EntryRefusal::NotReady)
            .and_then(|resident| self.observed_entry(port, deck, member, &resident, true));
        let (settling, refused) = match entry {
            Ok(entry) => match self.verified(SyncOperation::Replan {
                operation,
                target: deck,
                load: entry.load,
                transport: entry.transport,
                source: entry.source,
                activation: entry.activation,
            }) {
                Ok(_) => return Ok(()),
                Err(rejected) => (false, rejected.error().to_string()),
            },
            Err(refusal) => (
                matches!(
                    refusal,
                    EntryRefusal::NotReady | EntryRefusal::TransportNotProcessed
                ),
                format!("{refusal:?}"),
            ),
        };
        if cause == ReplanCause::Break && resident && settling {
            trace!(?deck, ?operation, %refused, "sync: a broken deck waits for the transport its Host processed");
            return Ok(());
        }
        debug!(?deck, ?operation, %refused, "sync: a waiting deck cannot be planned again");
        self.verified(SyncOperation::AbandonReplan {
            operation,
            target: deck,
        })
        .map(drop)
        .map_err(|rejected| rejected.error().clone())
    }
}

/// Whether `resident` renders its staged track by hand: its last callback
/// sounded no map.
fn plays_by_hand<Id: Copy>(resident: &ResidentLoadObservation<Id>) -> bool {
    matches!(resident.staging(), ResidentStaging::Available)
        && matches!(
            resident.render(),
            ResidentRender::Snapshot(snapshot) if snapshot.frontier().warp_map().is_none()
        )
}

/// The operation `deck` holds while it waits to be planned again, and why.
fn replanning<G: SyncGroup<NestedGroup = G>>(
    root: &GroupState<G>,
    deck: BeatGridId,
) -> Option<(SyncOperationId, ReplanCause)> {
    match root.with_group(deck, SyncGroup::status)? {
        SyncStatusSnapshot::Replanning {
            operation, cause, ..
        } => Some((operation, cause)),
        _ => None,
    }
}
