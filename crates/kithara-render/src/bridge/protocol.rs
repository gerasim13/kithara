use std::fmt;

use kithara_audio::DecodeErrorKind;
use kithara_command::{Protocol, Target};
use kithara_effects::{GainDb, eq::EqLayout};
use kithara_signal::SessionFrame;

use super::DeckMixSettingsChange;
use crate::{CrossfadeSettings, rt::track::PlayerResource};

/// Types a deck's mixer speaks: its parts, its slots, its clock and its answers.
///
/// Every part names the slot its owner assigned; a batch's basis lists the slots whose time it
/// shifts, so a batch computed before another one moved a slot comes back stale. A receipt
/// carries the batch's parts back as they applied: a part that took a resource in comes back as
/// nothing, one that let a resource go comes back as [`DeckPart::Released`].
#[derive(Debug)]
pub enum DeckProtocol {}

impl Protocol for DeckProtocol {
    type Applied = DeckApplied;
    type Clock = SessionFrame;
    type Command = DeckPart;
    type Refusal = DeckRefusal;
    type Target = Slot;

    fn frames_since(at: SessionFrame, start: SessionFrame) -> Option<u64> {
        at.frames_since(start)
    }
}

/// One slot of a deck's mixer, assigned by the deck's owner; the mixer has as many as its
/// config names.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Slot(u16);

impl Slot {
    #[must_use]
    pub const fn new(index: u16) -> Self {
        Self(index)
    }

    #[must_use]
    pub const fn get(self) -> u16 {
        self.0
    }
}

impl Target for Slot {
    fn index(self) -> usize {
        usize::from(self.0)
    }
}

/// Why the mixer refused a batch; it refuses before applying any part, so nothing of the batch
/// applied.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeckRefusal {
    /// A track was attached to a slot that holds one.
    Occupied { slot: Slot },
    /// A part named a slot that holds no track.
    Empty { slot: Slot },
}

/// What a deck reports of a batch it applied.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct DeckApplied {
    /// The media position, in seconds, the slot the batch stopped stood at on the frame the
    /// batch applied on.
    pub stopped_at: Option<f64>,
}

/// The envelope a slot enters with on `Start` or leaves with on `Stop`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Fade {
    /// The mixer's declick ramp.
    Declick,
    /// A crossfade half, by [`CrossfadeSettings::gains`].
    Crossfade(CrossfadeSettings),
}

/// Which half of [`CrossfadeSettings::gains`] a [`DeckPart::Fade`] follows.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FadeDir {
    In,
    Out,
}

/// One change a deck's mixer applies on its audio thread, on the slot its owner names.
pub enum DeckPart {
    /// Put a track's PCM consumer into an empty slot, stopped.
    Attach {
        slot: Slot,
        pcm: Box<PlayerResource>,
    },
    /// Take the track out of a slot; its consumer comes back in the receipt.
    Detach { slot: Slot },
    /// Let a slot sound from this frame on, entering with `fade`.
    Start { slot: Slot, fade: Fade },
    /// Take a slot out from this frame on with `fade`; once silent it is not read, so it holds
    /// its position. The position it stopped at comes back in the receipt.
    Stop { slot: Slot, fade: Fade },
    /// Ramp a slot's envelope from its gain on this frame along one half of `settings`; an
    /// envelope that reaches silence stops the slot and reports [`DeckEvent::Faded`].
    Fade {
        slot: Slot,
        settings: CrossfadeSettings,
        dir: FadeDir,
    },
    /// Start `to` on the frame after `from`'s last. The batch is judged and applied on that
    /// frame: one that shifted `from` in between leaves it stale.
    Chain { from: Slot, to: Slot },
    /// Change how loud the whole deck sounds from this frame on.
    Mix(DeckMixSettingsChange),
    /// Change the deck's equaliser from this frame on; a displaced layout comes back in the
    /// receipt.
    Eq(DeckEqChange),
    /// Re-base a slot's track on a seek its owner began off the audio thread.
    Seek {
        slot: Slot,
        seconds: f64,
        seek_epoch: u64,
    },
    /// Update the media seconds a slot consumes per output second.
    Rate { slot: Slot, rate: f32 },
    /// Swap a slot's consumer on this frame: the old one's next `evict_fade` frames play out of
    /// the slot's tail ramped down to silence, the new one takes the slot in the old one's
    /// transport state, and the old one comes back in the receipt.
    Replace {
        slot: Slot,
        pcm: Box<PlayerResource>,
    },
    /// What a part that carried or displaced a resource comes back as in the receipt: the
    /// consumer a `Detach` or `Replace` let go of, or the layout an `Eq` change displaced. The
    /// mixer never takes it as a command.
    Released(Released),
}

/// A resource the mixer let go of, returned in the receipt so it drops off the audio thread.
pub enum Released {
    /// The consumer `slot` held before a `Detach` or a `Replace` of it.
    Pcm {
        slot: Slot,
        pcm: Box<PlayerResource>,
    },
    /// The layout an `Eq` change displaced.
    Eq(Box<EqLayout>),
}

/// One change to a deck's equaliser.
#[derive(Debug)]
pub enum DeckEqChange {
    /// Ramp `band` of the layout the deck was last handed to `gain`.
    Gain { band: usize, gain: GainDb },
    /// Cross over to a layout built off the audio thread.
    Layout(Box<EqLayout>),
}

/// What the mixer tells its owner of a slot, apart from the receipts: the owner reacts in its
/// own loop.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeckEvent {
    /// The slot's track played to its end marker, or its source failed.
    Ended { slot: Slot, at: SessionFrame },
    /// The slot's envelope reached silence; the slot stopped.
    Faded { slot: Slot, at: SessionFrame },
    /// The slot's consumer had `frames` fewer frames than the block asked for.
    Underrun {
        slot: Slot,
        at: SessionFrame,
        frames: u32,
    },
}

impl fmt::Debug for DeckPart {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Attach { slot, pcm } => f
                .debug_struct("Attach")
                .field("slot", slot)
                .field("src", pcm.src())
                .finish(),
            Self::Replace { slot, pcm } => f
                .debug_struct("Replace")
                .field("slot", slot)
                .field("src", pcm.src())
                .finish(),
            Self::Detach { slot } => f.debug_struct("Detach").field("slot", slot).finish(),
            Self::Start { slot, fade } => f
                .debug_struct("Start")
                .field("slot", slot)
                .field("fade", fade)
                .finish(),
            Self::Stop { slot, fade } => f
                .debug_struct("Stop")
                .field("slot", slot)
                .field("fade", fade)
                .finish(),
            Self::Fade {
                slot,
                settings,
                dir,
            } => f
                .debug_struct("Fade")
                .field("slot", slot)
                .field("settings", settings)
                .field("dir", dir)
                .finish(),
            Self::Chain { from, to } => f
                .debug_struct("Chain")
                .field("from", from)
                .field("to", to)
                .finish(),
            Self::Mix(change) => f.debug_tuple("Mix").field(change).finish(),
            Self::Eq(change) => f.debug_tuple("Eq").field(change).finish(),
            Self::Seek {
                slot,
                seconds,
                seek_epoch,
            } => f
                .debug_struct("Seek")
                .field("slot", slot)
                .field("seconds", seconds)
                .field("seek_epoch", seek_epoch)
                .finish(),
            Self::Rate { slot, rate } => f
                .debug_struct("Rate")
                .field("slot", slot)
                .field("rate", rate)
                .finish(),
            Self::Released(Released::Pcm { slot, pcm }) => f
                .debug_struct("Released")
                .field("slot", slot)
                .field("src", pcm.src())
                .finish(),
            Self::Released(Released::Eq(_)) => f.write_str("Released(Eq)"),
        }
    }
}

/// Where a slot's track stands.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum SlotState {
    /// No track.
    #[default]
    Empty,
    /// A track that is not read: never started, stopped, or faded out.
    Stopped,
    /// A track that sounds.
    Playing,
    /// A track that played to its end marker, or whose source failed.
    Ended,
}

/// Which fault ended a track before its natural end.
///
/// The classification is `Copy` because it is raised on the audio thread,
/// which cannot allocate: the decoder's own error is reduced to its kind at
/// the read that returned it and travels as a code from there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaybackFault {
    /// The decoder or source returned an error mid-stream.
    Decode(DecodeErrorKind),
    /// The render context's output rate disagreed with the track's.
    OutputRateMismatch,
    /// The render context could not supply the requested output range.
    OutputRangeUnavailable,
}

impl fmt::Display for PlaybackFault {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Decode(kind) => write!(f, "decode error ({kind:?})"),
            Self::OutputRateMismatch => f.write_str("output sample-rate mismatch"),
            Self::OutputRangeUnavailable => f.write_str("render context has no output range"),
        }
    }
}
