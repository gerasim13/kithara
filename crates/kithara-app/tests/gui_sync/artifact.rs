use kithara_test_utils::audio_artifact::write_audio_artifact_from_env;
use serde::Serialize;

use super::{
    PcmCapture, SelectedAnalysis, SyncCaptures, SyncExpectation,
    fixture::{ARTIFACT_CASE, ARTIFACT_DIR_ENV, PHASE_BUDGET_FRAMES, STAGGER_BEATS},
    offline::{BLOCK_FRAMES, CHANNELS, SAMPLE_RATE},
};

#[derive(Serialize)]
struct AppSyncManifest {
    case: &'static str,
    sample_rate: u32,
    channels: u16,
    block_frames: usize,
    phase_budget_frames: u64,
    stagger_beats: f64,
    stagger_output_frames: usize,
    selected_tracks: [SelectedTrackManifest; 2],
    captures: [CaptureManifest; 6],
}

#[derive(Serialize)]
struct SelectedTrackManifest {
    deck: &'static str,
    queue_index: usize,
    track_id: u64,
    analysis_source_frames: u64,
    analysis_bpm: f64,
}

#[derive(Serialize)]
struct CaptureManifest {
    role: &'static str,
    file: String,
    start_frame: i64,
    frames: usize,
}

#[derive(Clone, Copy)]
struct CaptureArtifact<'a> {
    role: &'static str,
    label: &'static str,
    capture: &'a PcmCapture,
}

impl CaptureArtifact<'_> {
    fn manifest(self) -> CaptureManifest {
        CaptureManifest {
            role: self.role,
            file: format!("{}.wav", self.label),
            start_frame: self.capture.start_frame,
            frames: self.capture.samples.len() / usize::from(CHANNELS),
        }
    }
}

pub(super) fn write_optional_artifacts(
    captures: &SyncCaptures,
    expected: &SyncExpectation,
    a_selected: &SelectedAnalysis,
    b_selected: &SelectedAnalysis,
) {
    let entries = [
        CaptureArtifact {
            role: "unsynced_deck_a",
            label: "unsynced-deck-a",
            capture: &captures.unsynced_deck_a,
        },
        CaptureArtifact {
            role: "unsynced_deck_b",
            label: "unsynced-deck-b",
            capture: &captures.unsynced_deck_b,
        },
        CaptureArtifact {
            role: "control_unsynced_mix",
            label: "control-unsynced-mix",
            capture: &captures.unsynced_mix,
        },
        CaptureArtifact {
            role: "synced_deck_a",
            label: "synced-deck-a",
            capture: &captures.synced_deck_a,
        },
        CaptureArtifact {
            role: "synced_deck_b",
            label: "synced-deck-b",
            capture: &captures.synced_deck_b,
        },
        CaptureArtifact {
            role: "synced_mix",
            label: "synced-mix",
            capture: &captures.synced_mix,
        },
    ];
    let manifest = AppSyncManifest {
        case: ARTIFACT_CASE,
        sample_rate: SAMPLE_RATE,
        channels: CHANNELS,
        block_frames: BLOCK_FRAMES,
        phase_budget_frames: PHASE_BUDGET_FRAMES,
        stagger_beats: STAGGER_BEATS,
        stagger_output_frames: expected.stagger_frames,
        selected_tracks: [
            selected_track("A", a_selected, expected.primary_bpm),
            selected_track("B", b_selected, expected.secondary_bpm),
        ],
        captures: entries.map(CaptureArtifact::manifest),
    };
    let audio = entries.map(|entry| (entry.label, entry.capture.samples.as_slice()));

    if let Err(error) = write_audio_artifact_from_env(
        ARTIFACT_DIR_ENV,
        ARTIFACT_CASE,
        SAMPLE_RATE,
        CHANNELS,
        &audio,
        &manifest,
    ) {
        panic!("configured app SYNC artifact bundle must be writable: {error}");
    }
}

fn selected_track(
    deck: &'static str,
    selected: &SelectedAnalysis,
    analysis_bpm: f64,
) -> SelectedTrackManifest {
    SelectedTrackManifest {
        deck,
        queue_index: selected.index,
        track_id: u64::from(selected.track_id),
        analysis_source_frames: selected.analysis.source_frames(),
        analysis_bpm,
    }
}
