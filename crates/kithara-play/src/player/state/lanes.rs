//! The render lanes of the tracks a player's processor holds.

use kithara_command::{Live, LiveError, Outcome, Sender, Seq, When};
use kithara_config::ConfigOwner;
use kithara_events::TrackId;
use kithara_platform::sync::Mutex;
use kithara_render::{LaneCommand, LaneFrame, LaneProtocol};
use tracing::warn;

use crate::{
    PlayError,
    player::config::{TrackSettings, TrackSettingsChange, TrackSettingsExec},
};

/// Player end of the lane of every track its processor holds, oldest first.
#[derive(Default)]
pub(crate) struct TrackLanes(Mutex<Vec<TrackLane>>);

/// A held track's lane and its settings as the lane confirmed them.
struct TrackLane {
    item_id: TrackId,
    lane: Sender<LaneProtocol>,
    settings: Live<TrackSettings, LaneProtocol>,
}

impl TrackLanes {
    /// Holds the lane of `item_id`, a track the processor just took with
    /// `settings`, and sends it their speed for its next block.
    pub(crate) fn load(
        &self,
        item_id: TrackId,
        lane: Sender<LaneProtocol>,
        settings: TrackSettings,
    ) {
        let settings = match Live::new(settings) {
            Ok(settings) => settings,
            Err(error) => {
                warn!(%error, "track settings refused; the lane is not held");
                return;
            }
        };
        let mut track = TrackLane {
            item_id,
            lane,
            settings,
        };
        let speed = track.settings.config().speed();
        if let Err(error) = track.exec(TrackSettingsChange::Speed(speed), When::Next, &mut ()) {
            warn!(%error, speed, "lane speed not sent");
        }
        self.0.lock().push(track);
    }

    /// Sends `speed` to every held lane for its next block and hands `sent`
    /// the number of each batch that went out.
    pub(crate) fn set_speed(&self, speed: f32, mut sent: impl FnMut(Seq)) {
        for track in self.0.lock().iter_mut() {
            match track.exec(TrackSettingsChange::Speed(speed), When::Next, &mut ()) {
                Ok(seq) => sent(seq),
                Err(error) => warn!(%error, speed, "lane speed not sent"),
            }
        }
    }

    /// Releases the oldest lane of `item_id`, a track the processor unloaded.
    pub(crate) fn unload(&self, item_id: TrackId) {
        let mut lanes = self.0.lock();
        if let Some(index) = lanes.iter().position(|track| track.item_id == item_id) {
            lanes.remove(index);
        }
    }
}

impl TrackSettingsExec<()> for TrackLane {
    type At = When<LaneFrame>;
    type Output = Result<Seq, LiveError<PlayError, LaneProtocol>>;

    /// Settles the receipts the lane returned, then sends `change` for the
    /// lane to apply at `at`.
    fn exec_live(
        &mut self,
        change: TrackSettingsChange,
        at: Self::At,
        _cx: &mut (),
    ) -> Self::Output {
        for receipt in self.lane.receipts() {
            if let Some(settled) = self.settings.settle(&receipt)
                && let Outcome::Rejected(rejection) = receipt.outcome()
            {
                warn!(?rejection, change = ?settled.change, "the lane refused a track settings change");
            }
        }
        self.settings
            .send(&mut self.lane, at, change, LaneCommand::from)
    }
}
