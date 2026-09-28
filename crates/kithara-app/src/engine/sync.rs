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
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct DeckSync {
    /// The user's ask, while the Host has yet to answer for it; `None`
    /// otherwise.
    pub(crate) wish: Option<Wish>,
    /// What the Host answered last; `None` before the first answer.
    pub(crate) reported: Option<SyncReport>,
    /// Whether the Host refused the last ask.
    pub(crate) is_refused: bool,
}

/// A SYNC the user asked for that the Host has yet to answer for.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Wish {
    /// Whether the ask is for SYNC on.
    pub(crate) on: bool,
    /// How far the ask has come with the Host.
    pub(crate) stage: WishStage,
}

/// How far a pending SYNC ask has come with the Host.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum WishStage {
    /// Asked for on each publish until the Host takes it.
    Standing,
    /// Asked for on each publish while the Host waits for beats the track's
    /// grid does not cover yet.
    WaitsForBeats,
    /// The Host took the ask. An ask for ON is done at the Host's next
    /// answer; an ask for OFF is done once the mode leaves the Host timeline,
    /// since the Host may take a release it cannot make yet and keep the
    /// deck where it is.
    Admitted,
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
        self.wish = Some(Wish {
            on,
            stage: WishStage::Standing,
        });
        self.is_refused = false;
    }

    /// Whether the deck plays off the Host's timelines, at its manual tempo:
    /// the Host answered Off, or has not answered yet.
    pub(crate) fn is_manual(&self) -> bool {
        self.reported
            .is_none_or(|report| report.mode == SyncMode::Off)
    }

    /// Whether the Host holds the deck on its timeline.
    pub(crate) fn is_synced(&self) -> bool {
        self.reported
            .is_some_and(|report| report.mode == SyncMode::HostSync)
    }

    /// Whether SYNC stands on for the user: the pending ask, else the
    /// Host's mode.
    pub(crate) fn wants_on(&self) -> bool {
        self.wish.map_or_else(|| self.is_synced(), |wish| wish.on)
    }

    /// Takes in the Host's answer and names the intent to ask it for: the
    /// ask, while the accepted mode does not meet it. An ask the mode meets,
    /// or an ask for ON the Host took, is done and the mode speaks from then
    /// on, so an alignment the Host rejects later is not asked for again.
    pub(super) fn observe(&mut self, report: SyncReport) -> Option<SyncIntent> {
        self.reported = Some(report);
        let Wish { on, stage } = self.wish?;
        if (on && stage == WishStage::Admitted) || on == (report.mode == SyncMode::HostSync) {
            self.wish = None;
            return None;
        }
        Some(if on {
            SyncIntent::Enable
        } else {
            SyncIntent::Disable
        })
    }

    /// Takes in the Host's reply to the ask `observe` named. An ask the Host
    /// took is done once its next answer arrives. The Host answers for the
    /// moment, not for the ask, while the deck renders nothing to align on
    /// (`NotReady`), while it holds a commit its graph has not processed
    /// (`TransportNotProcessed`), while its control is busy past the bounded
    /// wait (`SyncControlBusy`), while an armed alignment is committed to the
    /// output (`ArmedOperation`), or while the track's grid does not cover the
    /// beats an alignment needs (`GridCoverageUnavailable`): the ask waits for
    /// a later publish. Any other error refuses the ask until the user asks
    /// again.
    pub(super) fn answer(&mut self, deck: DeckId, reply: &Result<(), PlayError>) {
        let stage = match reply {
            Ok(()) => WishStage::Admitted,
            Err(PlayError::Session(SessionError::Sync(SyncError::GridCoverageUnavailable {
                ..
            }))) => WishStage::WaitsForBeats,
            Err(
                PlayError::NotReady
                | PlayError::Session(
                    SessionError::TransportNotProcessed
                    | SessionError::SyncControlBusy
                    | SessionError::Sync(SyncError::ArmedOperation { .. }),
                ),
            ) => WishStage::Standing,
            Err(error) => {
                error!(deck = deck.0, wish = ?self.wish, %error, "Host refused the deck SYNC");
                self.wish = None;
                self.is_refused = true;
                return;
            }
        };
        if let Err(error) = reply {
            debug!(deck = deck.0, wish = ?self.wish, ?stage, %error, "Host takes the deck SYNC later");
        }
        if let Some(wish) = self.wish.as_mut() {
            wish.stage = stage;
        }
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

    fn no_beats() -> SessionError {
        SessionError::Sync(SyncError::GridCoverageUnavailable {
            member_id: BeatGridId::allocate().expect("fixture grid identity"),
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
        assert_eq!(sync.wish, None, "the met ask is done");
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
    #[case::no_beats_yet(PlayError::Session(no_beats()))]
    fn a_host_answering_for_the_moment_is_asked_again_on_a_later_publish(
        #[case] answer: PlayError,
    ) {
        let mut sync = DeckSync::default();
        sync.request(true);
        assert_eq!(
            sync.observe(report(SyncMode::Off, SyncPhase::Off)),
            Some(SyncIntent::Enable)
        );

        sync.answer(consts::DECK, &Err(answer));
        assert!(!sync.is_refused);
        assert_eq!(
            sync.observe(report(SyncMode::Off, SyncPhase::Off)),
            Some(SyncIntent::Enable),
            "the ask waits for a later publish"
        );
    }

    #[kithara::test]
    fn an_ask_the_host_waits_on_beats_for_is_named_by_them_until_it_is_taken() {
        let mut sync = DeckSync::default();
        sync.request(true);
        assert_eq!(
            sync.observe(report(SyncMode::Off, SyncPhase::Off)),
            Some(SyncIntent::Enable)
        );

        sync.answer(consts::DECK, &Err(PlayError::Session(no_beats())));
        assert_eq!(
            sync.wish,
            Some(Wish {
                on: true,
                stage: WishStage::WaitsForBeats
            })
        );

        sync.answer(consts::DECK, &Err(PlayError::NotReady));
        assert_eq!(
            sync.wish,
            Some(Wish {
                on: true,
                stage: WishStage::Standing
            }),
            "the Host's latest answer names the wait"
        );
    }

    #[kithara::test]
    fn an_ask_the_host_took_and_then_rejected_is_not_asked_again() {
        let mut sync = DeckSync::default();
        sync.request(true);
        assert_eq!(
            sync.observe(report(SyncMode::Off, SyncPhase::Off)),
            Some(SyncIntent::Enable)
        );
        sync.answer(consts::DECK, &Ok(()));

        assert_eq!(
            sync.observe(report(
                SyncMode::Off,
                SyncPhase::Rejected(SyncExecutionReject::Capacity)
            )),
            None,
            "the Host answered the ask it took; its rejection is no call for a fresh one"
        );
        assert!(!sync.wants_on());
    }

    #[kithara::test]
    fn a_release_the_host_took_but_deferred_is_asked_again_until_the_deck_leaves() {
        let mut sync = DeckSync::default();
        assert_eq!(
            sync.observe(report(SyncMode::HostSync, SyncPhase::Locked)),
            None
        );
        sync.request(false);
        assert_eq!(
            sync.observe(report(SyncMode::HostSync, SyncPhase::Locked)),
            Some(SyncIntent::Disable)
        );
        sync.answer(consts::DECK, &Ok(()));

        assert_eq!(
            sync.observe(report(SyncMode::HostSync, SyncPhase::WaitingForGrid)),
            Some(SyncIntent::Disable),
            "a release the Host could not make yet keeps the deck on its timeline"
        );
        assert!(!sync.wants_on(), "the press stands");
        sync.answer(consts::DECK, &Ok(()));
        assert_eq!(
            sync.observe(report(SyncMode::LocalSync, SyncPhase::Preparing)),
            None
        );
        assert_eq!(sync.wish, None, "the deck left the Host timeline");
    }

    #[kithara::test]
    fn a_refused_ask_is_dropped_until_the_user_asks_again() {
        let mut sync = DeckSync::default();
        sync.request(true);
        assert_eq!(
            sync.observe(report(SyncMode::Off, SyncPhase::Off)),
            Some(SyncIntent::Enable)
        );

        sync.answer(consts::DECK, &Err(PlayError::ForeignSession));
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
