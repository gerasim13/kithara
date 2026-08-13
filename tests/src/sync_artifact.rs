use std::{
    collections::BTreeMap,
    fs::{self, File},
    io::{self, BufWriter, Write},
    path::{Path, PathBuf},
};

use serde::Serialize;

use crate::cochlea::CochleaReport;

const ARTIFACT_DIR_ENV: &str = "KITHARA_SYNC_ARTIFACT_DIR";

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
    #[cfg(test)]
    directory: PathBuf,
    #[cfg(test)]
    reports: BTreeMap<String, CochleaReport>,
}

#[cfg(test)]
impl WrittenSyncArtifact {
    #[must_use]
    fn directory(&self) -> &Path {
        &self.directory
    }

    #[must_use]
    fn reports(&self) -> &BTreeMap<String, CochleaReport> {
        &self.reports
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
    metadata: &SyncArtifactMetadata,
    audio: &[ArtifactAudio<'_>],
) -> io::Result<Option<WrittenSyncArtifact>> {
    let Some(root) = std::env::var_os(ARTIFACT_DIR_ENV).map(PathBuf::from) else {
        return Ok(None);
    };
    write_sync_artifact_to(&root, metadata, audio).map(Some)
}

fn write_sync_artifact_to(
    root: &Path,
    metadata: &SyncArtifactMetadata,
    audio: &[ArtifactAudio<'_>],
) -> io::Result<WrittenSyncArtifact> {
    fs::create_dir_all(root)?;
    let directory = create_case_directory(root, &metadata.case)?;
    let mut manifest_audio = BTreeMap::new();
    #[cfg(test)]
    let mut reports = BTreeMap::new();
    for entry in audio {
        validate_audio(entry, metadata)?;
        let label = sanitize_component(entry.label);
        let filename = format!("{label}.wav");
        write_wav_f32(
            &directory.join(&filename),
            entry.samples,
            metadata.sample_rate,
            metadata.channels,
        )?;
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
    }

    let manifest = Manifest {
        metadata,
        audio: manifest_audio,
    };
    let manifest_bytes = serde_json::to_vec_pretty(&manifest).map_err(io::Error::other)?;
    fs::write(directory.join("manifest.json"), manifest_bytes)?;
    Ok(WrittenSyncArtifact {
        #[cfg(test)]
        directory,
        #[cfg(test)]
        reports,
    })
}

fn validate_audio(entry: &ArtifactAudio<'_>, metadata: &SyncArtifactMetadata) -> io::Result<()> {
    if metadata.channels == 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "sync artifact channel count must be non-zero",
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

fn create_case_directory(root: &Path, case: &str) -> io::Result<PathBuf> {
    let base = sanitize_component(case);
    for attempt in 0..1_000_u16 {
        let directory = root.join(format!("{base}-{}-{attempt}", std::process::id()));
        match fs::create_dir(&directory) {
            Ok(()) => return Ok(directory),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        format!("sync artifact directory space exhausted for case '{case}'"),
    ))
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

fn write_wav_f32(
    path: &Path,
    interleaved: &[f32],
    sample_rate: u32,
    channels: u16,
) -> io::Result<()> {
    let bytes_per_sample = 4_u32;
    let byte_rate = sample_rate
        .checked_mul(u32::from(channels))
        .and_then(|value| value.checked_mul(bytes_per_sample))
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "WAV byte rate overflow"))?;
    let block_align = channels
        .checked_mul(bytes_per_sample as u16)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "WAV block align overflow"))?;
    let data_bytes = u32::try_from(interleaved.len().saturating_mul(bytes_per_sample as usize))
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "WAV data is too large"))?;
    let riff_size = 36_u32
        .checked_add(data_bytes)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "WAV RIFF size overflow"))?;

    let mut writer = BufWriter::new(File::create(path)?);
    writer.write_all(b"RIFF")?;
    writer.write_all(&riff_size.to_le_bytes())?;
    writer.write_all(b"WAVE")?;
    writer.write_all(b"fmt ")?;
    writer.write_all(&16_u32.to_le_bytes())?;
    writer.write_all(&3_u16.to_le_bytes())?;
    writer.write_all(&channels.to_le_bytes())?;
    writer.write_all(&sample_rate.to_le_bytes())?;
    writer.write_all(&byte_rate.to_le_bytes())?;
    writer.write_all(&block_align.to_le_bytes())?;
    writer.write_all(&32_u16.to_le_bytes())?;
    writer.write_all(b"data")?;
    writer.write_all(&data_bytes.to_le_bytes())?;
    for sample in interleaved {
        writer.write_all(&sample.to_le_bytes())?;
    }
    writer.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
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
            &metadata,
            &[ArtifactAudio::new("final mix", &pcm)],
        )
        .expect("write artifact bundle");

        let wav = fs::read(written.directory.join("final-mix.wav")).expect("read WAV");
        assert_eq!(&wav[..4], b"RIFF");
        assert_eq!(&wav[8..12], b"WAVE");
        let manifest =
            fs::read_to_string(written.directory.join("manifest.json")).expect("read manifest");
        assert!(manifest.contains("local-hls"));
        assert!(manifest.contains("sync_frame_budget"));
        assert!(manifest.contains("example red verdict"));
        assert!(written.reports().contains_key("final mix"));
    }

    #[test]
    fn case_directories_never_overwrite_a_previous_audition() {
        let root = tempfile::tempdir().expect("artifact temp dir");
        let metadata = SyncArtifactMetadata::new("same-case", 48_000, 2, 512);
        let pcm = vec![0.0; 1_024];
        let first =
            write_sync_artifact_to(root.path(), &metadata, &[ArtifactAudio::new("mix", &pcm)])
                .expect("first artifact");
        let second =
            write_sync_artifact_to(root.path(), &metadata, &[ArtifactAudio::new("mix", &pcm)])
                .expect("second artifact");

        assert_ne!(first.directory(), second.directory());
        assert!(first.directory().join("mix.wav").is_file());
        assert!(second.directory().join("mix.wav").is_file());
    }
}
