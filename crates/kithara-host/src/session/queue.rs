//! The Host queue: Host settings changes the session transport applies on
//! its render clock, and the receipts that settle them on the session owner.

use std::mem;

use kithara_command::{LiveError, Outcome, Protocol, Receipt, Rejection, SendError, Target, When};
use kithara_config::ConfigOwner;
use kithara_play::PlayError;
use kithara_signal::SessionFrame;
use tracing::{error, warn};

use super::{
    SessionError,
    state::SessionState,
    transport::{TransportEvent, TransportProcessError, publish_transport_event},
};
use crate::{
    api::{Tempo, TransportRevision},
    host::{HostSettingsChange, HostSettingsExec},
};

/// One command the session transport applies.
#[derive(Clone, Copy, Debug)]
pub(crate) enum HostPart {
    /// A change of one Host setting.
    Settings(HostSettingsChange),
}

/// What the session owner and the session transport say to each other.
#[derive(Debug)]
pub(crate) enum HostProtocol {}

/// No batch of the Host queue shifts the time of a target.
#[derive(Clone, Copy, Debug)]
pub(crate) enum NoTarget {}

impl Target for NoTarget {
    fn index(self) -> usize {
        match self {}
    }
}

impl Protocol for HostProtocol {
    type Applied = TransportRevision;
    type Clock = SessionFrame;
    type Command = HostPart;
    type Refusal = TransportProcessError;
    type Target = NoTarget;

    fn frames_since(at: SessionFrame, start: SessionFrame) -> Option<u64> {
        at.frames_since(start)
    }
}

impl<T, S> HostSettingsExec<()> for SessionState<T, S> {
    type At = When<SessionFrame>;
    type Output = Result<(), PlayError>;

    /// Without a render graph the tempo applies at once, since no clock runs
    /// to place a frame on; with one, it goes to the transport, which
    /// re-anchors the session beats on the asked frame.
    fn exec_tempo(&mut self, value: Tempo, at: Self::At, _cx: &mut ()) -> Self::Output {
        let change = HostSettingsChange::Tempo(value);
        if let When::At(frame) = at
            && frame < self.earliest_frame().ok_or(PlayError::Untimed)?
        {
            return Err(PlayError::Late);
        }
        let Some(control) = self.transport_control.as_mut() else {
            let Ok(()) = self.settings.apply(change);
            self.publish_root();
            return Ok(());
        };
        self.settings
            .send(control.queue(), at, change, HostPart::Settings)
            .map(drop)
            .map_err(|error| match error {
                LiveError::Invalid(never) => match never {},
                LiveError::Send(SendError::Full(_)) => SessionError::HostQueueFull.into(),
                LiveError::Send(SendError::Target(_)) => {
                    PlayError::Internal("a host settings batch names a target".to_owned())
                }
            })
    }
}

impl<T, S> SessionState<T, S> {
    /// The first frame a change can still reach: the transport applies a
    /// change no earlier than the block after the one rendering now.
    fn earliest_frame(&self) -> Option<SessionFrame> {
        let ctx = self.ctx.as_ref()?;
        let block = ctx.stream_info()?.max_block_frames.get();
        ctx.audio_clock()
            .samples
            .0
            .checked_add(i64::from(block))
            .map(SessionFrame::new)
    }
}

/// Settles every receipt the transport returned. An applied change moves
/// into the settings the Host reads and announces a tempo it changed. A
/// change for the next block the transport refused goes out again; any other
/// rejected change is dropped and reported.
pub(crate) fn settle_receipts<T, S>(state: &mut SessionState<T, S>) {
    let mut applied = false;
    while let Some(receipt) = state
        .transport_control
        .as_mut()
        .and_then(|control| control.queue().receipts().next())
    {
        let before = *state.settings.config();
        let Some(settled) = state.settings.settle(&receipt) else {
            continue;
        };
        match receipt.outcome() {
            Outcome::Applied { data, .. } => {
                applied = true;
                let HostSettingsChange::Tempo(tempo) = settled.change;
                if tempo == before.tempo() {
                    continue;
                }
                publish_transport_event(
                    state,
                    &TransportEvent::TempoCommitted {
                        revision: u64::from(*data),
                        beats_per_minute: tempo.beats_per_minute(),
                    },
                );
            }
            Outcome::Rejected(Rejection::Unanswered) => {
                error!(change = ?settled.change, "the transport dropped a host settings change unanswered");
            }
            Outcome::Rejected(Rejection::Refused(_)) if settled.when == When::Next => {
                send_again(state, &receipt, settled.change);
            }
            Outcome::Rejected(rejection) => {
                warn!(?rejection, change = ?settled.change, "host settings change was not applied");
                if let When::At(_) = settled.when {
                    publish_transport_event(
                        state,
                        &TransportEvent::Failed {
                            revision: None,
                            reason: format!("host settings change was not applied: {rejection:?}"),
                        },
                    );
                }
            }
        }
    }
    if applied {
        state.publish_root();
    }
}

/// Sends a change for the next block the transport refused once more, unless
/// a newer change of the same field for the next block is already on its way
/// and decides the setting instead.
fn send_again<T, S>(
    state: &mut SessionState<T, S>,
    receipt: &Receipt<HostProtocol>,
    change: HostSettingsChange,
) {
    let superseded = state.settings.pending().any(|(seq, when, pending)| {
        seq > receipt.seq()
            && when == When::Next
            && mem::discriminant(&pending) == mem::discriminant(&change)
    });
    if superseded {
        return;
    }
    if let Err(error) = state.exec(change, When::Next, &mut ()) {
        warn!(%error, ?change, "a refused host settings change could not be sent again");
    }
}
