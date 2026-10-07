use kithara_command::{Mailbox, Postbox};
use kithara_events::TrackId;

use super::SelectTransition;
use crate::{
    EqBandConfig, InterruptionKind, PlayError, Resource, SelectionPlayback, SuccessorLink,
};

/// Where a [`PlayerControl`](super::PlayerControl) posts to its player; each
/// post is answered applied or refused with the player's error.
pub(crate) type PlayerPostbox = Postbox<PlayerCommand, PlayError>;

/// What a player drains its posts from.
pub(crate) type PlayerMailbox = Mailbox<PlayerCommand, PlayError>;

/// What a handle asks a player to do. The executor that holds the player runs
/// the commands one at a time, in the order they were posted, and answers
/// each one; a decorator hands its player's commands on unchanged.
pub enum PlayerCommand {
    /// Attach `resource` to the deck as `item`, ahead of the current item and
    /// joined to it by `link`.
    ArmNext {
        item: TrackId,
        resource: Resource,
        link: SuccessorLink,
    },
    /// Drop the armed successor without committing it.
    UnarmNext,
    /// The platform interrupted, or released, the audio output.
    NotifyInterruption(InterruptionKind),
    Pause,
    Play,
    /// Drop every track the deck holds and release its slot.
    RemoveAllItems,
    ResetEq,
    /// Seek within the current item, in seconds.
    Seek(f64),
    /// Make `item` current with the configured crossfade.
    Select {
        item: TrackId,
        resource: Option<Resource>,
        playback: SelectionPlayback,
    },
    /// Make `item` current with this transition's crossfade.
    SelectWithCrossfade {
        item: TrackId,
        resource: Option<Resource>,
        transition: SelectTransition,
    },
    SetCrossfadeDuration(f32),
    SetDefaultRate(f32),
    SetEqGain {
        band: usize,
        gain_db: f32,
    },
    SetEqLayout(Vec<EqBandConfig>),
    SetLevel(f32),
    SetMuted(bool),
    SetRate(f32),
    SetVolume(f32),
    /// Handle what the deck reported since the last tick.
    Tick,
    Close,
}
