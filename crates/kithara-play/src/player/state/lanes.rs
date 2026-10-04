//! The render lanes of the tracks a player's processor holds.

use kithara_command::{Batch, Sender, Seq, When};
use kithara_events::TrackId;
use kithara_platform::sync::Mutex;
use kithara_render::{LaneCommand, LaneProtocol};
use kithara_warp::SpeedCurve;
use tracing::warn;

/// Player end of the lane of every track its processor holds, oldest first.
#[derive(Default)]
pub(crate) struct TrackLanes(Mutex<Vec<(TrackId, Sender<LaneProtocol>)>>);

impl TrackLanes {
    /// Holds the lane of `item_id`, a track the processor just took, and
    /// sends it `speed` for its next block.
    pub(crate) fn load(&self, item_id: TrackId, mut lane: Sender<LaneProtocol>, speed: f32) {
        send_speed(&mut lane, speed);
        self.0.lock().push((item_id, lane));
    }

    /// Sends `speed` to every held lane for its next block and hands `sent`
    /// the number of each batch that went out.
    pub(crate) fn set_speed(&self, speed: f32, mut sent: impl FnMut(Seq)) {
        for (_, lane) in self.0.lock().iter_mut() {
            if let Some(seq) = send_speed(lane, speed) {
                sent(seq);
            }
        }
    }

    /// Releases the oldest lane of `item_id`, a track the processor unloaded.
    pub(crate) fn unload(&self, item_id: TrackId) {
        let mut lanes = self.0.lock();
        if let Some(index) = lanes.iter().position(|(held, _)| *held == item_id) {
            lanes.remove(index);
        }
    }
}

/// Settles the lane's receipts, then sends it `speed` for its next block.
fn send_speed(lane: &mut Sender<LaneProtocol>, speed: f32) -> Option<Seq> {
    lane.receipts().for_each(drop);
    let batch = Batch {
        basis: Vec::new(),
        commands: vec![LaneCommand::SetSpeed(SpeedCurve::Constant(speed))],
    };
    lane.send(When::Next, batch)
        .inspect_err(|error| warn!(%error, speed, "lane speed not sent"))
        .ok()
}
