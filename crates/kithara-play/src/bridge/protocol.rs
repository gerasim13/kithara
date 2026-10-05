use std::{convert::Infallible, fmt};

use kithara_audio::DecodeErrorKind;
use kithara_command::Protocol;
use kithara_events::TrackId;
use kithara_platform::sync::Arc;
use kithara_signal::SessionFrame;

use super::DeckMixSettingsChange;
use crate::rt::track::PlayerResource;

/// Types a deck's audio thread speaks: its parts, its clock and its answers.
///
/// Batches name no target and the deck refuses none, so every batch due
/// inside a block applies.
#[derive(Debug)]
pub enum DeckProtocol {}

impl Protocol for DeckProtocol {
    type Applied = DeckApplied;
    type Clock = SessionFrame;
    type Command = DeckPart;
    type Refusal = Infallible;
    type Target = Infallible;

    fn frames_since(at: SessionFrame, start: SessionFrame) -> Option<u64> {
        at.frames_since(start)
    }
}

/// What a deck reports of a batch it applied.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct DeckApplied {
    /// The media position, in seconds, the track the batch stopped stood at on the frame the
    /// batch applied on.
    pub stopped_at: Option<f64>,
}

/// One change a deck applies on its audio thread.
pub enum DeckPart {
    /// Put a track into the deck.
    Attach {
        resource: Box<PlayerResource>,
        item_id: TrackId,
    },
    /// Take a track out of the deck by its queue-item identity.
    Detach { item_id: TrackId },
    /// Take a track out only while it is still preloading. The audio thread may
    /// have stitched it in at the end of the leading track before reading
    /// this; a promoted track keeps playing.
    Withdraw { item_id: TrackId },
    /// Take every track out of the deck and reset the position/duration
    /// snapshot to zero. Sent when the queue is explicitly cleared.
    Clear,
    /// Start a track's fade in or out.
    Fade(TrackTransition),
    /// Seek active tracks to the given position in seconds.
    Seek { seconds: f64, seek_epoch: u64 },
    /// Let a held track sound from this frame on, ramped in over the deck's declick.
    Start { item_id: TrackId },
    /// Ramp a held track out from this frame on; once silent it is not read, so it holds its
    /// position until a later `Start`.
    Stop { item_id: TrackId },
    /// Start every held track, and every track attached after, from this frame on.
    StartAll,
    /// Stop every held track, and attach later tracks stopped, from this frame on.
    StopAll,
    /// Change how loud the deck sounds from this frame on.
    Mix(DeckMixSettingsChange),
    /// Update the fade duration.
    SetFadeDuration(f32),
    /// Update the prefetch lead time.
    SetPrefetchDuration(f32),
    /// Update the media seconds every track consumes per output second.
    SetRate(f32),
}

impl fmt::Debug for DeckPart {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Attach { item_id, resource } => f
                .debug_struct("Attach")
                .field("item_id", item_id)
                .field("src", resource.src())
                .finish_non_exhaustive(),
            Self::Detach { item_id } => f.debug_struct("Detach").field("item_id", item_id).finish(),
            Self::Withdraw { item_id } => f
                .debug_struct("Withdraw")
                .field("item_id", item_id)
                .finish(),
            Self::Clear => f.write_str("Clear"),
            Self::Fade(t) => f.debug_tuple("Fade").field(t).finish(),
            Self::Seek {
                seconds,
                seek_epoch,
            } => f
                .debug_struct("Seek")
                .field("seconds", seconds)
                .field("seek_epoch", seek_epoch)
                .finish(),
            Self::Start { item_id } => f.debug_struct("Start").field("item_id", item_id).finish(),
            Self::Stop { item_id } => f.debug_struct("Stop").field("item_id", item_id).finish(),
            Self::StartAll => f.write_str("StartAll"),
            Self::StopAll => f.write_str("StopAll"),
            Self::Mix(change) => f.debug_tuple("Mix").field(change).finish(),
            Self::SetFadeDuration(d) => f.debug_tuple("SetFadeDuration").field(d).finish(),
            Self::SetPrefetchDuration(d) => f.debug_tuple("SetPrefetchDuration").field(d).finish(),
            Self::SetRate(rate) => f.debug_tuple("SetRate").field(rate).finish(),
        }
    }
}

/// State machine for a single track's lifecycle.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum TrackState {
    /// Track is loaded but not yet playing.
    #[default]
    Preloading,
    /// Track is actively playing at full volume.
    Playing,
    /// Track is fading in (volume ramping up).
    FadingIn,
    /// Track is fading out (volume ramping down).
    FadingOut,
    /// Track has finished playback (EOF or stopped).
    Finished,
}

impl TrackState {
    /// Whether the track is the "leading" track (playing or fading in).
    pub(crate) const fn is_leading(self) -> bool {
        matches!(self, Self::Playing | Self::FadingIn)
    }

    /// Whether the track is producing audible audio.
    pub(crate) const fn is_playing(self) -> bool {
        matches!(self, Self::Playing | Self::FadingIn | Self::FadingOut)
    }
}

/// Transition command for a track.
#[derive(Debug, Clone, PartialEq)]
pub enum TrackTransition {
    /// Start fading in the track with the given queue-item identity.
    FadeIn {
        item_id: TrackId,
        settings: crate::CrossfadeSettings,
        /// Leading epoch the control side published for this item.
        epoch: u64,
    },
    /// Start fading out the track with the given queue-item identity.
    FadeOut {
        item_id: TrackId,
        settings: crate::CrossfadeSettings,
    },
}

/// Which fault ended a track before its natural end.
///
/// The classification is `Copy` because it is raised on the audio thread,
/// which cannot allocate: the decoder's own error is reduced to its kind at
/// the read that returned it and travels as a code from there. Carrying it
/// is what lets a consumer tell a decode fault from an output rate the
/// render context disagrees with, or from a range that context could not
/// supply -- three different defects that otherwise reach the queue as one
/// indistinguishable "the engine failed". It is named for the render path
/// because the decode pipeline already owns its own failure classification.
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrackPlaybackStopReason {
    /// Playback stopped because the track naturally reached EOF.
    Eof,
    /// Playback stopped because the track was explicitly stopped or interrupted.
    Stop,
    /// Playback stopped because the underlying decoder / source reported
    /// a non-recoverable error mid-stream. Distinct from `Eof`: the
    /// track did NOT play to its natural end. Queue consumers must
    /// treat this as a track-failed signal, NOT as an auto-advance
    /// trigger, and the payload says which fault it was.
    Failed(PlaybackFault),
}

#[derive(Debug, Clone)]
pub enum PlayerNotification {
    /// A track was successfully loaded into the processor arena.
    Loaded { src: Arc<str> },
    /// A track was removed from the processor arena.
    Unloaded { src: Arc<str>, item_id: TrackId },
    /// A track started audible playback (fade-in completed or `play()`).
    PlaybackStarted { src: Arc<str>, item_id: TrackId },
    /// A track stopped playback. `src` and `item_id` are read by the
    /// player to construct the `ItemRole` on `ItemDidPlayToEnd`.
    PlaybackStopped {
        src: Arc<str>,
        item_id: TrackId,
        reason: TrackPlaybackStopReason,
        /// The slot seek epoch the track sat at when this stop was minted.
        /// An `Eof` stop is delivered only while this is still the published
        /// epoch: a newer published seek revives the track, and the end the
        /// user left behind must not reach the queue. `Stop` and `Failed`
        /// carry the epoch too but are never fenced on it.
        seek_epoch: u64,
    },
    /// The next track should be loaded into the processor (position
    /// reached the prefetch lead window before EOF). Preload-only —
    /// handlers must not start fade-in or change the current item.
    Requested,
    /// Time to hand over to the next track (position reached
    /// `crossfade_duration + block_seconds` before EOF, or natural EOF was
    /// observed). Handlers may activate the already-preloaded successor;
    /// when `crossfade_duration == 0` the activation defers to the
    /// playback-stopped path instead.
    ///
    /// `src` and `item_id` name the track that is running out, for the same
    /// reason [`PlaybackStopped`](Self::PlaybackStopped) carries them: the
    /// request is minted by one track while any number of others render, and
    /// a consumer that has already advanced past it must be able to tell
    /// that this handover is not about the track it now holds.
    HandoverRequested { src: Arc<str>, item_id: TrackId },
    /// A track change occurred: old track fading out, new track fading in.
    Changed { src: Arc<str> },
    /// A track started fading in.
    FadingIn { src: Arc<str> },
    /// A track started fading out.
    FadingOut { src: Arc<str> },
    /// The processor applied a new effective live playback rate.
    RateChanged { rate: f32 },
}

impl PlayerNotification {
    /// Returns the track src for variants that carry it.
    ///
    /// Used by the offline test harness (`take_notification_kinds`) and by
    /// tracing call-sites that need to discriminate between concurrent
    /// tracks beyond what the variant tag alone can express.
    #[must_use]
    pub const fn src(&self) -> Option<&Arc<str>> {
        match self {
            Self::Loaded { src }
            | Self::Unloaded { src, .. }
            | Self::Changed { src }
            | Self::FadingIn { src }
            | Self::FadingOut { src }
            | Self::HandoverRequested { src, .. }
            | Self::PlaybackStopped { src, .. } => Some(src),
            Self::PlaybackStarted { .. } | Self::Requested | Self::RateChanged { .. } => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use kithara_platform::sync::Arc;
    use kithara_test_utils::kithara;

    use super::*;

    #[kithara::test]
    #[case(PlayerNotification::Loaded { src: Arc::from("a.mp3") }, "Loaded")]
    #[case(PlayerNotification::Requested, "Requested")]
    #[case(
        PlayerNotification::HandoverRequested {
            src: Arc::from("ending.mp3"),
            item_id: TrackId::allocate(),
        },
        "HandoverRequested"
    )]
    #[case(PlayerNotification::FadingIn { src: Arc::from("a.mp3") }, "FadingIn")]
    #[case(PlayerNotification::RateChanged { rate: 1.25 }, "RateChanged")]
    #[case(
        PlayerNotification::PlaybackStopped {
            src: Arc::from("ended.mp3"),
            item_id: TrackId::allocate(),
            reason: TrackPlaybackStopReason::Eof,
            seek_epoch: 0,
        },
        "PlaybackStopped"
    )]
    fn notification_debug_format(#[case] n: PlayerNotification, #[case] variant_name: &str) {
        let debug = format!("{n:?}");
        assert!(debug.contains(variant_name));
    }

    #[kithara::test]
    fn notification_clone() {
        let n = PlayerNotification::PlaybackStopped {
            src: Arc::from("ended.mp3"),
            item_id: TrackId::allocate(),
            reason: TrackPlaybackStopReason::Stop,
            seek_epoch: 0,
        };
        let cloned = n.clone();
        assert!(matches!(
            &n,
            PlayerNotification::PlaybackStopped { src, .. } if &**src == "ended.mp3"
        ));
        assert!(matches!(
            cloned,
            PlayerNotification::PlaybackStopped { ref src, .. } if &**src == "ended.mp3"
        ));
    }

    #[kithara::test]
    fn notification_changed_carries_src() {
        let n = PlayerNotification::Changed {
            src: Arc::from("next.mp3"),
        };
        let PlayerNotification::Changed { src } = n else {
            panic!("expected Changed");
        };
        assert_eq!(&*src, "next.mp3");
    }
}
