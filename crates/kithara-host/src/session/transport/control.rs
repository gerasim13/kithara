use std::num::NonZeroU32;

use kithara_sync::{ParentGridUpdate, SyncError};
use kithara_warp::{BeatGrid, BeatGridState, MapAxis};

use super::{
    commit::{SessionGridGeneration, TransportObservation},
    event::TransportEvent,
    process::converge_transport_restart,
};
use crate::{
    api::SessionTransportSnapshot,
    session::{SessionError, state::SessionState},
};

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
    publish_committed(state, &observation)?;
    Ok(observation)
}

/// Brings the root group up to what the render graph has committed: on every
/// session tick and offline block, and before a synchronization command reads
/// it, so the Host grid follows the tempo it clicks with no deck ticking.
///
/// Nothing is committed while no graph runs or a route restart holds the
/// session grid.
///
/// # Errors
///
/// Returns the root group's refusal of the committed session grid.
pub(crate) fn observe_commits<T, S>(state: &mut SessionState<T, S>) -> Result<(), SyncError> {
    if state.reserved_session_grid.is_some() {
        return Ok(());
    }
    let Some(control) = state.transport_control.as_mut() else {
        return Ok(());
    };
    let observation = control.observation();
    publish_committed(state, &observation)
}

/// Publishes the committed session grid on the root group; idempotent.
fn publish_committed<T, S>(
    state: &mut SessionState<T, S>,
    observation: &TransportObservation,
) -> Result<(), SyncError> {
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
    Ok(())
}

pub(crate) fn publish_transport_event<T, S>(state: &SessionState<T, S>, event: &TransportEvent) {
    for deck in state.graph.decks() {
        deck.bus.publish(event.clone());
    }
}
