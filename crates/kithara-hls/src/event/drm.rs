#![forbid(unsafe_code)]

use kithara_events::Event;

/// Stage where a DRM key fetch failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum KeyFailureStage {
    Network,
    BodyCollect,
    Processor,
    Missing,
}

/// Source that produced the final DRM key bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum KeySource {
    Network,
    DiskCache,
    MemCache,
}

/// Events emitted during DRM key fetch / decrypt lifecycle.
#[derive(Debug, Clone, PartialEq, Eq, Event)]
#[non_exhaustive]
pub enum DrmEvent {
    KeyFetchFailed {
        key_host: Option<String>,
        stage: KeyFailureStage,
        detail: String,
    },
    KeyAcquired {
        key_host: Option<String>,
        source: KeySource,
        bytes: usize,
        latency_ms: Option<u64>,
    },
    SegmentDecryptFailed {
        variant: u32,
        segment_index: u32,
        detail: String,
    },
}
