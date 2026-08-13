use std::{
    env, fs,
    io::{self, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use serde::Serialize;

const ARTIFACT_DIR_ENV: &str = "KITHARA_SYNC_ARTIFACT_DIR";
static ATTEMPT: AtomicU64 = AtomicU64::new(0);

pub fn write_audio_artifact<T: Serialize>(
    case: &str,
    sample_rate: u32,
    channels: u16,
    audio: &[(&str, &[f32])],
    manifest: &T,
) -> io::Result<Option<PathBuf>> {
    let Some(root) = env::var_os(ARTIFACT_DIR_ENV).map(PathBuf::from) else {
        return Ok(None);
    };
    if !root.is_absolute() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{ARTIFACT_DIR_ENV} must be an absolute path"),
        ));
    }
    validate_label(case)?;
    fs::create_dir_all(&root)?;
    let directory = loop {
        let attempt = ATTEMPT.fetch_add(1, Ordering::Relaxed);
        let candidate = root.join(format!("{case}-{}-{attempt}", std::process::id()));
        match fs::create_dir(&candidate) {
            Ok(()) => break candidate,
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(err) => return Err(err),
        }
    };

    for (label, samples) in audio {
        validate_label(label)?;
        write_float_wav(
            &directory.join(format!("{label}.wav")),
            samples,
            sample_rate,
            channels,
        )?;
    }

    let manifest_file = fs::File::create(directory.join("manifest.json"))?;
    serde_json::to_writer_pretty(manifest_file, manifest).map_err(io::Error::other)?;
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

fn write_float_wav(
    path: &Path,
    samples: &[f32],
    sample_rate: u32,
    channels: u16,
) -> io::Result<()> {
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
    let bytes_per_sample = 4u16;
    let data_bytes = u32::try_from(
        samples
            .len()
            .checked_mul(usize::from(bytes_per_sample))
            .ok_or_else(|| io::Error::other("WAV data size overflow"))?,
    )
    .map_err(|_| io::Error::other("WAV data exceeds RIFF limit"))?;
    let riff_size = 36u32
        .checked_add(data_bytes)
        .ok_or_else(|| io::Error::other("WAV RIFF size overflow"))?;
    let byte_rate = sample_rate
        .checked_mul(u32::from(channels))
        .and_then(|value| value.checked_mul(u32::from(bytes_per_sample)))
        .ok_or_else(|| io::Error::other("WAV byte rate overflow"))?;
    let block_align = channels
        .checked_mul(bytes_per_sample)
        .ok_or_else(|| io::Error::other("WAV block alignment overflow"))?;

    let mut file = fs::File::create(path)?;
    file.write_all(b"RIFF")?;
    file.write_all(&riff_size.to_le_bytes())?;
    file.write_all(b"WAVEfmt ")?;
    file.write_all(&16u32.to_le_bytes())?;
    file.write_all(&3u16.to_le_bytes())?;
    file.write_all(&channels.to_le_bytes())?;
    file.write_all(&sample_rate.to_le_bytes())?;
    file.write_all(&byte_rate.to_le_bytes())?;
    file.write_all(&block_align.to_le_bytes())?;
    file.write_all(&32u16.to_le_bytes())?;
    file.write_all(b"data")?;
    file.write_all(&data_bytes.to_le_bytes())?;
    for sample in samples {
        file.write_all(&sample.to_le_bytes())?;
    }
    file.sync_all()
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;

    #[test]
    fn float_wav_has_expected_header_and_payload_size() {
        let temp = tempdir().expect("temporary artifact directory");
        let path = temp.path().join("capture.wav");
        write_float_wav(&path, &[0.25, -0.25, 0.5, -0.5], 48_000, 2).expect("write float WAV");

        let bytes = fs::read(path).expect("read float WAV");
        assert_eq!(&bytes[0..4], b"RIFF");
        assert_eq!(&bytes[8..12], b"WAVE");
        assert_eq!(u16::from_le_bytes([bytes[20], bytes[21]]), 3);
        assert_eq!(bytes.len(), 44 + 4 * size_of::<f32>());
    }
}
