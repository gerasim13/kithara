use kithara::play::Tempo;

/// The session tempo the engine asks the Host for, and what the Host did
/// with it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct HostTempo {
    /// Tempo the engine asks the Host for.
    pub(crate) target: Tempo,
    /// Tempo the configuration starts the Host at.
    pub(crate) configured: Tempo,
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
            processed: None,
            is_refused: false,
        }
    }

    pub(crate) const fn retarget(&mut self, target: Tempo) {
        self.target = target;
        self.is_refused = false;
    }

    pub(super) const fn refuse(&mut self) {
        self.is_refused = true;
    }

    /// Whether the Host's graph has yet to process the target.
    pub(crate) fn is_pending(&self) -> bool {
        !self.is_refused && self.processed != Some(self.target)
    }

    /// Whether a graph that processed a tempo before has yet to process the
    /// target. Before any graph processed one, the target waits for playback
    /// to start one.
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
        self.processed = processed;
        (!self.is_refused && accepted != Some(self.target)).then_some(self.target)
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
    fn a_refused_target_is_not_asked_for_again_until_the_target_changes() {
        let mut tempo = HostTempo::new(bpm(120.0));
        tempo.retarget(bpm(124.0));
        tempo.refuse();

        assert_eq!(observe(&mut tempo, Some(120.0), Some(120.0)), None);
        assert!(!tempo.is_pending(), "a refused target waits for nothing");

        tempo.retarget(bpm(126.0));
        assert_eq!(
            observe(&mut tempo, Some(120.0), Some(120.0)),
            Some(bpm(126.0))
        );
    }
}
