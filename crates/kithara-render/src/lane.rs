//! Commands a render stage executes at a frame of its own output.

use std::convert::Infallible;

use kithara_bufpool::HasPool;
use kithara_command::{Inbox, Protocol};
#[cfg(any(
    feature = "stretch-signalsmith",
    feature = "stretch-bungee",
    feature = "stretch-glide"
))]
use kithara_warp::StretchKind;
use kithara_warp::{SpeedCurve, WarpRenderer};

/// What a player and its render lane say to each other.
#[derive(Debug)]
pub enum LaneProtocol {}

/// A frame of a lane's output, counted from the first frame it emitted.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct LaneFrame(pub u64);

/// A command a lane executes at one frame of its output.
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub enum LaneCommand {
    /// Render from this frame on at the speed the curve holds.
    SetSpeed(SpeedCurve),
    /// Render from this frame on with a keylock engine, which keeps the
    /// pitch at any speed, when on and the backend has one.
    #[cfg(any(
        feature = "stretch-signalsmith",
        feature = "stretch-bungee",
        feature = "stretch-glide"
    ))]
    SetKeylock(bool),
    /// Render from this frame on with this backend's engine.
    #[cfg(any(
        feature = "stretch-signalsmith",
        feature = "stretch-bungee",
        feature = "stretch-glide"
    ))]
    SetBackend(StretchKind),
}

impl Protocol for LaneProtocol {
    type Applied = ();
    type Clock = LaneFrame;
    type Command = LaneCommand;
    type Refusal = Infallible;
    type Target = Infallible;

    fn frames_since(at: LaneFrame, start: LaneFrame) -> Option<u64> {
        at.0.checked_sub(start.0)
    }
}

/// The executing end of a lane: its batches and the output frame it reached.
pub(crate) struct Lane {
    inbox: Inbox<LaneProtocol>,
    cursor: LaneFrame,
}

impl Lane {
    pub(crate) fn new(inbox: Inbox<LaneProtocol>) -> Self {
        Self {
            inbox,
            cursor: LaneFrame::default(),
        }
    }

    /// Advances the cursor past `frames` emitted output frames.
    pub(crate) fn advance(&mut self, frames: usize) {
        let frames = u64::try_from(frames).unwrap_or(u64::MAX);
        self.cursor.0 = self.cursor.0.saturating_add(frames);
    }

    /// Applies to `warp`, in order, every batch due at the cursor, so the
    /// quantum that starts there renders under them.
    pub(crate) fn execute_due<S: HasPool<f32>>(&mut self, warp: &mut WarpRenderer<S>) {
        self.inbox.drain();
        while let Some(due) = self.inbox.next_due(self.cursor, 1) {
            let revision = due.seq().get();
            for command in due.commands() {
                match *command {
                    LaneCommand::SetSpeed(curve) => warp.set_speed(curve, revision),
                    #[cfg(any(
                        feature = "stretch-signalsmith",
                        feature = "stretch-bungee",
                        feature = "stretch-glide"
                    ))]
                    LaneCommand::SetKeylock(on) => warp.set_keylock(on),
                    #[cfg(any(
                        feature = "stretch-signalsmith",
                        feature = "stretch-bungee",
                        feature = "stretch-glide"
                    ))]
                    LaneCommand::SetBackend(kind) => warp.set_backend(kind),
                }
            }
            due.apply(());
        }
    }

    /// Output frames the next quantum may render before the next batch's frame.
    pub(crate) fn output_limit(&self) -> usize {
        self.inbox
            .frames_until_due(self.cursor)
            .map_or(usize::MAX, |frames| {
                usize::try_from(frames).unwrap_or(usize::MAX)
            })
    }
}
