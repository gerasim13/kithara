use std::num::NonZeroU32;

use firewheel::{FirewheelContext, error::UpdateError};
use kithara_signal::SessionFrame;
use kithara_sync::ParentGridUpdate;
use kithara_warp::{BeatGrid, BeatGridState, MapAxis};

use super::{
    commit::{
        SessionGridGeneration, SessionTransportCommit, TransportBoundary, TransportCommitResult,
        TransportCommitStamp, TransportObservation,
    },
    event::TransportEvent,
    process::converge_transport_restart,
};
use crate::{
    api::{SessionBeat, SessionTransportSnapshot, Tempo, TransportRevision},
    session::{SessionError, dispatch::stream_died, state::SessionState},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum AbortDelivery {
    Pending,
    Sent,
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct TransportLedger {
    completed: Option<TransportRevision>,
    last: Option<TransportRevision>,
    rejected: Option<TransportRevision>,
}

/// The revision bookkeeping is the same in every phase, so it lives beside the
/// phase rather than being repeated inside each variant.
#[derive(Debug, Default, fieldwork::Fieldwork)]
#[fieldwork(get, get_mut, vis = "pub(crate)")]
pub(crate) struct SessionTransportState {
    ledger: TransportLedger,
    #[field(skip)]
    phase: TransportPhase,
}

#[derive(Debug, Default)]
enum TransportPhase {
    #[default]
    Unconfigured,
    Stable {
        active: SessionTransportCommit,
    },
    Applying {
        next: SessionTransportCommit,
        previous: Option<SessionTransportCommit>,
    },
    Aborting {
        delivery: AbortDelivery,
        previous: Option<SessionTransportCommit>,
        revision: TransportRevision,
    },
}

impl SessionTransportState {
    /// The commit the caller has already asked for, pending or not.
    pub(crate) const fn accepted(&self) -> Option<SessionTransportCommit> {
        match self.phase {
            TransportPhase::Unconfigured => None,
            TransportPhase::Stable { active } => Some(active),
            TransportPhase::Applying { next, .. } => Some(next),
            TransportPhase::Aborting { previous, .. } => previous,
        }
    }

    /// The commit the graph has actually rendered.
    pub(crate) const fn observed(&self) -> Option<SessionTransportCommit> {
        match self.phase {
            TransportPhase::Unconfigured => None,
            TransportPhase::Stable { active } => Some(active),
            TransportPhase::Applying { previous, .. }
            | TransportPhase::Aborting { previous, .. } => previous,
        }
    }

    pub(crate) fn pending_revision(&self) -> Option<TransportRevision> {
        match self.phase {
            TransportPhase::Applying { next, .. } => Some(next.revision()),
            TransportPhase::Aborting { revision, .. } => Some(revision),
            TransportPhase::Unconfigured | TransportPhase::Stable { .. } => None,
        }
    }
}

pub(crate) fn set_tempo<T, S>(
    state: &mut SessionState<T, S>,
    tempo: Tempo,
) -> Result<(), SessionError> {
    let _ = refresh_observation(state)?;
    let accepted = state.transport.accepted();
    if accepted.is_some_and(|commit| commit.tempo() == tempo) {
        return Ok(());
    }
    ensure_no_pending_commit(state)?;
    let revision = next_revision(state)?;
    let (target_frame, sample_rate) = commit_boundary(state)?;
    let next = SessionTransportCommit::new(
        tempo,
        accepted.is_none_or(|commit| commit.is_playing()),
        revision,
    );
    let stamp =
        TransportCommitStamp::new(state.transport.observed(), next, target_frame, sample_rate);
    schedule_commit(state, next, stamp)
}

pub(crate) fn set_playing<T, S>(
    state: &mut SessionState<T, S>,
    playing: bool,
) -> Result<(), SessionError> {
    let _ = refresh_observation(state)?;
    let accepted = state
        .transport
        .accepted()
        .ok_or(SessionError::TransportNotProcessed)?;
    if accepted.is_playing() == playing {
        return Ok(());
    }
    ensure_no_pending_commit(state)?;
    let revision = next_revision(state)?;
    let (target_frame, sample_rate) = commit_boundary(state)?;
    let next = SessionTransportCommit::new(accepted.tempo(), playing, revision);
    let stamp =
        TransportCommitStamp::new(state.transport.observed(), next, target_frame, sample_rate);
    schedule_commit(state, next, stamp)
}

pub(crate) fn seek<T, S>(
    state: &mut SessionState<T, S>,
    target: SessionBeat,
) -> Result<(), SessionError> {
    let _ = refresh_observation(state)?;
    let accepted = state
        .transport
        .accepted()
        .ok_or(SessionError::TransportNotProcessed)?;
    ensure_no_pending_commit(state)?;
    let revision = next_revision(state)?;
    let (target_frame, sample_rate) = commit_boundary(state)?;
    let next =
        SessionTransportCommit::relocate(accepted.tempo(), accepted.is_playing(), revision, target);
    let stamp =
        TransportCommitStamp::new(state.transport.observed(), next, target_frame, sample_rate);
    schedule_commit(state, next, stamp)
}

pub(crate) fn snapshot<T, S>(
    state: &mut SessionState<T, S>,
) -> Result<SessionTransportSnapshot, SessionError> {
    refresh_observation(state)?
        .snapshot()
        .ok_or(SessionError::TransportNotProcessed)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RouteRestartStatus {
    Pending,
    Ready,
}

/// Zero sample rate remains a valid backend-default request; the grid axis's sample rate is always
/// concrete, so it substitutes for zero rather than propagating it.
pub(crate) fn prepare_route_restart<T, S>(
    state: &mut SessionState<T, S>,
    sample_rate: u32,
) -> Result<RouteRestartStatus, SessionError> {
    let was_running = state
        .ctx
        .as_ref()
        .ok_or(SessionError::NoContext)?
        .is_active();
    let current = state.root.snapshot();
    let MapAxis::Session(axis) = current.axis() else {
        return Err(SessionError::Graph(
            "session host published a non-session grid axis".to_owned(),
        ));
    };
    let target = if let Some(target) = state.reserved_session_grid {
        let target_stamp = target
            .stamp()
            .map_err(|error| SessionError::Graph(error.message().to_owned()))?;
        if was_running
            || !matches!(current.state(), BeatGridState::Unavailable(_))
            || current.stamp() != target_stamp
            || axis.epoch() != target.epoch()
        {
            return Err(SessionError::Graph(
                "reserved route boundary does not match the stopped session".to_owned(),
            ));
        }
        target
    } else {
        let observed = state
            .transport_control
            .as_mut()
            .ok_or_else(|| SessionError::Graph("session transport control is missing".to_owned()))?
            .observation()
            .session_grid();
        if observed.epoch() < axis.epoch() {
            return Err(SessionError::Graph(
                "session transport generation trails the published host grid".to_owned(),
            ));
        }
        let mut target = observed;
        if was_running || observed.epoch() == axis.epoch() {
            target
                .advance_restart()
                .map_err(|error| SessionError::Graph(error.message().to_owned()))?;
        }
        let stamp = target
            .stamp()
            .map_err(|error| SessionError::Graph(error.message().to_owned()))?;
        let sample_rate = NonZeroU32::new(sample_rate).unwrap_or_else(|| axis.sample_rate());
        state
            .root
            .publish_unavailable_grid(stamp, sample_rate, target.epoch())?;
        state.publish_root();
        state.reserved_session_grid = Some(target);
        target
    };

    if was_running {
        state
            .ctx
            .as_mut()
            .ok_or(SessionError::NoContext)?
            .request_deactivate();
        state.stream = None;
    }
    state.publish_root();
    finish_route_restart(state, target)
}

fn finish_route_restart<T, S>(
    state: &mut SessionState<T, S>,
    target: SessionGridGeneration,
) -> Result<RouteRestartStatus, SessionError> {
    let Some(store) = state
        .ctx
        .as_mut()
        .ok_or(SessionError::NoContext)?
        .proc_store_mut()
    else {
        return Ok(RouteRestartStatus::Pending);
    };
    let actual = converge_transport_restart(store, target)
        .map_err(|error| SessionError::Graph(error.message().to_owned()))?;
    let promoted = target
        .promote(actual)
        .map_err(|error| SessionError::Graph(error.message().to_owned()))?;
    if promoted != target {
        let published = state.root.snapshot();
        let MapAxis::Session(published_axis) = published.axis() else {
            return Err(SessionError::Graph(
                "session host published a non-session grid axis".to_owned(),
            ));
        };
        let stamp = promoted
            .stamp()
            .map_err(|error| SessionError::Graph(error.message().to_owned()))?;
        state.root.publish_unavailable_grid(
            stamp,
            published_axis.sample_rate(),
            promoted.epoch(),
        )?;
        state.publish_root();
        state.reserved_session_grid = Some(promoted);
    }
    let observed = state
        .transport_control
        .as_mut()
        .ok_or_else(|| SessionError::Graph("session transport control is missing".to_owned()))?
        .observation()
        .session_grid();
    if observed != promoted {
        return Err(SessionError::Graph(
            "session transport did not converge to the reserved route boundary".to_owned(),
        ));
    }
    Ok(RouteRestartStatus::Ready)
}

fn ensure_no_pending_commit<T, S>(state: &SessionState<T, S>) -> Result<(), SessionError> {
    if state.transport.pending_revision().is_some() {
        return Err(SessionError::TransportNotProcessed);
    }
    Ok(())
}

fn next_revision<T, S>(state: &SessionState<T, S>) -> Result<TransportRevision, SessionError> {
    state
        .transport
        .ledger()
        .last
        .map_or(Ok(TransportRevision::first()), |revision| {
            revision
                .checked_next()
                .ok_or(SessionError::TransportRevisionExhausted)
        })
}

fn schedule_commit<T, S>(
    state: &mut SessionState<T, S>,
    next: SessionTransportCommit,
    stamp: TransportCommitStamp,
) -> Result<(), SessionError> {
    let revision = next.revision();
    queue_stamp(state, stamp)?;
    state.transport.ledger_mut().last = Some(revision);
    if let Err(error) = update_context(state) {
        publish_transport_event(
            state,
            &TransportEvent::Failed {
                revision: Some(u64::from(revision)),
                reason: error.to_string(),
            },
        );
        abort_commit(state, revision)?;
        return Err(error);
    }
    let previous = state.transport.observed();
    state.transport.phase = TransportPhase::Applying { next, previous };
    Ok(())
}

fn commit_boundary<T, S>(
    state: &SessionState<T, S>,
) -> Result<(SessionFrame, NonZeroU32), SessionError> {
    let ctx = state.ctx.as_ref().ok_or(SessionError::NoContext)?;
    let stream_info = ctx.stream_info().ok_or(SessionError::NoContext)?;
    let lead_frames = state
        .transport
        .observed()
        .map_or(0, |_| i64::from(stream_info.max_block_frames.get()));
    let target_frame = ctx
        .audio_clock()
        .samples
        .0
        .checked_add(lead_frames)
        .ok_or(SessionError::TransportFrameExhausted)?;
    Ok((SessionFrame::new(target_frame), stream_info.sample_rate))
}

fn queue_stamp<T, S>(
    state: &mut SessionState<T, S>,
    stamp: TransportCommitStamp,
) -> Result<(), SessionError> {
    let ctx = state.ctx.as_mut().ok_or(SessionError::NoContext)?;
    let control = state
        .transport_control
        .as_ref()
        .ok_or_else(|| SessionError::Graph("session transport control is missing".to_owned()))?;
    control.queue_stamp(ctx, stamp);
    Ok(())
}

/// Marks the stream for restart directly rather than routing through a message, since the session
/// owns stream restarts and a message would strand the transport behind a stream nobody rearms.
fn update_context<T, S>(state: &mut SessionState<T, S>) -> Result<(), SessionError> {
    let Err(error) = state.ctx.as_mut().ok_or(SessionError::NoContext)?.update() else {
        return Ok(());
    };
    if stream_died(state) {
        state.stream_needs_restart = true;
    }
    Err(SessionError::TransportSync(sync_error_reason(error)))
}

fn abort_commit<T, S>(
    state: &mut SessionState<T, S>,
    revision: TransportRevision,
) -> Result<(), SessionError> {
    let ctx = state.ctx.as_mut().ok_or(SessionError::NoContext)?;
    let control = state
        .transport_control
        .as_ref()
        .ok_or_else(|| SessionError::Graph("session transport control is missing".to_owned()))?;
    control.queue_abort(ctx, revision);
    let previous = state.transport.observed();
    state.transport.phase = TransportPhase::Aborting {
        previous,
        revision,
        delivery: AbortDelivery::Pending,
    };
    deliver_abort(state)
}

/// Leaves a pending abort delivery untouched when the stream is stopped, since Firewheel only
/// flushes queued events while running; the next refresh retries an update that would otherwise
/// deliver nothing.
fn deliver_abort<T, S>(state: &mut SessionState<T, S>) -> Result<(), SessionError> {
    update_context(state)?;
    if !state.ctx.as_ref().is_some_and(FirewheelContext::is_active) {
        return Ok(());
    }
    if let TransportPhase::Aborting { delivery, .. } = &mut state.transport.phase
        && *delivery == AbortDelivery::Pending
    {
        *delivery = AbortDelivery::Sent;
    }
    Ok(())
}

fn sync_error_reason(error: UpdateError) -> String {
    match error {
        UpdateError::MsgChannelFull => "message channel is full".to_owned(),
        UpdateError::GraphCompileError(error) => {
            format!("audio graph compilation failed: {error}")
        }
    }
}

fn refresh_observation<T, S>(
    state: &mut SessionState<T, S>,
) -> Result<TransportObservation, SessionError> {
    if state.reserved_session_grid.is_some() {
        return Err(SessionError::TransportNotProcessed);
    }
    let observation = state
        .transport_control
        .as_mut()
        .ok_or_else(|| SessionError::Graph("session transport control is missing".to_owned()))?
        .observation();
    if let Some(snapshot) = observation.snapshot()
        && state.root.snapshot().stamp() != snapshot.session_grid_stamp()
    {
        state.root.publish_session(ParentGridUpdate::new(
            snapshot.session_grid_stamp(),
            snapshot.session_epoch(),
            snapshot.anchor(),
            None,
        ))?;
        state.publish_root();
    }
    if let Some(completion) = observation.completion() {
        apply_completion(state, completion);
    }
    if matches!(
        state.transport.phase,
        TransportPhase::Aborting {
            delivery: AbortDelivery::Pending,
            ..
        }
    ) {
        deliver_abort(state)?;
    }
    if state.transport.ledger_mut().rejected.take().is_some() {
        return Err(SessionError::TransportCommitRejected);
    }
    Ok(observation)
}

/// Treats the graph's reported pending revision as authoritative for whether an abort happened,
/// regardless of whether this side's own delivery bookkeeping had caught up.
fn apply_completion<T, S>(state: &mut SessionState<T, S>, completion: TransportCommitResult) {
    let revision = completion.revision();
    if state
        .transport
        .ledger()
        .completed
        .is_some_and(|completed| revision <= completed)
    {
        return;
    }
    let observed = state.transport.observed();
    let phase = std::mem::take(&mut state.transport.phase);
    let (active, committed, rejected) = match (completion, phase) {
        (TransportCommitResult::Applied(_), TransportPhase::Applying { next, .. })
            if next.revision() == revision =>
        {
            (Some(next), Some(next), false)
        }
        (
            TransportCommitResult::Aborted(_),
            TransportPhase::Aborting {
                previous,
                revision: pending_revision,
                ..
            },
        ) if pending_revision == revision => (previous, None, false),
        _ => (observed, None, true),
    };
    let ledger = state.transport.ledger_mut();
    ledger.completed = Some(revision);
    if rejected {
        ledger.rejected = Some(revision);
    }
    state.transport.phase = active.map_or(TransportPhase::Unconfigured, |active| {
        TransportPhase::Stable { active }
    });
    if let Some(next) = committed {
        publish_transport_commit(state, observed, next);
    } else if rejected {
        publish_transport_event(
            state,
            &TransportEvent::Failed {
                revision: Some(u64::from(revision)),
                reason: "render graph rejected transport commit".to_owned(),
            },
        );
    }
}

fn publish_transport_commit<T, S>(
    state: &SessionState<T, S>,
    previous: Option<SessionTransportCommit>,
    next: SessionTransportCommit,
) {
    for event in transport_events(previous, next).into_iter().flatten() {
        publish_transport_event(state, &event);
    }
}

fn transport_events(
    previous: Option<SessionTransportCommit>,
    next: SessionTransportCommit,
) -> [Option<TransportEvent>; 3] {
    let revision = u64::from(next.revision());
    let tempo = previous
        .is_none_or(|commit| commit.tempo() != next.tempo())
        .then(|| TransportEvent::TempoCommitted {
            revision,
            beats_per_minute: next.tempo().beats_per_minute(),
        });
    let play_state = previous
        .is_none_or(|commit| commit.is_playing() != next.is_playing())
        .then(|| TransportEvent::PlayStateCommitted {
            revision,
            playing: next.is_playing(),
        });
    let seek = match next.boundary() {
        TransportBoundary::Continuous => None,
        TransportBoundary::Relocate(target) => Some(TransportEvent::SeekCommitted {
            revision,
            position_beats: f64::from(target),
        }),
    };
    [tempo, play_state, seek]
}

fn publish_transport_event<T, S>(state: &SessionState<T, S>, event: &TransportEvent) {
    for deck in state.graph.decks() {
        deck.bus.publish(event.clone());
    }
}
