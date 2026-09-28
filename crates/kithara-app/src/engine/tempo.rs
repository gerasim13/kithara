use kithara::play::{PlayError, SessionError, Tempo};
use tracing::{debug, error};

/// The session tempo the engine asks the Host for, and what the Host did
/// with it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct HostTempo {
    /// Tempo the engine asks the Host for.
    pub(crate) target: Tempo,
    /// Tempo the configuration starts the Host at.
    pub(crate) configured: Tempo,
    /// Tempo the Host accepted last; `None` before the first.
    accepted: Option<Tempo>,
    /// Tempo of the commit the Host's graph processed last; `None` before
    /// the first.
    pub(crate) processed: Option<Tempo>,
    /// Whether the Host refused `target`.
    pub(crate) is_refused: bool,
}

impl HostTempo {
    pub(crate) const fn new(configured: Tempo) -> Self {
        Self {
            configured,
            target: configured,
            accepted: None,
            processed: None,
            is_refused: false,
        }
    }

    /// Asks for `target` from now on. What the Host accepted was read for
    /// the old target, so the new one stays pending until an observation
    /// made after the retarget shows it settled.
    pub(crate) const fn retarget(&mut self, target: Tempo) {
        self.target = target;
        self.accepted = None;
        self.is_refused = false;
    }

    /// Whether the Host has yet to settle on the target: settled is one
    /// observation in which the Host accepted it and its graph processed it.
    pub(crate) fn is_pending(&self) -> bool {
        !self.is_refused
            && (self.accepted != Some(self.target) || self.processed != Some(self.target))
    }

    /// Whether a graph that processed a tempo before has yet to settle on
    /// the target. Before any graph processed one, the target waits for
    /// playback to start one.
    pub(super) fn is_settling(&self) -> bool {
        self.processed.is_some() && self.is_pending()
    }

    /// Takes in the tempo the Host accepted and the one its graph processed,
    /// and names the tempo to ask it for: the target, while the Host has not
    /// taken it.
    pub(super) fn observe(
        &mut self,
        accepted: Option<Tempo>,
        processed: Option<Tempo>,
    ) -> Option<Tempo> {
        self.accepted = accepted;
        self.processed = processed;
        (!self.is_refused && accepted != Some(self.target)).then_some(self.target)
    }

    /// Takes in the Host's error to a tempo query or ask. The Host answers
    /// for the transport, not for the tempo, when it holds one commit until
    /// the graph processed it or a route restart holds the session grid
    /// (`TransportNotProcessed`), when the render boundary rejected a commit
    /// on its timing and the Host fell back to the processed transport
    /// (`TransportCommitRejected`), or when its control was busy past the
    /// bounded wait (`SyncControlBusy`): the target waits for a later
    /// publish. Any other error refuses the target until it changes.
    pub(super) fn hear(&mut self, error: &PlayError) {
        if matches!(
            error,
            PlayError::Session(
                SessionError::TransportNotProcessed
                    | SessionError::TransportCommitRejected
                    | SessionError::SyncControlBusy
            )
        ) {
            debug!(target = ?self.target, %error, "Host takes the session tempo later");
            return;
        }
        error!(target = ?self.target, %error, "Host refused the session tempo");
        self.is_refused = true;
    }
}

#[cfg(test)]
mod tests {
    use kithara_test_utils::kithara;

    use super::*;

    fn bpm(beats_per_minute: f64) -> Tempo {
        Tempo::new(beats_per_minute).expect("fixture tempo")
    }

    fn observe(
        tempo: &mut HostTempo,
        accepted: Option<f64>,
        processed: Option<f64>,
    ) -> Option<Tempo> {
        tempo.observe(accepted.map(bpm), processed.map(bpm))
    }

    fn not_processed() -> PlayError {
        PlayError::Session(SessionError::TransportNotProcessed)
    }

    #[kithara::test]
    fn a_target_is_asked_for_until_the_host_takes_it_and_waits_for_a_graph_to_process_it() {
        let mut tempo = HostTempo::new(bpm(120.0));
        assert_eq!(
            observe(&mut tempo, None, None),
            Some(bpm(120.0)),
            "a fresh Host is asked for the configured tempo"
        );
        assert_eq!(observe(&mut tempo, Some(120.0), None), None);
        assert!(tempo.is_pending(), "no graph processed it yet");
        assert!(
            !tempo.is_settling(),
            "nothing plays, so nothing will process it"
        );

        tempo.retarget(bpm(124.0));
        assert_eq!(
            observe(&mut tempo, Some(120.0), Some(120.0)),
            Some(bpm(124.0))
        );
        assert_eq!(
            observe(&mut tempo, Some(120.0), Some(120.0)),
            Some(bpm(124.0)),
            "a Host that did not take the target is asked again"
        );
        assert_eq!(
            observe(&mut tempo, Some(124.0), Some(120.0)),
            None,
            "the Host took the target"
        );
        assert!(tempo.is_settling(), "the running graph processes it next");

        assert_eq!(observe(&mut tempo, Some(124.0), Some(124.0)), None);
        assert!(!tempo.is_pending());
        assert!(!tempo.is_settling());
        assert_eq!(tempo.processed, Some(bpm(124.0)));
    }

    #[kithara::test]
    fn returning_to_the_processed_tempo_stays_pending_while_another_commit_is_held() {
        let mut tempo = HostTempo::new(bpm(120.0));
        assert_eq!(observe(&mut tempo, Some(120.0), Some(120.0)), None);
        assert!(!tempo.is_pending());

        tempo.retarget(bpm(124.0));
        assert_eq!(
            observe(&mut tempo, Some(120.0), Some(120.0)),
            Some(bpm(124.0))
        );
        assert_eq!(observe(&mut tempo, Some(124.0), Some(120.0)), None);

        tempo.retarget(bpm(120.0));
        assert!(
            tempo.is_settling(),
            "the graph is about to process 124, not the target"
        );
        assert_eq!(
            observe(&mut tempo, Some(124.0), Some(120.0)),
            Some(bpm(120.0))
        );
        tempo.hear(&not_processed());
        assert_eq!(
            observe(&mut tempo, Some(124.0), Some(124.0)),
            Some(bpm(120.0))
        );
        assert!(tempo.is_settling());
        assert_eq!(observe(&mut tempo, Some(120.0), Some(124.0)), None);
        assert!(tempo.is_settling(), "the graph still plays 124");

        assert_eq!(observe(&mut tempo, Some(120.0), Some(120.0)), None);
        assert!(!tempo.is_pending());
    }

    #[kithara::test]
    fn a_retarget_reads_pending_until_the_host_is_observed_again() {
        let mut tempo = HostTempo::new(bpm(120.0));
        assert_eq!(observe(&mut tempo, Some(120.0), Some(120.0)), None);

        tempo.retarget(bpm(124.0));
        assert_eq!(
            observe(&mut tempo, Some(120.0), Some(120.0)),
            Some(bpm(124.0))
        );
        tempo.retarget(bpm(120.0));
        assert!(
            tempo.is_settling(),
            "the Host may have taken 124 since it was last observed"
        );
        assert_eq!(
            observe(&mut tempo, Some(124.0), Some(120.0)),
            Some(bpm(120.0))
        );
    }

    #[kithara::test]
    #[case::held_or_restarting(SessionError::TransportNotProcessed)]
    #[case::rejected_on_its_timing(SessionError::TransportCommitRejected)]
    #[case::control_busy(SessionError::SyncControlBusy)]
    fn a_host_answering_for_its_transport_is_asked_again_on_a_later_publish(
        #[case] answer: SessionError,
    ) {
        let mut tempo = HostTempo::new(bpm(120.0));
        assert_eq!(observe(&mut tempo, Some(120.0), Some(120.0)), None);

        tempo.retarget(bpm(124.0));
        tempo.hear(&PlayError::Session(answer));
        assert!(tempo.is_settling(), "the target keeps waiting");
        assert_eq!(
            observe(&mut tempo, Some(120.0), Some(120.0)),
            Some(bpm(124.0)),
            "the Host is asked for the target again"
        );
    }

    #[kithara::test]
    fn a_refused_target_is_not_asked_for_again_until_the_target_changes() {
        let mut tempo = HostTempo::new(bpm(120.0));
        tempo.retarget(bpm(124.0));
        tempo.hear(&PlayError::Session(
            SessionError::TransportRevisionExhausted,
        ));

        assert_eq!(observe(&mut tempo, Some(120.0), Some(120.0)), None);
        assert!(!tempo.is_pending(), "a refused target waits for nothing");

        tempo.retarget(bpm(126.0));
        assert_eq!(
            observe(&mut tempo, Some(120.0), Some(120.0)),
            Some(bpm(126.0))
        );
    }
}
