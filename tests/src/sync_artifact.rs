use std::{
    collections::BTreeMap,
    io,
    path::{Path, PathBuf},
};

#[cfg(test)]
use kithara::assets::AssetReader;
use kithara::bufpool::BytePool;
use serde::Serialize;

#[cfg(test)]
use crate::audio_artifact::WrittenAudioArtifact;
use crate::{
    audio_artifact::{AUDIO_ARTIFACT_DIR_ENV, write_audio_artifact_to},
    cochlea::CochleaReport,
};

#[derive(Clone, Debug, Serialize)]
#[non_exhaustive]
pub struct ArtifactSource {
    deck: String,
    media: String,
    analysis_key: Option<String>,
}

impl ArtifactSource {
    #[must_use]
    pub fn new(deck: impl Into<String>, media: impl Into<String>) -> Self {
        Self {
            deck: deck.into(),
            media: media.into(),
            analysis_key: None,
        }
    }

    #[must_use]
    pub fn with_analysis_key(mut self, key: impl Into<String>) -> Self {
        self.analysis_key = Some(key.into());
        self
    }
}

#[derive(Clone, Copy, Debug, Serialize)]
#[non_exhaustive]
pub struct ArtifactFrame {
    frame: u64,
    event: &'static str,
}

impl ArtifactFrame {
    #[must_use]
    pub const fn new(frame: u64, event: &'static str) -> Self {
        Self { frame, event }
    }
}

#[derive(Clone, Debug, Serialize)]
#[non_exhaustive]
pub struct SyncArtifactMetadata {
    case: String,
    sample_rate: u32,
    channels: u16,
    quantum_frames: usize,
    sources: Vec<ArtifactSource>,
    library_seed: Option<u64>,
    operation: Option<String>,
    frame_ledger: Vec<ArtifactFrame>,
    states: BTreeMap<String, String>,
    thresholds: BTreeMap<String, f64>,
    failures: Vec<String>,
}

impl SyncArtifactMetadata {
    #[must_use]
    pub fn new(
        case: impl Into<String>,
        sample_rate: u32,
        channels: u16,
        quantum_frames: usize,
    ) -> Self {
        Self {
            case: case.into(),
            sample_rate,
            channels,
            quantum_frames,
            sources: Vec::new(),
            library_seed: None,
            operation: None,
            frame_ledger: Vec::new(),
            states: BTreeMap::new(),
            thresholds: BTreeMap::new(),
            failures: Vec::new(),
        }
    }

    pub fn add_source(&mut self, source: ArtifactSource) {
        self.sources.push(source);
    }

    pub fn set_library_seed(&mut self, seed: u64) {
        self.library_seed = Some(seed);
    }

    pub fn set_operation(&mut self, operation: impl Into<String>) {
        self.operation = Some(operation.into());
    }

    pub fn add_frame(&mut self, frame: ArtifactFrame) {
        self.frame_ledger.push(frame);
    }

    pub fn add_state(&mut self, name: impl Into<String>, value: impl Into<String>) {
        self.states.insert(name.into(), value.into());
    }

    pub fn add_threshold(&mut self, name: impl Into<String>, value: f64) {
        self.thresholds.insert(name.into(), value);
    }

    pub fn add_failure(&mut self, failure: impl Into<String>) {
        self.failures.push(failure.into());
    }

    pub fn add_failures(&mut self, failures: impl IntoIterator<Item = String>) {
        self.failures.extend(failures);
    }
}

#[derive(Clone, Copy)]
#[non_exhaustive]
pub struct ArtifactAudio<'a> {
    label: &'a str,
    samples: &'a [f32],
}

impl<'a> ArtifactAudio<'a> {
    #[must_use]
    pub const fn new(label: &'a str, samples: &'a [f32]) -> Self {
        Self { label, samples }
    }
}

#[derive(Debug)]
#[non_exhaustive]
pub struct WrittenSyncArtifact {
    directory: PathBuf,
    #[cfg(test)]
    artifact: WrittenAudioArtifact,
    #[cfg(test)]
    reports: BTreeMap<String, CochleaReport>,
}

impl WrittenSyncArtifact {
    /// Return the exact directory containing the listening WAV files and manifest.
    #[must_use]
    pub fn directory(&self) -> &Path {
        &self.directory
    }

    #[cfg(test)]
    #[must_use]
    fn reports(&self) -> &BTreeMap<String, CochleaReport> {
        &self.reports
    }

    #[cfg(test)]
    #[must_use]
    fn reader(&self, name: &str) -> Option<&AssetReader> {
        self.artifact.reader(name)
    }
}

#[derive(Serialize)]
struct Manifest<'a> {
    metadata: &'a SyncArtifactMetadata,
    audio: BTreeMap<String, ManifestAudio>,
}

#[derive(Serialize)]
struct ManifestAudio {
    file: String,
    frames: usize,
    cochlea: CochleaReport,
}

pub fn write_sync_artifact(
    pool: &BytePool,
    metadata: &SyncArtifactMetadata,
    audio: &[ArtifactAudio<'_>],
) -> io::Result<Option<WrittenSyncArtifact>> {
    let Some(root) = std::env::var_os(AUDIO_ARTIFACT_DIR_ENV).map(PathBuf::from) else {
        return Ok(None);
    };
    write_sync_artifact_to(&root, pool, metadata, audio).map(Some)
}

fn write_sync_artifact_to(
    root: &Path,
    pool: &BytePool,
    metadata: &SyncArtifactMetadata,
    audio: &[ArtifactAudio<'_>],
) -> io::Result<WrittenSyncArtifact> {
    let mut manifest_audio = BTreeMap::new();
    let mut artifact_audio = Vec::with_capacity(audio.len());
    #[cfg(test)]
    let mut reports = BTreeMap::new();
    for entry in audio {
        validate_audio(entry, metadata)?;
        let label = sanitize_component(entry.label);
        let filename = format!("{label}.wav");
        let report = CochleaReport::measure(entry.samples, metadata.channels, metadata.sample_rate);
        let frames = entry.samples.len() / usize::from(metadata.channels);
        #[cfg(test)]
        reports.insert(entry.label.to_owned(), report.clone());
        manifest_audio.insert(
            entry.label.to_owned(),
            ManifestAudio {
                file: filename,
                frames,
                cochlea: report,
            },
        );
        artifact_audio.push((label, entry.samples));
    }

    let manifest = Manifest {
        metadata,
        audio: manifest_audio,
    };
    let artifact_audio = artifact_audio
        .iter()
        .map(|(label, samples)| (label.as_str(), *samples))
        .collect::<Vec<_>>();
    let artifact = write_audio_artifact_to(
        root,
        pool,
        &sanitize_component(&metadata.case),
        metadata.sample_rate,
        metadata.channels,
        &artifact_audio,
        &manifest,
    )?;
    let directory = artifact.directory().to_path_buf();
    Ok(WrittenSyncArtifact {
        directory,
        #[cfg(test)]
        artifact,
        #[cfg(test)]
        reports,
    })
}

fn validate_audio(entry: &ArtifactAudio<'_>, metadata: &SyncArtifactMetadata) -> io::Result<()> {
    if metadata.sample_rate == 0 || metadata.channels == 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "sync artifact sample rate and channel count must be non-zero",
        ));
    }
    if entry.label.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "sync artifact audio label must not be empty",
        ));
    }
    if !entry
        .samples
        .len()
        .is_multiple_of(usize::from(metadata.channels))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "sync artifact '{}' has {} samples for {} channels",
                entry.label,
                entry.samples.len(),
                metadata.channels,
            ),
        ));
    }
    Ok(())
}

fn sanitize_component(component: &str) -> String {
    let sanitized = component
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
                character.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>();
    let sanitized = sanitized.trim_matches('-');
    if sanitized.is_empty() {
        "sync-artifact".to_owned()
    } else {
        sanitized.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use kithara_test_utils::kithara;

    use super::*;
    use crate::kithara::assets::ReadSide;

    #[kithara::test]
    fn artifact_bundle_writes_listenable_pcm_and_manifest() {
        let root = tempfile::tempdir().expect("artifact temp dir");
        let mut metadata = SyncArtifactMetadata::new("HLS / sync", 48_000, 2, 512);
        metadata.add_source(ArtifactSource::new("A", "local-hls"));
        metadata.add_frame(ArtifactFrame::new(1_024, "deck-b-start"));
        metadata.add_threshold("sync_frame_budget", 512.0);
        metadata.add_failure("example red verdict");
        let pcm = (0..48_000)
            .flat_map(|frame| {
                let sample = (std::f32::consts::TAU * 880.0 * frame as f32 / 48_000.0).sin() * 0.25;
                [sample, sample]
            })
            .collect::<Vec<_>>();

        let written = write_sync_artifact_to(
            root.path(),
            &BytePool::default(),
            &metadata,
            &[ArtifactAudio::new("final mix", &pcm)],
        )
        .expect("write artifact bundle");

        let mut wav = Vec::new();
        written
            .reader("final-mix.wav")
            .expect("WAV reader")
            .read_into(&mut wav)
            .expect("read WAV through AssetStore");
        assert_eq!(&wav[..4], b"RIFF");
        assert_eq!(&wav[8..12], b"WAVE");
        let mut manifest = Vec::new();
        written
            .reader("manifest.json")
            .expect("manifest reader")
            .read_into(&mut manifest)
            .expect("read manifest through AssetStore");
        let manifest = String::from_utf8(manifest).expect("manifest UTF-8");
        assert!(manifest.contains("local-hls"));
        assert!(manifest.contains("sync_frame_budget"));
        assert!(manifest.contains("example red verdict"));
        assert!(written.reports().contains_key("final mix"));
    }

    #[kithara::test]
    fn case_directories_never_overwrite_a_previous_audition() {
        let root = tempfile::tempdir().expect("artifact temp dir");
        let metadata = SyncArtifactMetadata::new("same-case", 48_000, 2, 512);
        let pcm = vec![0.0; 1_024];
        let first = write_sync_artifact_to(
            root.path(),
            &BytePool::default(),
            &metadata,
            &[ArtifactAudio::new("mix", &pcm)],
        )
        .expect("first artifact");
        let second = write_sync_artifact_to(
            root.path(),
            &BytePool::default(),
            &metadata,
            &[ArtifactAudio::new("mix", &pcm)],
        )
        .expect("second artifact");

        assert_ne!(first.directory(), second.directory());
        assert!(first.reader("mix.wav").is_some());
        assert!(second.reader("mix.wav").is_some());
    }
}
