use std::{
    collections::BTreeMap,
    env, fs,
    io::{self, Write},
    path::{Path, PathBuf},
};

pub use kithara::assets::{AssetReader, ReadSide};
use kithara::{
    assets::{
        AcquisitionResult, AssetResource, AssetScope, AssetSource, AssetStore, StorageBackend,
        WriteSide,
    },
    warp::BeatGridSnapshot,
};
use kithara_encode::EncodeConfig;
use kithara_record::{RecordingConfig, RecordingCore, assets::AssetPartSink};
pub use kithara_render::bridge::DeckSnapshot;
use serde::Serialize;
use serde_json::Value;

use crate::{
    artifact_timeline::ArtifactTimeline,
    bufpool_ext::{TestPools, pools},
    underrun_ledger::UnderrunLedger,
    usdt_trace,
};

const ARTIFACT_DIR_ENV: &str = "KITHARA_AUDIO_ARTIFACT_DIR";

pub type AudioArtifactRecording = RecordingCore<AssetPartSink<TestPools>>;

/// One opt-in disk `AssetStore` scope for related listening artifacts.
pub struct AudioArtifactSet {
    channels: u16,
    sample_rate: u32,
    case: String,
    root: PathBuf,
    scope: AssetScope<TestPools>,
}

/// One listening artifact per harness: PCM pushed by the render funnel,
/// markers stamped by control calls, published on drop.
#[derive(fieldwork::Fieldwork)]
#[fieldwork(opt_in, get, get_mut)]
pub struct AudioArtifactTap {
    set: AudioArtifactSet,
    recording: Option<AudioArtifactRecording>,
    markers: Vec<Marker>,
    frames: u64,
    channels: u16,
    #[field(get_mut = timeline)]
    timeline: ArtifactTimeline,
    source_grids: BTreeMap<u64, BeatGridSnapshot>,
    evidence: BTreeMap<String, Value>,
}

#[derive(Serialize)]
struct Marker {
    frame: u64,
    label: String,
}

impl AudioArtifactTap {
    /// Write required diagnostics beside the already-uploaded hang dumps.
    pub fn in_hang_dir(case: &str, sample_rate: u32, channels: u16) -> io::Result<Self> {
        let root = env::var_os("KITHARA_HANG_DUMP_DIR").map_or_else(env::temp_dir, PathBuf::from);
        let mut tap = Self::over(AudioArtifactSet::new(&root, case, sample_rate, channels)?)?;
        tap.evidence(
            "nextest",
            serde_json::json!({
                "run_id": env::var("NEXTEST_RUN_ID").ok(),
                "attempt_id": env::var("NEXTEST_ATTEMPT_ID").ok(),
                "attempt": env::var("NEXTEST_ATTEMPT").ok(),
                "binary_id": env::var("NEXTEST_BINARY_ID").ok(),
            }),
        );
        Ok(tap)
    }

    /// Preserve the live recorder's complete bounded history, including overflow status.
    pub fn record_probe_evidence(&mut self, capture_start: usize) {
        let (probes, truncated) = usdt_trace::recorded();
        let origin = probes.iter().map(|probe| probe.wall_time).min();
        let anchor = probes.get(capture_start..).and_then(|tail| {
            tail.iter().find(|probe| {
                probe.probe == "render" && matches!(
                    (probe.field("output_base"), probe.field("range_start"), probe.field("range_end")),
                    (Some(base), Some(start), Some(end)) if base != u64::MAX && end > start)
            })
        });
        let anchor = anchor.map(|probe| probe.fields().collect::<BTreeMap<_, _>>());
        let mut fired = BTreeMap::new();
        for probe in &probes {
            fired
                .entry(probe.probe)
                .or_insert_with(|| usdt_trace::count(probe.probe));
        }
        let events: Vec<_> = probes.iter().map(|probe| serde_json::json!({
            "probe": probe.probe, "target": probe.target,
            "file": probe.file, "line": probe.line, "thread": format!("{:?}", probe.thread),
            "elapsed_ns": origin.map(|origin| probe.wall_time.duration_since(origin).as_nanos()),
            "fields": probe.fields().collect::<BTreeMap<_, _>>(),
        })).collect();
        self.evidence(
            "probe_history",
            serde_json::json!({
                "events": events, "truncated": truncated, "empty": probes.is_empty(),
                "capture_start_index": capture_start, "session_output_anchor": anchor,
                "fired_per_probe": fired,
                "cap_per_probe": kithara_test_utils::test::usdt::MAX_EVENTS,
                "time_axis": "monotonic nanoseconds from earliest retained scoped probe",
            }),
        );
    }

    /// Build a tap only when `KITHARA_AUDIO_ARTIFACT_DIR` is set.
    pub fn from_env(case: &str, sample_rate: u32, channels: u16) -> io::Result<Option<Self>> {
        let Some(set) = AudioArtifactSet::from_env(case, sample_rate, channels)? else {
            return Ok(None);
        };
        Self::over(set).map(Some)
    }

    /// Open the tap over an artifact set that is already placed on disk.
    fn over(set: AudioArtifactSet) -> io::Result<Self> {
        let channels = set.channels;
        let recording = set.recording("output", None)?;
        Ok(Self {
            set,
            recording: Some(recording),
            markers: Vec::new(),
            frames: 0,
            channels,
            timeline: ArtifactTimeline::default(),
            source_grids: BTreeMap::new(),
            evidence: BTreeMap::new(),
        })
    }

    /// Build a tap in the opt-in directory, falling back to `fallback` when unset.
    ///
    /// A harness whose product is the recording itself must have somewhere to
    /// write in every ordinary run, not only under the opt-in variable.
    pub fn from_env_or(
        fallback: &Path,
        case: &str,
        sample_rate: u32,
        channels: u16,
    ) -> io::Result<Self> {
        if let Some(tap) = Self::from_env(case, sample_rate, channels)? {
            return Ok(tap);
        }
        Self::over(AudioArtifactSet::new(
            fallback,
            case,
            sample_rate,
            channels,
        )?)
    }

    pub fn push(&mut self, pcm: &[f32]) {
        if let Some(recording) = self.recording.as_mut() {
            recording
                .push(pcm)
                .unwrap_or_else(|error| panic!("listening tap push: {error}"));
            self.frames += (pcm.len() / usize::from(self.channels)) as u64;
        }
    }

    pub fn mark(&mut self, label: &str) {
        self.markers.push(Marker {
            frame: self.frames,
            label: label.to_owned(),
        });
        self.timeline
            .point("control", self.frames, "command", label);
    }

    pub fn source_grid(&mut self, track: u64, grid: BeatGridSnapshot) {
        self.source_grids.insert(track, grid);
    }

    /// Attach test-owned observed evidence to this artifact's manifest.
    pub fn evidence(&mut self, key: &str, value: Value) {
        self.evidence.insert(key.to_owned(), value);
    }

    /// Per-track ledger of every starved render this capture observed.
    ///
    /// Each entry carries the output interval the feeder silenced and the source
    /// frontier it stopped at, so a starved capture is read at its own frames
    /// instead of through a bare counter.
    #[must_use]
    pub fn underrun_ledger(&self) -> UnderrunLedger {
        UnderrunLedger::from_probes(&usdt_trace::events())
    }
}

/// Capture indices and the separately observed offline/session/source coordinates.
pub fn block_evidence(capture_frames: usize, output_cursor: u64, snapshot: &DeckSnapshot) -> Value {
    let slots: Vec<_> = snapshot
        .slots
        .iter()
        .map(|slot| {
            serde_json::json!({
                "binding": format!("{:?}", slot.binding),
                "item": slot.binding.map(|binding| binding.item().as_u64()),
                "state": format!("{:?}", slot.state),
                "session_frame": slot.mark.map(|mark| i64::from(mark.session)),
                "lane_frame": slot.mark.map(|mark| mark.lane.frame),
                "segment": slot.mark.map(|mark| mark.lane.segment.get()),
                "source": slot.source.map(|source| serde_json::json!({
                    "variant": source.variant_index, "source_revision": source.source_revision,
                    "render_revision": source.render_revision, "lane_frame": source.lane_frame,
                    "segment": source.segment.get(), "frame_offset": source.frame_offset,
                    "sample_rate": source.spec.sample_rate.get(), "channels": source.spec.channels,
                    "geometry": source.source_geometry().map(<[u64; 24]>::from),
                })),
            })
        })
        .collect();
    serde_json::json!({ "capture_frames": capture_frames,
        "offline_output_cursor": output_cursor, "slots": slots })
}

impl Drop for AudioArtifactTap {
    fn drop(&mut self) {
        let Some(recording) = self.recording.take() else {
            return;
        };
        let output = match AudioArtifactSet::finish(recording) {
            Ok(reader) => audio_artifact_path(&reader).ok(),
            Err(error) => {
                eprintln!("KITHARA_AUDIO_ARTIFACT output not published: {error}");
                None
            }
        };
        let (probes, probes_truncated) = usdt_trace::recorded();
        let underruns = UnderrunLedger::from_probes(&probes);
        self.timeline.record_probes(&probes);
        self.timeline.record_underruns(&underruns);
        for (&track, grid) in &self.source_grids {
            self.timeline.record_source_grid(track, grid);
        }
        let timeline = if self.timeline.is_empty() {
            None
        } else {
            self.set
                .write_bytes("timeline.svg", self.timeline.svg().as_bytes())
                .and_then(|reader| audio_artifact_path(&reader))
                .map_err(|error| {
                    eprintln!("KITHARA_AUDIO_ARTIFACT timeline not published: {error}");
                    error
                })
                .ok()
        };
        let manifest = serde_json::json!({
            "case": self.set.case,
            "frames": self.frames,
            "channels": self.channels,
            "sample_rate": self.set.sample_rate,
            "markers": self.markers,
            "output": output,
            "timeline": timeline,
            "timeline_events": self.timeline.events(),
            "underruns": underruns,
            "probes_truncated": probes_truncated,
            "evidence": self.evidence,
        });
        match self
            .set
            .write_manifest(&manifest)
            .and_then(|reader| audio_artifact_path(&reader))
        {
            Ok(path) => {
                if let Err(error) =
                    self.set
                        .append_index(&path, output.as_deref(), timeline.as_deref())
                {
                    eprintln!("KITHARA_AUDIO_ARTIFACT index not extended: {error}");
                }
                eprintln!("KITHARA_AUDIO_ARTIFACT manifest: {}", path.display());
            }
            Err(error) => eprintln!("KITHARA_AUDIO_ARTIFACT manifest not published: {error}"),
        }
    }
}

/// Return the running test's path as an artifact label.
#[must_use]
pub fn artifact_label() -> String {
    let name = std::thread::current()
        .name()
        .unwrap_or("unnamed")
        .to_owned();
    name.chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '_' {
                character
            } else {
                '-'
            }
        })
        .collect()
}

impl AudioArtifactSet {
    /// Build an artifact set only when the absolute opt-in directory is set.
    pub fn from_env(case: &str, sample_rate: u32, channels: u16) -> io::Result<Option<Self>> {
        let Some(root) = env::var_os(ARTIFACT_DIR_ENV).map(PathBuf::from) else {
            return Ok(None);
        };
        Self::new(&root, case, sample_rate, channels).map(Some)
    }

    /// Build an artifact set in the opt-in directory, or in `fallback` when unset.
    ///
    /// A recorder whose whole product is the artifact must run in every
    /// ordinary suite run, so it needs a directory unconditionally.
    pub fn from_env_or(
        fallback: &Path,
        case: &str,
        sample_rate: u32,
        channels: u16,
    ) -> io::Result<Self> {
        if let Some(set) = Self::from_env(case, sample_rate, channels)? {
            return Ok(set);
        }
        Self::new(fallback, case, sample_rate, channels)
    }

    /// Build an artifact set in an explicit absolute directory.
    pub fn new(root: &Path, case: &str, sample_rate: u32, channels: u16) -> io::Result<Self> {
        if !root.is_absolute() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("{ARTIFACT_DIR_ENV} must be an absolute path"),
            ));
        }
        if sample_rate == 0 || channels == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "audio artifact sample rate and channel count must be non-zero",
            ));
        }
        validate_label(case)?;
        fs::create_dir_all(root)?;
        let namespace = tempfile::Builder::new()
            .prefix("capture-")
            .tempdir_in(root)?
            .keep();
        let source = AssetSource::Local {
            path: namespace.join(case),
        };
        let store = AssetStore::builder(pools())
            .backend(StorageBackend::Disk { root: namespace })
            .build();
        let scope = store.scope::<Self>(&source).map_err(io::Error::other)?;
        Ok(Self {
            channels,
            sample_rate,
            case: case.to_owned(),
            root: root.to_path_buf(),
            scope,
        })
    }

    /// Open one WAV float32 transaction.
    pub fn recording(
        &self,
        label: &str,
        expected_frames: Option<u64>,
    ) -> io::Result<AudioArtifactRecording> {
        validate_label(label)?;
        let key = self.key(&format!("{label}.wav"))?;
        let sink = AssetPartSink::acquire(self.scope.store(), &key).map_err(io::Error::other)?;
        let config = RecordingConfig::builder()
            .encode(
                EncodeConfig::builder()
                    .sample_rate(self.sample_rate)
                    .channels(self.channels)
                    .build(),
            )
            .build();
        RecordingCore::new(&config, sink, expected_frames).map_err(io::Error::other)
    }

    /// Finish and atomically publish one audio artifact.
    pub fn finish(recording: AudioArtifactRecording) -> io::Result<AssetReader<TestPools>> {
        recording.finish().map_err(io::Error::other)
    }

    /// Serialize and atomically publish the set manifest.
    pub fn write_manifest<T: Serialize>(&self, manifest: &T) -> io::Result<AssetReader<TestPools>> {
        let bytes = serde_json::to_vec_pretty(manifest).map_err(io::Error::other)?;
        self.write_bytes("manifest.json", &bytes)
    }

    /// Append this artifact to the run-wide index so a reader can find the
    /// WAV and SVG of a named case without opening every hashed directory.
    pub fn append_index(
        &self,
        manifest: &Path,
        output: Option<&Path>,
        timeline: Option<&Path>,
    ) -> io::Result<()> {
        let entry = serde_json::json!({
            "case": self.case,
            "sample_rate": self.sample_rate,
            "channels": self.channels,
            "manifest": manifest,
            "output": output,
            "timeline": timeline,
        });
        let mut line = serde_json::to_vec(&entry).map_err(io::Error::other)?;
        line.push(b'\n');
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.root.join("index.jsonl"))?;
        file.write_all(&line)
    }

    /// Atomically publish one non-audio artifact in this set.
    pub fn write_bytes(&self, name: &str, bytes: &[u8]) -> io::Result<AssetReader<TestPools>> {
        validate_artifact_name(name)?;
        let key = self.key(name)?;
        let writer = match self
            .scope
            .store()
            .acquire_resource(&key, None)
            .map_err(io::Error::other)?
        {
            AcquisitionResult::Pending(writer) => writer,
            AcquisitionResult::Ready(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    "audio artifact manifest is already committed",
                ));
            }
            _ => return Err(io::Error::other("unexpected manifest acquisition phase")),
        };
        writer.write_at(0, bytes).map_err(io::Error::other)?;
        writer
            .commit(Some(
                u64::try_from(bytes.len()).map_err(|_| io::Error::other("manifest too large"))?,
            ))
            .map_err(io::Error::other)
    }

    fn key(&self, name: &str) -> io::Result<kithara::assets::ResourceKey> {
        self.scope
            .key(&AssetResource::Named {
                namespace: "artifacts".to_owned(),
                name: name.to_owned(),
            })
            .map_err(io::Error::other)
    }
}

/// Return the absolute disk path of a committed artifact.
pub fn audio_artifact_path(reader: &AssetReader<TestPools>) -> io::Result<PathBuf> {
    let path = reader
        .path()
        .ok_or_else(|| io::Error::other("disk audio artifact has no path"))?;
    if !path.is_absolute() {
        return Err(io::Error::other("audio artifact path is not absolute"));
    }
    Ok(path.to_path_buf())
}

/// Write listening WAV files and a manifest when the artifact directory is set.
pub fn write_audio_artifact<T: Serialize>(
    case: &str,
    sample_rate: u32,
    channels: u16,
    audio: &[(&str, &[f32])],
    manifest: &T,
) -> io::Result<Option<PathBuf>> {
    let Some(set) = AudioArtifactSet::from_env(case, sample_rate, channels)? else {
        return Ok(None);
    };
    for (label, samples) in audio {
        if !samples.len().is_multiple_of(usize::from(channels)) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "audio artifact samples must contain complete interleaved frames",
            ));
        }
        let frames = u64::try_from(samples.len() / usize::from(channels))
            .map_err(|_| io::Error::other("audio artifact frame count overflow"))?;
        let mut recording = set.recording(label, Some(frames))?;
        recording.push(samples).map_err(io::Error::other)?;
        let _ = AudioArtifactSet::finish(recording)?;
    }
    let manifest = set.write_manifest(manifest)?;
    let directory = audio_artifact_path(&manifest)?
        .parent()
        .ok_or_else(|| io::Error::other("audio artifact manifest has no parent"))?
        .to_path_buf();
    Ok(Some(directory))
}

fn validate_label(label: &str) -> io::Result<()> {
    if label.is_empty()
        || !label
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("artifact label must contain only ASCII letters, digits, '-' or '_': {label}"),
        ));
    }
    Ok(())
}

fn validate_artifact_name(name: &str) -> io::Result<()> {
    let mut parts = name.split('.');
    let stem = parts.next().unwrap_or_default();
    let extension = parts.next().unwrap_or_default();
    if parts.next().is_some() || extension.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("artifact name must contain one extension: {name}"),
        ));
    }
    validate_label(stem)?;
    validate_label(extension)
}

#[cfg(test)]
mod evidence_tests {
    use kithara_test_utils::{TestTempDir, kithara, temp_dir};

    use super::*;

    fn produce(_scope: &usdt_trace::Scope) -> [f32; 2] {
        tracing::event!(target: "evidence_probe", tracing::Level::TRACE,
            probe = "capture_identity", item = 11u64, variant = 2u64, source_revision = 3u64);
        tracing::event!(target: "evidence_probe", tracing::Level::TRACE,
            probe = "render", track_id = 11u64, output_base = 9_000u64,
            range_start = 0u64, range_end = 1u64);
        [0.25, -0.25]
    }

    #[kithara::test]
    fn artifact_keeps_identity_after_capture_returns(temp_dir: TestTempDir) {
        let scope = usdt_trace::scope();
        let pcm = produce(&scope);
        {
            let set =
                AudioArtifactSet::new(temp_dir.path(), "scope", 48_000, 2).expect("artifact set");
            let mut tap = AudioArtifactTap::over(set).expect("artifact tap");
            tap.record_probe_evidence(0);
            tap.push(&pcm);
        }
        drop(scope);
        let index = fs::read_to_string(temp_dir.path().join("index.jsonl")).expect("index");
        let index: Value = serde_json::from_str(index.lines().next().expect("artifact entry"))
            .expect("index JSON");
        let manifest: Value = serde_json::from_slice(
            &fs::read(index["manifest"].as_str().expect("manifest path")).expect("manifest"),
        )
        .expect("manifest JSON");
        let history = &manifest["evidence"]["probe_history"];
        assert_eq!(history["truncated"], false);
        assert_eq!(history["empty"], false);
        assert_eq!(history["session_output_anchor"]["output_base"], 9_000);
        assert!(
            history["events"]
                .as_array()
                .expect("probe history")
                .iter()
                .any(|event| event["probe"] == "capture_identity"
                    && event["fields"]["item"] == 11
                    && event["fields"]["variant"] == 2
                    && event["fields"]["source_revision"] == 3)
        );
        assert_eq!(manifest["frames"], 1);
    }
}
