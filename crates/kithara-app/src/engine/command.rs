use kithara::{effects::GainDb, play::Tempo, queue::TrackId, warp::BeatsPerMinute};

use crate::deck::{DeckId, EqMode, TempoPercent};

#[derive(Debug)]
pub(crate) struct Envelope {
    pub(crate) command: Command,
    pub(crate) seq: u64,
}

#[derive(Debug)]
pub(crate) enum Command {
    Deck { deck: DeckId, cmd: DeckCmd },
    Mix(MixCmd),
    LoadOntoDeck { deck: DeckId, source: String },
    SetDeckSync { deck: DeckId, on: bool },
    App(AppCmd),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum DeckCmd {
    Play,
    Pause,
    Next,
    Prev,
    SeekFraction(f64),
    SetEqGain {
        layout: EqMode,
        band: usize,
        gain: GainDb,
    },
    RemoveTrack(TrackId),
    Tempo(TempoChange),
    SetQuality(Option<usize>),
}

/// How the user moves a deck's tempo, resolved by whoever owns it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum TempoChange {
    /// `steps` wheel detents, each worth [`TempoPercent::STEP`].
    Step(f32),
    /// Back to the track's own tempo.
    Reset,
}

impl TempoChange {
    /// The manual tempo this change leaves `tempo` at, clamped to its travel.
    pub(crate) fn manual(self, tempo: TempoPercent) -> TempoPercent {
        match self {
            Self::Step(steps) => TempoPercent::from(steps.mul_add(TempoPercent::STEP, tempo.0)),
            Self::Reset => TempoPercent::DEFAULT,
        }
    }

    /// The deck tempo this change asks the Host for: `accepted` scaled by one
    /// detent's factor per step, so steps compose and a step back undoes a
    /// step on, or the track's `native` BPM for a reset. `None` while the one
    /// it needs is unknown.
    pub(crate) fn own(
        self,
        accepted: Option<BeatsPerMinute>,
        native: Option<f64>,
    ) -> Option<BeatsPerMinute> {
        let bpm = match self {
            Self::Step(steps) => {
                let detent = f64::from(TempoPercent::from(TempoPercent::STEP).speed());
                f64::from(accepted?) * detent.powf(f64::from(steps))
            }
            Self::Reset => native?,
        };
        BeatsPerMinute::try_from(bpm).ok()
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum MixCmd {
    Crossfader(f32),
    Master(f32),
    Muted(DeckId, bool),
    Trim(DeckId, f32),
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum AppCmd {
    SetEqMode(EqMode),
    SetHostTempo(Tempo),
    BroadcastToggle,
    Shutdown,
}

#[cfg(all(test, not(feature = "broadcast")))]
mod tests {
    use std::convert::Infallible;

    use ::kithara::{effects::GainDb, ui::render::ControlAction};
    use kithara_test_utils::{kithara, off_thread::OffThread};

    use super::TempoChange;
    use crate::{deck::TempoPercent, gui::rig::Rig};

    mod consts {
        pub(super) const KNOB: f64 = 1e-4;
    }

    fn bpm(value: f64) -> ::kithara::warp::BeatsPerMinute {
        ::kithara::warp::BeatsPerMinute::try_from(value).expect("fixture tempo")
    }

    #[kithara::test]
    fn a_manual_step_moves_the_percent_by_its_detents_within_the_travel() {
        let from = TempoPercent::from(3.0);

        assert_eq!(TempoChange::Step(2.0).manual(from), TempoPercent::from(6.0));
        assert_eq!(
            TempoChange::Step(-1.0).manual(from),
            TempoPercent::from(1.5)
        );
        assert_eq!(TempoChange::Step(80.0).manual(from), TempoPercent::MAX);
        assert_eq!(TempoChange::Reset.manual(from), TempoPercent::DEFAULT);
    }

    #[kithara::test]
    fn an_own_step_moves_the_accepted_tempo_and_a_reset_returns_to_the_track() {
        let step = TempoChange::Step(1.0).own(Some(bpm(124.0)), Some(120.0));
        assert!(
            step.is_some_and(|tempo| (f64::from(tempo) - 125.86).abs() < 1e-4),
            "one detent is 1.5 % of the accepted 124 BPM, got {step:?}"
        );
        assert_eq!(
            TempoChange::Reset.own(Some(bpm(124.0)), Some(120.0)),
            Some(bpm(120.0))
        );
        assert_eq!(TempoChange::Step(1.0).own(None, Some(120.0)), None);
        assert_eq!(TempoChange::Reset.own(Some(bpm(124.0)), None), None);
    }

    #[kithara::test]
    fn own_steps_compose_and_a_step_back_undoes_a_step_on() {
        let after = |start: f64, steps: &[f32]| {
            steps.iter().try_fold(bpm(start), |tempo, &detents| {
                TempoChange::Step(detents).own(Some(tempo), None)
            })
        };
        let near = |left: Option<::kithara::warp::BeatsPerMinute>, right: f64| {
            left.is_some_and(|tempo| (f64::from(tempo) - right).abs() < 1e-9)
        };

        assert!(
            near(after(124.0, &[10.0, -10.0]), 124.0),
            "out and back returns to 124 BPM, got {:?}",
            after(124.0, &[10.0, -10.0])
        );
        let whole = after(124.0, &[3.0]).map(f64::from).unwrap_or_default();
        assert!(
            near(after(124.0, &[1.0, 2.0]), whole),
            "one move split across events lands where it lands whole"
        );
    }

    #[kithara::test(native, tokio, flash(false))]
    async fn a_gain_for_a_replaced_band_layout_is_rejected_and_echoed() {
        let rig = OffThread::spawn("engine", || Ok::<_, Infallible>(Rig::offline()))
            .await
            .expect("rig fixture is infallible");
        rig.call(|rig| {
            let mid = f32::from(GainDb::at_knob(0.75));
            rig.send("mixer/a/mid-3", ControlAction::SetScalar(0.75));
            rig.pump();
            rig.frame();
            assert!((rig.scalar("deck.eq.mid@deck=a") - 0.75).abs() < consts::KNOB);
            assert_eq!(rig.queues[0].eq_gain(1), Some(mid));

            rig.send("mixer/a/eq-4", ControlAction::Activate);
            rig.send("mixer/a/high-3", ControlAction::SetScalar(0.0));
            let applied = rig.pump();
            assert_eq!(
                applied.len(),
                2,
                "the UI still draws three bands, so the gain names the three-band layout"
            );
            rig.frame();

            assert_eq!(
                rig.applied_seq(),
                applied[1],
                "a rejected command is echoed like any other"
            );
            let queue = &rig.queues[0];
            assert_eq!(queue.eq_band_count(), 4);
            assert_eq!(
                queue.eq_gain(2),
                Some(mid),
                "high-mid keeps the gain the mid band folded into it"
            );
            assert_eq!(queue.eq_gain(3), Some(0.0), "the high band is untouched");
            assert!((rig.scalar("deck.eq.bands@deck=a") - 4.0).abs() < f64::EPSILON);
            assert!((rig.scalar("deck.eq.high_mid@deck=a") - 0.75).abs() < consts::KNOB);
            assert!((rig.scalar("deck.eq.high@deck=a") - 0.5).abs() < consts::KNOB);
        })
        .await;
        rig.close().await;
    }
}
