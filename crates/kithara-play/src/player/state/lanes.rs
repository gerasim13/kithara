//! The render lanes of the tracks a player's processor holds.

use kithara_command::{Live, LiveError, Outcome, Sender, Seq, When};
use kithara_config::{ConfigOwner, LiveConfig};
use kithara_events::TrackId;
use kithara_platform::sync::Mutex;
use kithara_render::{LaneCommand, LaneFrame, LaneProtocol};
use tracing::warn;

use crate::{
    PlayError,
    player::config::{TrackSettings, TrackSettingsChange, TrackSettingsExec},
};

/// Player end of the lane of every track its processor holds, oldest first,
/// and the settings the next track it takes starts with.
pub(crate) struct TrackLanes(Mutex<Lanes>);

struct Lanes {
    next: TrackSettings,
    held: Vec<TrackLane>,
}

/// A held track's lane and its settings as the lane confirmed them.
struct TrackLane {
    item_id: TrackId,
    lane: Sender<LaneProtocol>,
    settings: Live<TrackSettings, LaneProtocol>,
}

impl TrackLanes {
    /// Lanes of a processor that holds no track yet; the first it takes
    /// starts with `next`.
    pub(crate) fn new(next: TrackSettings) -> Self {
        Self(Mutex::new(Lanes {
            next,
            held: Vec::new(),
        }))
    }

    /// The settings the next track the processor takes starts with.
    pub(crate) fn next(&self) -> TrackSettings {
        self.0.lock().next
    }

    /// Holds the lane of `item_id`, a track the processor just took, and
    /// sends it the next-track speed for its next block.
    pub(crate) fn load(&self, item_id: TrackId, lane: Sender<LaneProtocol>) {
        let mut lanes = self.0.lock();
        let settings = match Live::new(lanes.next) {
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
        lanes.held.push(track);
    }

    /// Makes `speed` the next tracks' speed, sends it to every held lane for
    /// its next block, and hands `sent` the number of each batch that went
    /// out.
    ///
    /// # Errors
    ///
    /// Returns the speed check's refusal; nothing changes then.
    pub(crate) fn set_speed(&self, speed: f32, mut sent: impl FnMut(Seq)) -> Result<(), PlayError> {
        let change = TrackSettings::check(TrackSettingsChange::Speed(speed))?;
        let mut lanes = self.0.lock();
        lanes.next.apply_change(change);
        for track in &mut lanes.held {
            match track.exec(change, When::Next, &mut ()) {
                Ok(seq) => sent(seq),
                Err(error) => warn!(%error, speed, "lane speed not sent"),
            }
        }
        drop(lanes);
        Ok(())
    }

    /// Releases the oldest lane of `item_id`, a track the processor unloaded.
    pub(crate) fn unload(&self, item_id: TrackId) {
        let held = &mut self.0.lock().held;
        if let Some(index) = held.iter().position(|track| track.item_id == item_id) {
            held.remove(index);
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
