#![forbid(unsafe_code)]

use kithara_events::Event;
use kithara_platform::time::Duration;

use super::{AbrMode, VariantIndex};

/// Reason attached to an ABR decision.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AbrReason {
    Initial,
    ManualOverride,
    UpSwitch,
    DownSwitch,
    MinInterval,
    NoEstimate,
    BufferTooLowForUpSwitch,
    /// Buffer ahead below urgent-threshold; force down-switch.
    UrgentDownSwitch,
    /// The active variant cannot deliver the segment the reader is blocked on
    /// (its in-flight fetch crossed the downloader's `soft_timeout`). Staying
    /// cannot grow the buffer, so ABR escapes to a bandwidth-viable variant —
    /// the buffer-too-low up-switch gate does not apply to a non-delivering
    /// variant.
    EscapeStalled,
    AlreadyOptimal,
    Locked,
}

/// Source of a bandwidth sample.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BandwidthSource {
    Network,
    Cache,
}

/// Duration shape for a variant.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VariantDuration {
    /// Single total duration (e.g. MP3, WAV).
    Total(Duration),
    /// Per-segment durations (HLS).
    Segmented(Vec<Duration>),
    /// Live / unknown-length stream.
    Unknown,
}

/// Progress snapshot pulled from a peer for buffer-aware ABR decisions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AbrProgressSnapshot {
    pub download_head_playback_time: Duration,
    pub reader_playback_time: Duration,
}

/// Variant metadata. Single source of truth across HLS parsing, ABR
/// scheduler, event payload, and UI surfaces. Replaces the historical
/// split between `AbrVariant` (bandwidth + duration) and a separate
/// `VariantInfo` for UI metadata.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VariantInfo {
    pub bandwidth_bps: Option<u64>,
    pub codecs: Option<String>,
    pub container: Option<String>,
    pub name: Option<String>,
    pub duration: VariantDuration,
    pub variant_index: VariantIndex,
}

/// Events emitted by the ABR controller for a single registered peer.
///
/// Published into the peer's track-scoped bus; root-level subscribers see
/// events for every track, track-scoped subscribers only their own.
#[derive(Clone, Debug, Event)]
pub enum AbrEvent {
    ThroughputSample {
        bytes_per_second: f64,
        source: BandwidthSource,
    },
    BandwidthEstimate {
        bps: u64,
    },
    BufferAhead {
        ahead: Option<Duration>,
    },
    VariantsRegistered {
        variants: Vec<VariantInfo>,
        initial: VariantIndex,
    },
    VariantApplied {
        from: VariantIndex,
        to: VariantIndex,
        reason: AbrReason,
    },
    ModeChanged {
        mode: AbrMode,
    },
    MaxBandwidthCapChanged {
        cap: Option<u64>,
    },
    Locked,
    Unlocked,
    DecisionSkipped {
        reason: AbrReason,
    },
    /// Reader did not advance within `incoherence_deadline` after a
    /// `VariantApplied` event. Signals a potential deadlock between the
    /// scheduler and the reader.
    Incoherence {
        description: String,
        elapsed: Duration,
    },
}

#[cfg(test)]
mod tests {
    use kithara_events::EventBus;
    use kithara_platform::time::Duration;
    use kithara_test_utils::kithara;

    use super::*;
    #[kithara::test]
    fn variant_duration_equality() {
        assert_eq!(
            VariantDuration::Total(Duration::from_secs(30)),
            VariantDuration::Total(Duration::from_secs(30)),
        );
        assert_eq!(
            VariantDuration::Segmented(vec![Duration::from_secs(10); 3]),
            VariantDuration::Segmented(vec![Duration::from_secs(10); 3]),
        );
        assert_eq!(VariantDuration::Unknown, VariantDuration::Unknown);
        assert_ne!(
            VariantDuration::Total(Duration::from_secs(5)),
            VariantDuration::Unknown,
        );
    }

    #[kithara::test]
    fn typed_channel_carries_locked() {
        let bus = EventBus::default();
        let mut rx = bus.subscribe::<AbrEvent>();
        bus.publish(AbrEvent::Locked);
        let event = rx.try_recv().expect("the event arrives").event;
        assert!(matches!(event, AbrEvent::Locked));
    }

    #[kithara::test]
    fn typed_channel_preserves_variant_applied() {
        let bus = EventBus::default();
        let mut rx = bus.subscribe::<AbrEvent>();
        bus.publish(AbrEvent::VariantApplied {
            from: VariantIndex::new(0),
            to: VariantIndex::new(1),
            reason: AbrReason::UpSwitch,
        });
        let event = rx.try_recv().expect("the event arrives").event;
        let AbrEvent::VariantApplied { from, to, reason } = event else {
            panic!("expected VariantApplied");
        };
        assert_eq!(from, VariantIndex::new(0));
        assert_eq!(to, VariantIndex::new(1));
        assert_eq!(reason, AbrReason::UpSwitch);
    }
}
