use kithara::{platform::sync::Arc, play::Tempo};

use crate::{
    deck::DeckId,
    engine::{AppCmd, Command, DeckCmd, DeckSnapshot, EngineSnapshot, MixCmd},
    mix::MixStrip,
};

#[derive(Default)]
pub(crate) struct Overlay {
    pending: Vec<(u64, Edit)>,
}

#[derive(Clone, Copy)]
enum Edit {
    Mix(MixCmd),
    Deck(DeckId, DeckCmd),
    HostTempo(Tempo),
    Sync(DeckId, bool),
}

impl Overlay {
    pub(crate) fn over(&self, snapshot: &Arc<EngineSnapshot>) -> Arc<EngineSnapshot> {
        if self.pending.is_empty() {
            return Arc::clone(snapshot);
        }
        let mut drawn = EngineSnapshot::clone(snapshot);
        for (_, edit) in &self.pending {
            edit.lay_over(&mut drawn);
        }
        Arc::new(drawn)
    }

    pub(crate) fn record(&mut self, seq: u64, command: &Command) {
        let edit = match command {
            Command::Mix(cmd) => Edit::Mix(*cmd),
            Command::Deck {
                deck,
                cmd: cmd @ (DeckCmd::SetEqGain { .. } | DeckCmd::SetQuality(_) | DeckCmd::Tempo(_)),
            } => Edit::Deck(*deck, *cmd),
            Command::App(AppCmd::SetHostTempo(tempo)) => Edit::HostTempo(*tempo),
            Command::SetDeckSync { deck, on } => Edit::Sync(*deck, *on),
            Command::Deck { .. } | Command::LoadOntoDeck { .. } | Command::App(_) => return,
        };
        self.pending.push((seq, edit));
    }

    pub(crate) fn retire(&mut self, applied_seq: u64) {
        self.pending.retain(|(seq, _)| *seq > applied_seq);
    }
}

impl Edit {
    fn lay_over(self, drawn: &mut EngineSnapshot) {
        match self {
            Self::Mix(cmd) => lay_mix(cmd, drawn),
            Self::Deck(id, cmd) => lay_deck(id, cmd, drawn),
            Self::HostTempo(tempo) => drawn.host_tempo.retarget(tempo),
            Self::Sync(id, on) => {
                if let Some(deck) = deck_mut(drawn, id) {
                    deck.sync.request(on);
                }
            }
        }
    }
}

fn lay_deck(id: DeckId, cmd: DeckCmd, drawn: &mut EngineSnapshot) {
    let eq_mode = drawn.eq_mode;
    let Some(deck) = deck_mut(drawn, id) else {
        return;
    };
    match cmd {
        DeckCmd::SetEqGain { layout, band, gain } if layout == eq_mode => {
            if let Some(slot) = deck.eq_bands.get_mut(band) {
                *slot = gain;
            }
        }
        DeckCmd::SetQuality(variant) => {
            deck.stream.selected = variant;
            deck.stream.is_auto = variant.is_none();
        }
        DeckCmd::Tempo(change) if deck.sync.is_manual() => deck.tempo = change.manual(deck.tempo),
        _ => {}
    }
}

fn deck_mut(drawn: &mut EngineSnapshot, id: DeckId) -> Option<&mut DeckSnapshot> {
    drawn.decks.iter_mut().find(|deck| deck.id == id)
}

fn strip_mut(drawn: &mut EngineSnapshot, id: DeckId) -> Option<&mut MixStrip> {
    let at = drawn.decks.iter().position(|deck| deck.id == id)?;
    drawn.mix.strips.get_mut(at)
}

fn lay_mix(cmd: MixCmd, drawn: &mut EngineSnapshot) {
    match cmd {
        MixCmd::Crossfader(position) => drawn.mix.position = position,
        MixCmd::Master(gain) => drawn.mix.group_master = gain,
        MixCmd::Muted(id, muted) => {
            if let Some(strip) = strip_mut(drawn, id) {
                strip.muted = muted;
            }
        }
        MixCmd::Trim(id, trim) => {
            if let Some(strip) = strip_mut(drawn, id) {
                strip.trim = trim;
            }
        }
    }
}

#[cfg(all(test, not(feature = "broadcast")))]
mod tests {
    use std::convert::Infallible;

    use ::kithara::ui::render::ControlAction;
    use kithara_test_utils::{kithara, off_thread::OffThread};

    use crate::gui::rig::Rig;

    #[kithara::test(native, tokio, flash(false))]
    async fn a_moved_fader_draws_its_position_before_and_after_the_echo() {
        let rig = OffThread::spawn("engine", || Ok::<_, Infallible>(Rig::offline()))
            .await
            .expect("rig fixture is infallible");
        rig.call(|rig| {
            assert!((rig.scalar("mix.crossfader") - 0.5).abs() < f64::EPSILON);
            let before = rig.applied_seq();

            rig.send("mixer/xfade", ControlAction::SetScalar(1.0));

            assert!(
                (rig.scalar("mix.crossfader") - 1.0).abs() < f64::EPSILON,
                "the fader draws the position it was moved to at once"
            );
            assert_eq!(rig.applied_seq(), before, "the engine has not applied it");
            assert!((rig.snapshots.load().mix.position - 0.5).abs() < f32::EPSILON);

            rig.frame();
            assert!(
                (rig.scalar("mix.crossfader") - 1.0).abs() < f64::EPSILON,
                "a frame that reloads an older snapshot keeps the pending position"
            );

            let applied = rig.pump();
            assert_eq!(applied.len(), 1, "one move, one command");
            rig.frame();

            assert_eq!(rig.applied_seq(), applied[0]);
            assert!((rig.snapshots.load().mix.position - 1.0).abs() < f32::EPSILON);
            assert!(
                (rig.scalar("mix.crossfader") - 1.0).abs() < f64::EPSILON,
                "the echoed snapshot keeps the position"
            );
        })
        .await;
        rig.close().await;
    }

    #[kithara::test(native, tokio)]
    async fn a_tempo_step_on_a_manual_deck_draws_before_the_echo() {
        let rig = OffThread::spawn("engine", || Ok::<_, Infallible>(Rig::offline()))
            .await
            .expect("rig fixture is infallible");
        rig.call(|rig| {
            let tempo = |rig: &Rig| rig.text("deck.playback.tempo@deck=a");

            rig.send("deck-a/tempo", ControlAction::StepScalar(1.0));
            assert_eq!(
                tempo(rig).as_deref(),
                Some("+1.5%"),
                "the step draws before the engine applies it"
            );

            rig.pump();
            rig.frame();
            assert_eq!(
                tempo(rig).as_deref(),
                Some("+1.5%"),
                "the echoed snapshot keeps the step"
            );
        })
        .await;
        rig.close().await;
    }

    #[kithara::test(native, tokio)]
    async fn a_second_sync_press_before_the_echo_takes_the_first_back() {
        let rig = OffThread::spawn("engine", || Ok::<_, Infallible>(Rig::offline()))
            .await
            .expect("rig fixture is infallible");
        rig.call(|rig| {
            let word = |rig: &Rig| rig.text("deck.playback.sync_state@deck=a");

            rig.send("deck-a/sync", ControlAction::Activate);
            assert_eq!(
                word(rig).as_deref(),
                Some("WAITS FOR PLAY"),
                "the press draws its ask before the engine applies it"
            );

            rig.send("deck-a/sync", ControlAction::Activate);
            assert_eq!(
                word(rig).as_deref(),
                Some(""),
                "the second press reads the drawn ask and takes it back"
            );

            rig.pump();
            rig.frame();
            assert_eq!(word(rig).as_deref(), Some(""));
            assert!(
                !rig.flag("deck.playback.synced@deck=a"),
                "the deck never left manual"
            );
        })
        .await;
        rig.close().await;
    }
}
