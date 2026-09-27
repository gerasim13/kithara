use kithara_bufpool::HasPool;
use kithara_play::{
    PlayError,
    player::{ResidentLoadObservation, ResidentRender, ResidentStaging},
};
use kithara_signal::SessionFrame;
use kithara_sync::{
    AlignmentSource, GroupState, ObservedEntry, RootCut, RootPort, SyncGroup, SyncOperationId,
    SyncStatusSnapshot,
};
use kithara_warp::BeatGridId;
use tracing::debug;

use super::{
    dispatch::with_owner_cut, port::OwnerPort, protocol::SessionError, state::SessionState,
    transport,
};
use crate::PlayerMember;

/// Output frames the Host leaves between the audio it has already rendered
/// and a deck entry's first admissible activation.
const ENTRY_LEAD_FRAMES: i64 = 2048;

/// Validates `resident`, observed for `member` of the deck `target`, against
/// the transport the Host processed and the source the member's cell holds
/// now, and places its entry after the next commit boundary and the audio
/// already rendered. An entry the deck starts to sound needs its staging.
pub(super) fn observed_entry<S>(
    cut: &RootCut<'_, PlayerMember>,
    port: &mut OwnerPort<'_, S>,
    target: BeatGridId,
    member: BeatGridId,
    resident: &ResidentLoadObservation,
    stages: bool,
) -> Result<ObservedEntry, PlayError> {
    if stages && resident.staging() != ResidentStaging::Available {
        return Err(PlayError::NotReady);
    }
    let ResidentRender::Snapshot(snapshot) = resident.render() else {
        return Err(PlayError::NotReady);
    };
    let processed = port
        .transport_control
        .as_mut()
        .and_then(|transport| transport.observation().snapshot())
        .ok_or(SessionError::TransportNotProcessed)?;
    let output = snapshot.context().output();
    if output.session_epoch() != processed.session_epoch()
        || output.transport_revision() != Some(processed.revision())
        || output.sample_rate() != processed.session_grid().axis().sample_rate()
    {
        return Err(PlayError::NotReady);
    }
    let current = cut
        .current_source(target, member)
        .ok_or(SessionError::SyncMemberNotRegistered(member))?;
    if resident
        .source()
        .is_none_or(|observed| current != Ok(observed))
    {
        return Err(PlayError::NotReady);
    }
    let (boundary, _) = transport::commit_boundary(port.stream.ctx, port.transport)?;
    let activation = i64::from(boundary.max(output.output_frames().end))
        .checked_add(ENTRY_LEAD_FRAMES)
        .ok_or(SessionError::TransportFrameExhausted)?;
    Ok(ObservedEntry::builder()
        .load(resident.load())
        .transport(processed.revision())
        .source(AlignmentSource::Audible {
            frontier: snapshot.frontier(),
            speed: resident.requested_speed(),
        })
        .activation(SessionFrame::new(activation))
        .build())
}

/// A deck decision waiting for its Host, and the Host's fresh observation of
/// the deck's track.
struct Waiting {
    deck: BeatGridId,
    member: BeatGridId,
    operation: SyncOperationId,
    observation: Option<ResidentLoadObservation>,
}

/// Plans once more every deck decision that waits for its Host, without any
/// caller asking: observes each waiting deck's track outside Control, then,
/// in one owner cut, replans the decision that still waits from that
/// observation, or ends it when the track cannot be observed afresh.
///
/// # Errors
/// Returns the first refusal to end a waiting decision; the next tick
/// retries it.
pub(super) fn replan_waiting<T, S>(state: &mut SessionState<T, S>) -> Result<(), SessionError>
where
    S: HasPool<f32> + Send + Sync + 'static,
{
    let root = state.sync.group();
    let waiting: Vec<Waiting> = state
        .sync
        .cells()
        .iter()
        .filter_map(|entry| {
            let operation = replanning(root, entry.group())?;
            Some(Waiting {
                deck: entry.group(),
                member: entry.member(),
                operation,
                observation: root
                    .with_group(entry.group(), PlayerMember::resident_observation)
                    .flatten(),
            })
        })
        .collect();
    if waiting.is_empty() {
        return Ok(());
    }
    with_owner_cut(state, |cut, port| {
        transport::observe_commits(cut, port)?;
        let mut failure = None;
        for decision in waiting {
            if replanning(cut.group(), decision.deck) != Some(decision.operation) {
                continue;
            }
            if let Err(error) = settle(cut, port, decision) {
                failure.get_or_insert(error);
            }
        }
        port.publish(cut.group());
        failure.map_or(Ok(()), Err)
    })
}

/// The operation `deck` holds while it waits to be planned again.
fn replanning(root: &GroupState<PlayerMember>, deck: BeatGridId) -> Option<SyncOperationId> {
    match root.with_group(deck, SyncGroup::status)? {
        SyncStatusSnapshot::Replanning { operation, .. } => Some(operation),
        _ => None,
    }
}

/// Replans one waiting decision from its observation, or ends it.
fn settle<S>(
    cut: &mut RootCut<'_, PlayerMember>,
    port: &mut OwnerPort<'_, S>,
    decision: Waiting,
) -> Result<(), SessionError> {
    let Waiting {
        deck,
        member,
        operation,
        observation,
    } = decision;
    let entry = observation
        .ok_or(PlayError::NotReady)
        .and_then(|resident| observed_entry(cut, port, deck, member, &resident, true));
    let refused = match entry {
        Ok(entry) => match cut.replan(deck, operation, entry) {
            Ok(_) => return Ok(()),
            Err(rejected) => rejected.error().to_string(),
        },
        Err(error) => error.to_string(),
    };
    debug!(?deck, ?operation, %refused, "sync: a waiting deck cannot be planned again");
    cut.abandon_replan(deck, operation)
        .map(drop)
        .map_err(|rejected| SessionError::Sync(rejected.error().clone()))
}
