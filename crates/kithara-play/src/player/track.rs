//! One track on a deck: the parts that name it and the lane it reads.

use kithara_command::{Live, LiveError, Outcome, Sender, Seq, When};
use kithara_config::ConfigOwner;
use kithara_events::TrackId;
use kithara_render::{LaneCommand, LaneFrame, LaneProtocol};
use tracing::warn;

use crate::{
    PlayError,
    api::{CrossfadeSettings, SlotId},
    bridge::{DeckPart, SlotControl, TrackTransition},
    player::config::{TrackSettings, TrackSettingsChange, TrackSettingsExec},
    rt::track::PlayerResource,
};

/// A player: it changes its own state and reaches the executors only through
/// the outbox it is handed.
pub(crate) trait Player {
    /// What the player is told to do.
    type Command;

    /// Applies `command` on the owner's thread and returns the number of the
    /// batch it became, if one went out.
    ///
    /// # Errors
    ///
    /// Returns the deck's refusal of that batch; nothing changed then.
    fn apply(
        &mut self,
        command: Self::Command,
        out: &mut Outbox<'_>,
    ) -> Result<Option<Seq>, PlayError>;
}

/// The executors a player sends to: the deck its tracks sound on.
pub(crate) struct Outbox<'a> {
    slot: SlotId,
    deck: &'a mut SlotControl,
}

impl<'a> Outbox<'a> {
    pub(crate) const fn new(slot: SlotId, deck: &'a mut SlotControl) -> Self {
        Self { slot, deck }
    }

    /// Sends `parts` to the deck for its next block, admitted together or not
    /// at all.
    ///
    /// # Errors
    ///
    /// Returns [`PlayError::SlotChannelFull`] when the deck has no room for the
    /// batch.
    fn deck(&mut self, parts: Vec<DeckPart>) -> Result<Seq, PlayError> {
        self.deck
            .send_batch(parts)
            .map_err(|_| PlayError::SlotChannelFull { slot: self.slot })
    }
}

/// The track a loaded one starts behind, on the frame after its last.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Behind {
    pub(crate) track: TrackId,
    /// The playhead epoch the loaded track leads under once it starts.
    pub(crate) epoch: u64,
}

/// What a [`Track`] is told to do.
pub(crate) enum TrackCommand {
    /// Put the track on the deck, reading its lane from `lane`; `behind` a
    /// track, it starts on the frame after that track's last. The deck admits
    /// the track and its chain together or not at all.
    Load {
        resource: Box<PlayerResource>,
        lane: Option<Sender<LaneProtocol>>,
        behind: Option<Behind>,
    },
    /// Fade the track in with `settings`, leading under `epoch`.
    FadeIn {
        settings: CrossfadeSettings,
        epoch: u64,
    },
    /// Take the track off the deck.
    Release,
    /// Take the track off the deck only while it still preloads: one already
    /// stitched in keeps playing.
    Withdraw,
    /// Change one of the track's settings for its lane to apply at `at`.
    Configure(TrackSettingsChange, When<LaneFrame>),
}

/// One track a deck holds: every part it sends names it, and its settings
/// follow its lane's receipts.
#[derive(fieldwork::Fieldwork)]
#[fieldwork(opt_in)]
pub(crate) struct Track {
    #[field(get, copy, vis = "pub(crate)")]
    item_id: TrackId,
    lane: Option<Sender<LaneProtocol>>,
    settings: Live<TrackSettings, LaneProtocol>,
}

impl Track {
    /// A track not yet on the deck that starts with `settings`.
    ///
    /// # Errors
    ///
    /// Returns the settings check's refusal.
    pub(crate) fn new(item_id: TrackId, settings: TrackSettings) -> Result<Self, PlayError> {
        Ok(Self {
            item_id,
            lane: None,
            settings: Live::new(settings)?,
        })
    }

    /// Attaches the track, chained `behind` a track when one is named, then
    /// holds its lane, which starts at the track's speed.
    fn load(
        &mut self,
        resource: Box<PlayerResource>,
        lane: Option<Sender<LaneProtocol>>,
        behind: Option<Behind>,
        out: &mut Outbox<'_>,
    ) -> Result<Seq, PlayError> {
        let item_id = self.item_id;
        let mut parts = vec![DeckPart::Attach { item_id, resource }];
        if let Some(Behind { track, epoch }) = behind {
            parts.push(DeckPart::Chain {
                from: track,
                to: item_id,
                epoch,
            });
        }
        let seq = out.deck(parts)?;
        self.lane = lane;
        let speed = self.settings.config().speed();
        self.configure(TrackSettingsChange::Speed(speed), When::Next)?;
        Ok(seq)
    }

    /// Sends `change` to the track's lane; a refusal of the lane is logged.
    fn configure(
        &mut self,
        change: TrackSettingsChange,
        at: When<LaneFrame>,
    ) -> Result<Option<Seq>, PlayError> {
        match self.exec(change, at, &mut ()) {
            Ok(seq) => Ok(seq),
            Err(LiveError::Invalid(error)) => Err(error),
            Err(LiveError::Send(error)) => {
                warn!(%error, ?change, "track settings change not sent");
                Ok(None)
            }
        }
    }
}

impl Player for Track {
    type Command = TrackCommand;

    fn apply(
        &mut self,
        command: TrackCommand,
        out: &mut Outbox<'_>,
    ) -> Result<Option<Seq>, PlayError> {
        let item_id = self.item_id;
        let part = match command {
            TrackCommand::Load {
                resource,
                lane,
                behind,
            } => return self.load(resource, lane, behind, out).map(Some),
            TrackCommand::Configure(change, at) => return self.configure(change, at),
            TrackCommand::FadeIn { settings, epoch } => DeckPart::Fade(TrackTransition::FadeIn {
                item_id,
                settings,
                epoch,
            }),
            TrackCommand::Release => DeckPart::Detach { item_id },
            TrackCommand::Withdraw => DeckPart::Withdraw { item_id },
        };
        out.deck(vec![part]).map(Some)
    }
}

impl TrackSettingsExec<()> for Track {
    type At = When<LaneFrame>;
    type Output = Result<Option<Seq>, LiveError<PlayError, LaneProtocol>>;

    /// Settles the receipts the lane returned, then sends `change` for the
    /// lane to apply at `at`. A track without a lane sends nothing.
    fn exec_live(
        &mut self,
        change: TrackSettingsChange,
        at: Self::At,
        _cx: &mut (),
    ) -> Self::Output {
        let Some(lane) = &mut self.lane else {
            return Ok(None);
        };
        for receipt in lane.receipts() {
            if let Some(settled) = self.settings.settle(&receipt)
                && let Outcome::Rejected(rejection) = receipt.outcome()
            {
                warn!(?rejection, change = ?settled.change, "the lane refused a track settings change");
            }
        }
        self.settings
            .send(lane, at, change, LaneCommand::from)
            .map(Some)
    }
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU32;

    use kithara_audio::mock::TestPcmReader;
    use kithara_command::{ChannelConfig, Inbox, Step, channel};
    use kithara_platform::sync::Arc;
    use kithara_signal::{AudioSpec, SessionFrame};
    use kithara_test_fixtures::integration_fixtures::constant_half;
    use kithara_test_utils::kithara;
    use kithara_warp::{SpeedCurve, StretchKind};

    use super::*;
    use crate::{
        Resource,
        bridge::{DeckApplied, NodeInputs, SharedEq, slot_channels},
        consts::DECK_SLOT,
        test_pools::pools,
    };

    fn settings() -> TrackSettings {
        TrackSettings::builder()
            .speed(1.0)
            .keylock(false)
            .backend(StretchKind::default())
            .build()
    }

    fn resource(constant_half: &'static [u8]) -> Box<PlayerResource> {
        let spec = AudioSpec::new(2, NonZeroU32::new(44_100).expect("static sample rate"));
        let reader = TestPcmReader::with_pcm(spec, 0.01, constant_half);
        Box::new(
            PlayerResource::new(
                Resource::from_reader(reader, None),
                Arc::from("track"),
                &pools(),
            )
            .expect("player resource fits the test pool budget"),
        )
    }

    fn lane() -> (Sender<LaneProtocol>, Inbox<LaneProtocol>) {
        channel(ChannelConfig::builder().build())
    }

    /// The batches the deck takes in its next block, each as its parts.
    fn deck_batches(inputs: &mut NodeInputs) -> Vec<Vec<DeckPart>> {
        let mut batches = Vec::new();
        inputs.deck.run_block(SessionFrame::default(), 1, |step| {
            if let Step::Due(mut due) = step {
                batches.push(std::mem::take(due.commands_mut()));
                due.apply(DeckApplied::default());
            }
        });
        batches
    }

    /// The commands the lane takes in its next block, in order.
    fn lane_commands(inbox: &mut Inbox<LaneProtocol>) -> Vec<LaneCommand> {
        inbox.drain();
        let mut commands = Vec::new();
        while let Some(due) = inbox.next_due(LaneFrame(0), 1) {
            commands.extend(due.commands().iter().copied());
            due.apply(());
        }
        commands
    }

    fn load(resource: Box<PlayerResource>, lane: Option<Sender<LaneProtocol>>) -> TrackCommand {
        TrackCommand::Load {
            resource,
            lane,
            behind: None,
        }
    }

    #[kithara::test]
    fn a_track_loaded_behind_another_attaches_and_chains_in_one_batch(
        constant_half: &'static [u8],
    ) {
        let (mut inputs, mut deck) = slot_channels(SharedEq::new(0));
        let leading = TrackId::allocate();
        let mut track = Track::new(TrackId::allocate(), settings()).expect("valid settings");

        track
            .apply(
                TrackCommand::Load {
                    resource: resource(constant_half),
                    lane: None,
                    behind: Some(Behind {
                        track: leading,
                        epoch: 7,
                    }),
                },
                &mut Outbox::new(DECK_SLOT, &mut deck),
            )
            .expect("the deck has room");

        let batches = deck_batches(&mut inputs);
        let id = track.item_id();
        assert!(
            matches!(
                batches.as_slice(),
                [batch] if matches!(
                    batch.as_slice(),
                    [
                        DeckPart::Attach { item_id, .. },
                        DeckPart::Chain { from, to, epoch: 7 },
                    ] if *item_id == id && *from == leading && *to == id
                )
            ),
            "{batches:?}"
        );
    }

    #[kithara::test]
    fn every_part_a_track_sends_names_that_track(constant_half: &'static [u8]) {
        let (mut inputs, mut deck) = slot_channels(SharedEq::new(0));
        let mut track = Track::new(TrackId::allocate(), settings()).expect("valid settings");
        let mut out = Outbox::new(DECK_SLOT, &mut deck);
        for command in [
            load(resource(constant_half), None),
            TrackCommand::FadeIn {
                settings: CrossfadeSettings::default(),
                epoch: 1,
            },
            TrackCommand::Withdraw,
            TrackCommand::Release,
        ] {
            track.apply(command, &mut out).expect("the deck has room");
        }

        let parts: Vec<_> = deck_batches(&mut inputs).into_iter().flatten().collect();
        let id = track.item_id();
        assert!(
            matches!(
                parts.as_slice(),
                [
                    DeckPart::Attach { item_id: attached, .. },
                    DeckPart::Fade(TrackTransition::FadeIn { item_id: faded, .. }),
                    DeckPart::Withdraw { item_id: withdrawn },
                    DeckPart::Detach { item_id: detached },
                ] if [*attached, *faded, *withdrawn, *detached] == [id; 4]
            ),
            "{parts:?}"
        );
    }

    #[kithara::test]
    fn a_load_the_deck_refuses_leaves_the_lane_untouched(constant_half: &'static [u8]) {
        let (_inputs, mut deck) = slot_channels(SharedEq::new(0));
        while deck.send(DeckPart::StopAll).is_ok() {}
        let (sender, mut inbox) = lane();
        let mut track = Track::new(TrackId::allocate(), settings()).expect("valid settings");
        let mut out = Outbox::new(DECK_SLOT, &mut deck);

        let loaded = track.apply(load(resource(constant_half), Some(sender)), &mut out);
        track
            .apply(
                TrackCommand::Configure(TrackSettingsChange::Speed(2.0), When::Next),
                &mut out,
            )
            .expect("a valid speed");

        assert!(
            matches!(loaded, Err(PlayError::SlotChannelFull { slot }) if slot == DECK_SLOT),
            "{loaded:?}"
        );
        let commands = lane_commands(&mut inbox);
        assert!(commands.is_empty(), "{commands:?}");
    }

    #[kithara::test]
    fn a_loaded_track_sends_its_speed_and_each_change_to_its_lane(constant_half: &'static [u8]) {
        let (_inputs, mut deck) = slot_channels(SharedEq::new(0));
        let (sender, mut inbox) = lane();
        let mut track = Track::new(TrackId::allocate(), settings()).expect("valid settings");
        let mut out = Outbox::new(DECK_SLOT, &mut deck);
        track
            .apply(load(resource(constant_half), Some(sender)), &mut out)
            .expect("the deck has room");

        let sent = track
            .apply(
                TrackCommand::Configure(TrackSettingsChange::Speed(2.0), When::Next),
                &mut out,
            )
            .expect("a valid speed");

        assert!(sent.is_some());
        let commands = lane_commands(&mut inbox);
        assert!(
            matches!(
                commands.as_slice(),
                [
                    LaneCommand::SetSpeed(SpeedCurve::Constant(loaded)),
                    LaneCommand::SetSpeed(SpeedCurve::Constant(changed)),
                ] if *loaded == 1.0 && *changed == 2.0
            ),
            "{commands:?}"
        );
    }
}
