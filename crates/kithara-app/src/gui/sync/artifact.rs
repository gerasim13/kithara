use std::{
    env,
    fmt::Write as _,
    fs::{self, File},
    io::{self, BufWriter, Error, ErrorKind, Write},
    mem::size_of,
    path::{Path, PathBuf},
    process::id,
};

use super::{
    PcmCapture, SelectedAnalysis, SyncCaptures, SyncExpectation,
    fixture::{ARTIFACT_CASE, ARTIFACT_DIR_ENV, PHASE_BUDGET_FRAMES, STAGGER_BEATS},
    offline::{BLOCK_FRAMES, CHANNELS, SAMPLE_RATE},
};

struct Consts;

impl Consts {
    const ARTIFACT_DIRECTORY_ATTEMPTS: u16 = 1_000;
    const ARTIFACT_NAME_CAPACITY: usize = 64;
    const WAV_BITS_PER_SAMPLE: u16 = 32;
    const WAV_FMT_CHUNK_BYTES: u32 = 16;
    const WAV_FORMAT_IEEE_FLOAT: u16 = 3;
    const WAV_RIFF_HEADER_BYTES: u32 = 36;
}

pub(super) fn write_optional_artifacts(
    captures: &SyncCaptures,
    expected: &SyncExpectation,
    a_selected: &SelectedAnalysis,
    b_selected: &SelectedAnalysis,
) {
    let Some(root) = env::var_os(ARTIFACT_DIR_ENV).map(PathBuf::from) else {
        return;
    };
    if let Err(error) = write_artifact_bundle(&root, captures, expected, a_selected, b_selected) {
        panic!("configured app SYNC artifact bundle must be writable: {error}");
    }
}

fn write_artifact_bundle(
    root: &Path,
    captures: &SyncCaptures,
    expected: &SyncExpectation,
    a_selected: &SelectedAnalysis,
    b_selected: &SelectedAnalysis,
) -> io::Result<PathBuf> {
    fs::create_dir_all(root)?;
    let directory = create_artifact_directory(root)?;
    let audio = [
        ("unsynced-deck-a.wav", &captures.unsynced_deck_a),
        ("unsynced-deck-b.wav", &captures.unsynced_deck_b),
        ("control-unsynced-mix.wav", &captures.unsynced_mix),
        ("synced-deck-a.wav", &captures.synced_deck_a),
        ("synced-deck-b.wav", &captures.synced_deck_b),
        ("synced-mix.wav", &captures.synced_mix),
    ];
    for (name, capture) in audio {
        write_float_wav(&directory.join(name), &capture.samples)?;
    }
    write_artifact_manifest(
        &directory.join("manifest.json"),
        captures,
        expected,
        a_selected,
        b_selected,
    )?;
    Ok(directory)
}

fn create_artifact_directory(root: &Path) -> io::Result<PathBuf> {
    let mut directory = root.to_path_buf();
    let mut name = String::with_capacity(Consts::ARTIFACT_NAME_CAPACITY);
    for attempt in 0..Consts::ARTIFACT_DIRECTORY_ATTEMPTS {
        name.clear();
        write!(&mut name, "{ARTIFACT_CASE}-{}-{attempt}", id()).map_err(Error::other)?;
        directory.push(&name);
        match fs::create_dir(&directory) {
            Ok(()) => return Ok(directory),
            Err(error) if error.kind() == ErrorKind::AlreadyExists => {
                if !directory.pop() {
                    return Err(Error::other(
                        "sync artifact candidate has no removable file name",
                    ));
                }
            }
            Err(error) => return Err(error),
        }
    }
    Err(Error::new(
        ErrorKind::AlreadyExists,
        format!("sync artifact directory space exhausted for case '{ARTIFACT_CASE}'"),
    ))
}

fn write_artifact_manifest(
    path: &Path,
    captures: &SyncCaptures,
    expected: &SyncExpectation,
    a_selected: &SelectedAnalysis,
    b_selected: &SelectedAnalysis,
) -> io::Result<()> {
    let mut writer = BufWriter::new(File::create(path)?);
    writeln!(writer, "{{")?;
    writeln!(writer, "  \"schema_version\": 1,")?;
    writeln!(writer, "  \"case\": \"{ARTIFACT_CASE}\",")?;
    writeln!(writer, "  \"sample_rate\": {SAMPLE_RATE},")?;
    writeln!(writer, "  \"channels\": {CHANNELS},")?;
    writeln!(writer, "  \"block_frames\": {BLOCK_FRAMES},")?;
    writeln!(writer, "  \"phase_budget_frames\": {PHASE_BUDGET_FRAMES},")?;
    writeln!(
        writer,
        "  \"phase_contract\": {{\"unsynced_spread\":\"greater_than_budget\",\"synced_spread\":\"at_most_budget\"}},"
    )?;
    writeln!(writer, "  \"stagger_beats\": {STAGGER_BEATS},")?;
    writeln!(
        writer,
        "  \"stagger_output_frames\": {},",
        expected.stagger_frames
    )?;
    writeln!(writer, "  \"selected_tracks\": [")?;
    write_selected_manifest_entry(&mut writer, "A", a_selected, expected.primary_bpm, true)?;
    write_selected_manifest_entry(&mut writer, "B", b_selected, expected.secondary_bpm, false)?;
    writeln!(writer, "  ],")?;
    writeln!(writer, "  \"audio\": [")?;
    write_audio_manifest_entry(
        &mut writer,
        "unsynced_deck_a",
        "unsynced-deck-a.wav",
        &captures.unsynced_deck_a,
        true,
    )?;
    write_audio_manifest_entry(
        &mut writer,
        "unsynced_deck_b",
        "unsynced-deck-b.wav",
        &captures.unsynced_deck_b,
        true,
    )?;
    write_audio_manifest_entry(
        &mut writer,
        "control_unsynced_mix",
        "control-unsynced-mix.wav",
        &captures.unsynced_mix,
        true,
    )?;
    write_audio_manifest_entry(
        &mut writer,
        "synced_deck_a",
        "synced-deck-a.wav",
        &captures.synced_deck_a,
        true,
    )?;
    write_audio_manifest_entry(
        &mut writer,
        "synced_deck_b",
        "synced-deck-b.wav",
        &captures.synced_deck_b,
        true,
    )?;
    write_audio_manifest_entry(
        &mut writer,
        "synced_mix",
        "synced-mix.wav",
        &captures.synced_mix,
        false,
    )?;
    writeln!(writer, "  ]")?;
    writeln!(writer, "}}")?;
    writer.flush()
}

fn write_selected_manifest_entry(
    writer: &mut impl Write,
    deck: &str,
    selected: &SelectedAnalysis,
    bpm: f64,
    comma: bool,
) -> io::Result<()> {
    let suffix = if comma { "," } else { "" };
    writeln!(
        writer,
        "    {{\"deck\":\"{deck}\",\"queue_index\":{},\"track_id\":{},\"analysis_source_frames\":{},\"analysis_bpm\":{bpm:.9}}}{suffix}",
        selected.index,
        u64::from(selected.track_id),
        selected.analysis.source_frames(),
    )
}

fn write_audio_manifest_entry(
    writer: &mut impl Write,
    role: &str,
    file: &str,
    capture: &PcmCapture,
    comma: bool,
) -> io::Result<()> {
    let suffix = if comma { "," } else { "" };
    let frames = capture.samples.len() / usize::from(CHANNELS);
    writeln!(
        writer,
        "    {{\"role\":\"{role}\",\"file\":\"{file}\",\"start_frame\":{},\"frames\":{frames}}}{suffix}",
        capture.start_frame,
    )
}

fn write_float_wav(path: &Path, samples: &[f32]) -> io::Result<()> {
    let bytes_per_sample = size_of::<f32>();
    let data_bytes = u32::try_from(samples.len().saturating_mul(bytes_per_sample))
        .map_err(|_| Error::other("artifact WAV is too large"))?;
    let bytes_per_sample_u32 =
        u32::try_from(bytes_per_sample).expect("invariant: sample width fits u32");
    let bytes_per_sample_u16 =
        u16::try_from(bytes_per_sample).expect("invariant: sample width fits u16");
    let byte_rate = SAMPLE_RATE * u32::from(CHANNELS) * bytes_per_sample_u32;
    let block_align = CHANNELS * bytes_per_sample_u16;
    let mut writer = BufWriter::new(File::create(path)?);
    writer.write_all(b"RIFF")?;
    writer.write_all(&(Consts::WAV_RIFF_HEADER_BYTES + data_bytes).to_le_bytes())?;
    writer.write_all(b"WAVEfmt ")?;
    writer.write_all(&Consts::WAV_FMT_CHUNK_BYTES.to_le_bytes())?;
    writer.write_all(&Consts::WAV_FORMAT_IEEE_FLOAT.to_le_bytes())?;
    writer.write_all(&CHANNELS.to_le_bytes())?;
    writer.write_all(&SAMPLE_RATE.to_le_bytes())?;
    writer.write_all(&byte_rate.to_le_bytes())?;
    writer.write_all(&block_align.to_le_bytes())?;
    writer.write_all(&Consts::WAV_BITS_PER_SAMPLE.to_le_bytes())?;
    writer.write_all(b"data")?;
    writer.write_all(&data_bytes.to_le_bytes())?;
    for sample in samples {
        writer.write_all(&sample.to_le_bytes())?;
    }
    writer.flush()
}
