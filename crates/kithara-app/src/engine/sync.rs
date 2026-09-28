use kithara::{
    host::{
        DeckSyncState, SyncError, SyncExecutionReject, SyncIntent, SyncMode, SyncStatusSnapshot,
    },
    play::{PlayError, SessionError},
    warp::BeatsPerMinute,
};
use tracing::{debug, error};

use crate::deck::DeckId;

/// One deck's SYNC: what the user asked for, and what the Host answered.
#[derive(Clone, Copy, Debug, Default, PartialEq, fieldwork::Fieldwork)]
#[fieldwork(opt_in, get)]
pub(crate) struct DeckSync {
    /// Whether the user asked for SYNC on, while the Host's mode has yet to
    /// meet the ask; `None` otherwise.
    #[field(get, copy, vis = "pub(crate)")]
    wish: Option<bool>,
    /// What the Host answered last; `None` before the first answer.
    pub(crate) reported: Option<SyncReport>,
    /// Whether the Host refused the last ask.
    pub(crate) is_refused: bool,
}

/// What a deck draws of the Host's answer.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct SyncReport {
    /// Mode the Host accepted.
    pub(crate) mode: SyncMode,
    /// How far the deck's alignment has come.
    pub(crate) phase: SyncPhase,
    /// Tempo the deck's map sounds at; `None` while no map sounds.
    pub(crate) applied_tempo: Option<BeatsPerMinute>,
}

/// How far a deck's alignment has come, as the Host's status tells it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum SyncPhase {
    /// No correction is under way.
    Off,
    /// The alignment waits for beats the track's grid does not cover yet.
    WaitingForGrid,
    /// An alignment is planned and has yet to sound.
    Preparing,
    /// The deck sounds its alignment and approaches the target phase.
    Converging,
    /// The deck holds the target tempo and phase.
    Locked,
    /// The latest alignment ended without sounding, for this reason.
    Rejected(SyncExecutionReject),
}

impl DeckSync {
    /// Asks for SYNC on or off from now on; a refusal belonged to the old ask.
    pub(crate) const fn request(&mut self, on: bool) {
        self.wish = Some(on);
        self.is_refused = false;
    }

    /// Whether the Host holds the deck on its timeline.
    pub(crate) fn is_synced(&self) -> bool {
        self.reported
            .is_some_and(|report| report.mode == SyncMode::HostSync)
    }

    /// Whether SYNC stands on for the user: the pending ask, else the
    /// Host's mode.
    pub(crate) fn wants_on(&self) -> bool {
        self.wish.unwrap_or_else(|| self.is_synced())
    }

    /// Takes in the Host's answer and names the intent to ask it for: the
    /// ask, while the accepted mode does not meet it. A met ask is done and
    /// the mode speaks from then on, so an alignment the Host rejects later
    /// is not asked for again.
    pub(super) fn observe(&mut self, report: SyncReport) -> Option<SyncIntent> {
        self.reported = Some(report);
        let on = self.wish?;
        if on == (report.mode == SyncMode::HostSync) {
            self.wish = None;
            return None;
        }
        Some(if on {
            SyncIntent::Enable
        } else {
            SyncIntent::Disable
        })
    }

    /// Takes in the Host's error to an ask. The Host answers for the moment,
    /// not for the ask, while the deck renders nothing to align on
    /// (`NotReady`), while it holds a commit its graph has not processed
    /// (`TransportNotProcessed`), while its control is busy past the bounded
    /// wait (`SyncControlBusy`), or while an armed alignment is committed to
    /// the output (`ArmedOperation`): the ask waits for a later publish. Any
    /// other error refuses the ask until the user asks again.
    pub(super) fn hear(&mut self, deck: DeckId, error: &PlayError) {
        if matches!(
            error,
            PlayError::NotReady
                | PlayError::Session(
                    SessionError::TransportNotProcessed
                        | SessionError::SyncControlBusy
                        | SessionError::Sync(SyncError::ArmedOperation { .. })
                )
        ) {
            debug!(deck = deck.0, wish = ?self.wish, %error, "Host takes the deck SYNC later");
            return;
        }
        error!(deck = deck.0, wish = ?self.wish, %error, "Host refused the deck SYNC");
        self.wish = None;
        self.is_refused = true;
    }
}

impl From<&DeckSyncState> for SyncReport {
    fn from(state: &DeckSyncState) -> Self {
        Self {
            mode: state.mode,
            phase: SyncPhase::from(&state.status),
            applied_tempo: state.applied_tempo,
        }
    }
}

impl From<&SyncStatusSnapshot> for SyncPhase {
    /// A status this build does not know reads as no correction.
    fn from(status: &SyncStatusSnapshot) -> Self {
        match status {
            SyncStatusSnapshot::WaitingForGrid { .. } => Self::WaitingForGrid,
            SyncStatusSnapshot::Prepared { .. } | SyncStatusSnapshot::Replanning { .. } => {
                Self::Preparing
            }
            SyncStatusSnapshot::Converging { .. } => Self::Converging,
            SyncStatusSnapshot::Locked { .. } => Self::Locked,
            SyncStatusSnapshot::Rejected { reason, .. } => Self::Rejected(*reason),
            _ => Self::Off,
        }
    }
}

#[cfg(test)]
mod tests {
    use ::kithara::{sync::SyncOperationId, warp::BeatGridId};
    use kithara_test_utils::kithara;

    use super::*;

    mod consts {
        use crate::deck::DeckId;

        pub(super) const DECK: DeckId = DeckId(0);
    }

    fn report(mode: SyncMode, phase: SyncPhase) -> SyncReport {
        SyncReport {
            mode,
            phase,
            applied_tempo: None,
        }
    }

    fn armed() -> SessionError {
        SessionError::Sync(SyncError::ArmedOperation {
            member_id: BeatGridId::allocate().expect("fixture grid identity"),
            operation: SyncOperationId::first(),
        })
    }

    #[kithara::test]
    fn each_ask_is_repeated_until_the_host_mode_meets_it() {
        let mut sync = DeckSync::default();
        assert_eq!(sync.observe(report(SyncMode::Off, SyncPhase::Off)), None);
        assert!(!sync.wants_on());

        sync.request(true);
        assert!(sync.wants_on(), "the ask stands before the Host answers");
        assert_eq!(
            sync.observe(report(SyncMode::Off, SyncPhase::Off)),
            Some(SyncIntent::Enable)
        );
        assert_eq!(
            sync.observe(report(SyncMode::Off, SyncPhase::Off)),
            Some(SyncIntent::Enable),
            "a Host that did not take the ask is asked again"
        );
        assert_eq!(
            sync.observe(report(SyncMode::HostSync, SyncPhase::Preparing)),
            None
        );
        assert_eq!(sync.wish(), None, "the met ask is done");
        assert!(sync.is_synced());
        assert!(sync.wants_on());

        sync.request(false);
        assert_eq!(
            sync.observe(report(SyncMode::HostSync, SyncPhase::Locked)),
            Some(SyncIntent::Disable)
        );
        assert_eq!(
            sync.observe(report(SyncMode::LocalSync, SyncPhase::Preparing)),
            None
        );
        assert!(!sync.wants_on());

        sync.request(true);
        assert_eq!(
            sync.observe(report(SyncMode::LocalSync, SyncPhase::Locked)),
            Some(SyncIntent::Enable),
            "a released deck is asked onto the Host again"
        );
    }

    #[kithara::test]
    #[case::renders_nothing(PlayError::NotReady)]
    #[case::holds_a_commit(PlayError::Session(SessionError::TransportNotProcessed))]
    #[case::control_busy(PlayError::Session(SessionError::SyncControlBusy))]
    #[case::armed(PlayError::Session(armed()))]
    fn a_host_answering_for_the_moment_is_asked_again_on_a_later_publish(
        #[case] answer: PlayError,
    ) {
        let mut sync = DeckSync::default();
        sync.request(true);
        assert_eq!(
            sync.observe(report(SyncMode::Off, SyncPhase::Off)),
            Some(SyncIntent::Enable)
        );

        sync.hear(consts::DECK, &answer);
        assert!(!sync.is_refused);
        assert_eq!(
            sync.observe(report(SyncMode::Off, SyncPhase::Off)),
            Some(SyncIntent::Enable),
            "the ask waits for a later publish"
        );
    }

    #[kithara::test]
    fn a_refused_ask_is_dropped_until_the_user_asks_again() {
        let mut sync = DeckSync::default();
        sync.request(true);
        assert_eq!(
            sync.observe(report(SyncMode::Off, SyncPhase::Off)),
            Some(SyncIntent::Enable)
        );

        sync.hear(consts::DECK, &PlayError::ForeignSession);
        assert!(sync.is_refused);
        assert_eq!(sync.observe(report(SyncMode::Off, SyncPhase::Off)), None);
        assert!(
            !sync.wants_on(),
            "a second press asks afresh instead of cancelling a dead ask"
        );

        sync.request(true);
        assert!(!sync.is_refused, "a new ask clears the old refusal");
        assert_eq!(
            sync.observe(report(SyncMode::Off, SyncPhase::Off)),
            Some(SyncIntent::Enable)
        );
    }

    #[kithara::test]
    fn an_alignment_rejected_after_the_host_took_the_ask_is_not_asked_again() {
        let mut sync = DeckSync::default();
        sync.request(true);
        assert_eq!(
            sync.observe(report(SyncMode::HostSync, SyncPhase::Preparing)),
            None
        );

        assert_eq!(
            sync.observe(report(
                SyncMode::Off,
                SyncPhase::Rejected(SyncExecutionReject::Capacity)
            )),
            None,
            "a terminal rejection is no loop of fresh asks"
        );
        assert!(!sync.wants_on());
    }
}
