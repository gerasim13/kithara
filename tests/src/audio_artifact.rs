use std::{
    collections::BTreeSet,
    env,
    fs::{self, OpenOptions},
    io::{self, BufWriter, Write},
    mem::{size_of, size_of_val},
    path::{Path, PathBuf},
};

use serde::Serialize;
use uuid::Uuid;

pub(crate) const AUDIO_ARTIFACT_DIR_ENV: &str = "KITHARA_AUDIO_ARTIFACT_DIR";

const WAV_BATCH_SAMPLES: usize = 4 * 1024;

/// Write listening WAV files when the artifact directory is set.
/// Returns `Ok(None)` when unset.
///
/// Each label becomes the exact filename `<label>.wav` inside a unique case
/// directory under `KITHARA_AUDIO_ARTIFACT_DIR`.
///
/// # Errors
/// Returns an error for an invalid directory, case, label, or audio payload, or
/// when a filesystem write fails.
pub(crate) fn write_audio_dump(
    case: &str,
    sample_rate: u32,
    channels: u16,
    audio: &[(&str, &[f32])],
) -> io::Result<Option<PathBuf>> {
    let Some(root) = env::var_os(AUDIO_ARTIFACT_DIR_ENV).map(PathBuf::from) else {
        return Ok(None);
    };
    write_audio_dump_to(&root, case, sample_rate, channels, audio).map(Some)
}

/// Write listening WAV files and a manifest when the artifact directory is set.
/// Returns `Ok(None)` when unset.
///
/// # Errors
/// Returns an error for invalid audio, serialization, or filesystem failure.
pub fn write_audio_artifact<T: Serialize>(
    case: &str,
    sample_rate: u32,
    channels: u16,
    audio: &[(&str, &[f32])],
    manifest: &T,
) -> io::Result<Option<PathBuf>> {
    let Some(root) = env::var_os(AUDIO_ARTIFACT_DIR_ENV).map(PathBuf::from) else {
        return Ok(None);
    };
    write_audio_artifact_to(&root, case, sample_rate, channels, audio, manifest).map(Some)
}

pub(crate) fn write_audio_artifact_to<T: Serialize>(
    root: &Path,
    case: &str,
    sample_rate: u32,
    channels: u16,
    audio: &[(&str, &[f32])],
    manifest: &T,
) -> io::Result<PathBuf> {
    let directory = write_audio_dump_to(root, case, sample_rate, channels, audio)?;
    let manifest_path = directory.join("manifest.json");
    let manifest_file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(manifest_path)?;
    let mut writer = BufWriter::new(manifest_file);
    serde_json::to_writer_pretty(&mut writer, manifest).map_err(io::Error::other)?;
    writer.write_all(b"\n")?;
    writer.flush()?;
    Ok(directory)
}

pub(crate) fn write_audio_dump_to(
    root: &Path,
    case: &str,
    sample_rate: u32,
    channels: u16,
    audio: &[(&str, &[f32])],
) -> io::Result<PathBuf> {
    if !root.is_absolute() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{AUDIO_ARTIFACT_DIR_ENV} must be an absolute path"),
        ));
    }
    validate_label(case)?;
    let mut labels = BTreeSet::new();
    for (label, samples) in audio {
        validate_label(label)?;
        validate_audio(samples, sample_rate, channels)?;
        if !labels.insert(*label) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("duplicate artifact audio label: {label}"),
            ));
        }
    }

    fs::create_dir_all(root)?;
    let directory = root.join(format!("{case}-{}", Uuid::new_v4()));
    fs::create_dir(&directory)?;

    for (label, samples) in audio {
        let filename = format!("{label}.wav");
        write_float_wav(&directory.join(filename), samples, sample_rate, channels)?;
    }

    Ok(directory)
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

fn validate_audio(samples: &[f32], sample_rate: u32, channels: u16) -> io::Result<()> {
    if sample_rate == 0 || channels == 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "WAV sample rate and channel count must be non-zero",
        ));
    }
    if !samples.len().is_multiple_of(usize::from(channels)) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "WAV samples must contain complete interleaved frames",
        ));
    }
    Ok(())
}

fn write_float_wav(
    path: &Path,
    samples: &[f32],
    sample_rate: u32,
    channels: u16,
) -> io::Result<()> {
    let header = wav_header(samples.len(), sample_rate, channels)?;
    let file = OpenOptions::new().write(true).create_new(true).open(path)?;
    let mut writer = BufWriter::with_capacity(WAV_BATCH_SAMPLES * size_of::<f32>(), file);
    writer.write_all(&header)?;

    let mut encoded = [0_u8; WAV_BATCH_SAMPLES * size_of::<f32>()];
    for batch in samples.chunks(WAV_BATCH_SAMPLES) {
        for (sample, bytes) in batch.iter().zip(encoded.chunks_exact_mut(size_of::<f32>())) {
            bytes.copy_from_slice(&sample.to_le_bytes());
        }
        let byte_len = size_of_val(batch);
        writer.write_all(&encoded[..byte_len])?;
    }
    writer.flush()
}

fn wav_header(samples: usize, sample_rate: u32, channels: u16) -> io::Result<[u8; 44]> {
    if sample_rate == 0 || channels == 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "WAV sample rate and channel count must be non-zero",
        ));
    }
    let bytes_per_sample = u16::try_from(size_of::<f32>())
        .map_err(|_| io::Error::other("WAV sample size exceeds u16"))?;
    let data_bytes = u32::try_from(
        samples
            .checked_mul(usize::from(bytes_per_sample))
            .ok_or_else(|| io::Error::other("WAV data size overflow"))?,
    )
    .map_err(|_| io::Error::other("WAV data exceeds RIFF limit"))?;
    let riff_size = 36_u32
        .checked_add(data_bytes)
        .ok_or_else(|| io::Error::other("WAV RIFF size overflow"))?;
    let byte_rate = sample_rate
        .checked_mul(u32::from(channels))
        .and_then(|value| value.checked_mul(u32::from(bytes_per_sample)))
        .ok_or_else(|| io::Error::other("WAV byte rate overflow"))?;
    let block_align = channels
        .checked_mul(bytes_per_sample)
        .ok_or_else(|| io::Error::other("WAV block alignment overflow"))?;

    let mut header = [0_u8; 44];
    header[0..4].copy_from_slice(b"RIFF");
    header[4..8].copy_from_slice(&riff_size.to_le_bytes());
    header[8..16].copy_from_slice(b"WAVEfmt ");
    header[16..20].copy_from_slice(&16_u32.to_le_bytes());
    header[20..22].copy_from_slice(&3_u16.to_le_bytes());
    header[22..24].copy_from_slice(&channels.to_le_bytes());
    header[24..28].copy_from_slice(&sample_rate.to_le_bytes());
    header[28..32].copy_from_slice(&byte_rate.to_le_bytes());
    header[32..34].copy_from_slice(&block_align.to_le_bytes());
    header[34..36].copy_from_slice(&32_u16.to_le_bytes());
    header[36..40].copy_from_slice(b"data");
    header[40..44].copy_from_slice(&data_bytes.to_le_bytes());
    Ok(header)
}

#[cfg(test)]
mod tests {
    use serde::Serialize;

    use super::*;
    use crate::kithara;

    #[derive(Serialize)]
    struct Manifest {
        case: &'static str,
    }

    #[kithara::test(native, flash(false))]
    fn artifact_bytes_roundtrip_through_direct_filesystem_files() {
        let root = tempfile::tempdir().expect("artifact temp dir");
        let samples = [0.25, -0.25, 0.5, -0.5];
        let directory = write_audio_artifact_to(
            root.path(),
            "filesystem-roundtrip",
            48_000,
            2,
            &[("mix", samples.as_slice())],
            &Manifest { case: "roundtrip" },
        )
        .expect("write artifact bundle");

        let wav = fs::read(directory.join("mix.wav")).expect("read direct WAV file");
        assert_eq!(&wav[..4], b"RIFF");
        assert_eq!(&wav[8..12], b"WAVE");
        assert_eq!(wav.len(), 44 + samples.len() * size_of::<f32>());

        let manifest =
            fs::read_to_string(directory.join("manifest.json")).expect("read direct manifest file");
        assert!(manifest.contains("roundtrip"));
        assert!(directory.starts_with(root.path()));
    }

    #[kithara::test(native, flash(false))]
    fn wav_payload_survives_a_buffered_batch_boundary() {
        let root = tempfile::tempdir().expect("artifact temp dir");
        let samples = (0..WAV_BATCH_SAMPLES + 2)
            .map(|index| {
                f32::from_bits(0x3e80_0000 + u32::try_from(index).expect("sample index fits u32"))
            })
            .collect::<Vec<_>>();
        let directory = write_audio_artifact_to(
            root.path(),
            "batch-boundary",
            48_000,
            1,
            &[("mix", &samples)],
            &Manifest { case: "batch" },
        )
        .expect("write artifact bundle");

        let wav = fs::read(directory.join("mix.wav")).expect("read direct WAV file");
        let expected = samples
            .iter()
            .flat_map(|sample| sample.to_le_bytes())
            .collect::<Vec<_>>();
        assert_eq!(&wav[44..], expected);
    }

    #[kithara::test(native, flash(false))]
    fn repeated_case_writes_use_distinct_bundle_roots() {
        let root = tempfile::tempdir().expect("artifact temp dir");
        let samples = [0.25, -0.25];
        let write = || {
            write_audio_artifact_to(
                root.path(),
                "repeated-case",
                48_000,
                1,
                &[("mix", samples.as_slice())],
                &Manifest { case: "repeated" },
            )
            .expect("write unique artifact bundle")
        };

        let first = write();
        let second = write();

        assert_ne!(first, second);
        assert!(first.join("mix.wav").is_file());
        assert!(second.join("mix.wav").is_file());
    }

    #[kithara::test(native, flash(false))]
    fn listening_dump_uses_exact_wav_filenames_without_a_manifest() {
        let root = tempfile::tempdir().expect("artifact temp dir");
        let samples = [0.25, -0.25];
        let directory = write_audio_dump_to(
            root.path(),
            "legacy-listening-dump",
            48_000,
            1,
            &[
                ("01_deck_a_96bpm_sine", samples.as_slice()),
                ("02_deck_b_128bpm_square", samples.as_slice()),
                ("03_mix_on_a_120bpm_grid", samples.as_slice()),
                ("04_mix_riding_120_to_126", samples.as_slice()),
                ("05_mix_sweeping_90_to_145", samples.as_slice()),
            ],
        )
        .expect("write WAV-only listening dump");

        let filenames = fs::read_dir(&directory)
            .expect("read listening dump directory")
            .map(|entry| {
                entry
                    .expect("read listening dump entry")
                    .file_name()
                    .into_string()
                    .expect("artifact filename is UTF-8")
            })
            .collect::<BTreeSet<_>>();
        assert_eq!(
            filenames,
            BTreeSet::from([
                "01_deck_a_96bpm_sine.wav".to_owned(),
                "02_deck_b_128bpm_square.wav".to_owned(),
                "03_mix_on_a_120bpm_grid.wav".to_owned(),
                "04_mix_riding_120_to_126.wav".to_owned(),
                "05_mix_sweeping_90_to_145.wav".to_owned(),
            ])
        );
        assert!(!directory.join("manifest.json").exists());
    }

    #[kithara::test(native, flash(false))]
    fn relative_artifact_root_is_rejected_before_writing() {
        let error = write_audio_dump_to(
            Path::new("relative-artifacts"),
            "invalid-root",
            48_000,
            1,
            &[],
        )
        .expect_err("artifact root must be absolute");

        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
    }
}
