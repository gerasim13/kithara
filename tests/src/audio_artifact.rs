#[cfg(test)]
use std::collections::BTreeMap;
use std::{
    collections::BTreeSet,
    env, io,
    mem::{size_of, size_of_val},
    path::{Path, PathBuf},
};

use kithara::{
    assets::{
        AcquisitionResult, AssetReader, AssetResource, AssetScope, AssetSource, AssetStore,
        AssetWriter, DefaultLayout, ReadSide, StorageBackend, WriteSide,
    },
    bufpool::BytePool,
};
use serde::Serialize;
use url::Url;
use uuid::Uuid;

pub(crate) const AUDIO_ARTIFACT_DIR_ENV: &str = "KITHARA_AUDIO_ARTIFACT_DIR";

const ARTIFACT_NAMESPACE: &str = "test-artifact";
const ARTIFACT_SOURCE_URL: &str = "https://artifacts.kithara.invalid/";
const WAV_BATCH_SAMPLES: usize = 4 * 1024;

#[derive(Debug)]
pub(crate) struct WrittenAudioArtifact {
    directory: PathBuf,
    #[cfg(test)]
    readers: BTreeMap<String, AssetReader>,
}

impl WrittenAudioArtifact {
    #[must_use]
    pub(crate) fn directory(&self) -> &Path {
        &self.directory
    }

    #[cfg(test)]
    #[must_use]
    pub(crate) fn reader(&self, name: &str) -> Option<&AssetReader> {
        self.readers.get(name)
    }
}

/// Write listening WAV files and a manifest when the artifact directory is set.
/// Returns `Ok(None)` when unset.
///
/// # Errors
/// Returns an error for invalid audio, serialization, or asset-store failure.
pub fn write_audio_artifact<T: Serialize>(
    pool: &BytePool,
    case: &str,
    sample_rate: u32,
    channels: u16,
    audio: &[(&str, &[f32])],
    manifest: &T,
) -> io::Result<Option<PathBuf>> {
    let Some(root) = env::var_os(AUDIO_ARTIFACT_DIR_ENV).map(PathBuf::from) else {
        return Ok(None);
    };
    write_audio_artifact_to(&root, pool, case, sample_rate, channels, audio, manifest)
        .map(|written| Some(written.directory))
}

pub(crate) fn write_audio_artifact_to<T: Serialize>(
    root: &Path,
    pool: &BytePool,
    case: &str,
    sample_rate: u32,
    channels: u16,
    audio: &[(&str, &[f32])],
    manifest: &T,
) -> io::Result<WrittenAudioArtifact> {
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

    let store = AssetStore::builder()
        .backend(StorageBackend::Disk {
            root: root.join(format!("{case}-{}", Uuid::new_v4())),
        })
        .pool(pool.clone())
        .build();
    let (scope, manifest_writer) = claim_bundle(&store, case)?;
    let directory = manifest_writer
        .reader()
        .path()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .ok_or_else(|| io::Error::other("disk artifact has no directory path"))?;

    #[cfg(test)]
    let mut readers = BTreeMap::new();
    for (label, samples) in audio {
        let filename = format!("{label}.wav");
        let reader = write_float_wav(&scope, &filename, samples, sample_rate, channels)?;
        #[cfg(test)]
        readers.insert(filename, reader);
        #[cfg(not(test))]
        drop(reader);
    }

    let manifest_bytes = serde_json::to_vec_pretty(manifest).map_err(io::Error::other)?;
    manifest_writer
        .write_at(0, &manifest_bytes)
        .map_err(io::Error::other)?;
    let manifest_len = u64::try_from(manifest_bytes.len())
        .map_err(|_| io::Error::other("artifact manifest length exceeds u64"))?;
    let manifest_reader = manifest_writer
        .commit(Some(manifest_len))
        .map_err(io::Error::other)?;
    #[cfg(test)]
    readers.insert("manifest.json".to_owned(), manifest_reader);
    #[cfg(not(test))]
    drop(manifest_reader);
    store.checkpoint().map_err(io::Error::other)?;

    Ok(WrittenAudioArtifact {
        directory,
        #[cfg(test)]
        readers,
    })
}

fn claim_bundle(store: &AssetStore, case: &str) -> io::Result<(AssetScope, AssetWriter)> {
    let url = Url::parse(ARTIFACT_SOURCE_URL).map_err(io::Error::other)?;
    let source = AssetSource::Remote {
        url,
        discriminator: Some(case.to_owned()),
    };
    let scope = store
        .scope::<DefaultLayout>(&source)
        .map_err(io::Error::other)?;
    let key = scope
        .key(&artifact_resource("manifest.json"))
        .map_err(io::Error::other)?;
    match store
        .acquire_resource(&key, None)
        .map_err(io::Error::other)?
    {
        AcquisitionResult::Pending(writer) => Ok((scope, writer)),
        state => Err(io::Error::other(format!(
            "new artifact store returned unexpected manifest acquisition state: {state:?}"
        ))),
    }
}

fn artifact_resource(name: &str) -> AssetResource {
    AssetResource::Named {
        namespace: ARTIFACT_NAMESPACE.to_owned(),
        name: name.to_owned(),
    }
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
    scope: &AssetScope,
    filename: &str,
    samples: &[f32],
    sample_rate: u32,
    channels: u16,
) -> io::Result<AssetReader> {
    let header = wav_header(samples.len(), sample_rate, channels)?;
    let key = scope
        .key(&artifact_resource(filename))
        .map_err(io::Error::other)?;
    let AcquisitionResult::Pending(writer) = scope
        .store()
        .acquire_resource(&key, None)
        .map_err(io::Error::other)?
    else {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("artifact resource already exists: {filename}"),
        ));
    };
    writer.write_at(0, &header).map_err(io::Error::other)?;

    let mut offset = u64::try_from(header.len())
        .map_err(|_| io::Error::other("WAV header length exceeds u64"))?;
    let mut encoded = [0_u8; WAV_BATCH_SAMPLES * size_of::<f32>()];
    for batch in samples.chunks(WAV_BATCH_SAMPLES) {
        for (sample, bytes) in batch.iter().zip(encoded.chunks_exact_mut(size_of::<f32>())) {
            bytes.copy_from_slice(&sample.to_le_bytes());
        }
        let byte_len = size_of_val(batch);
        writer
            .write_at(offset, &encoded[..byte_len])
            .map_err(io::Error::other)?;
        offset = offset
            .checked_add(
                u64::try_from(byte_len)
                    .map_err(|_| io::Error::other("WAV batch length exceeds u64"))?,
            )
            .ok_or_else(|| io::Error::other("WAV payload offset exceeds u64"))?;
    }
    writer.commit(Some(offset)).map_err(io::Error::other)
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
    use kithara::assets::ReadSide;
    use serde::Serialize;

    use super::*;
    use crate::kithara;

    #[derive(Serialize)]
    struct Manifest {
        case: &'static str,
    }

    #[kithara::test(native, flash(false))]
    fn artifact_bytes_roundtrip_through_asset_store_readers() {
        let root = tempfile::tempdir().expect("artifact temp dir");
        let samples = [0.25, -0.25, 0.5, -0.5];
        let written = write_audio_artifact_to(
            root.path(),
            &BytePool::default(),
            "asset-store-roundtrip",
            48_000,
            2,
            &[("mix", samples.as_slice())],
            &Manifest { case: "roundtrip" },
        )
        .expect("write artifact bundle");

        let mut wav = Vec::new();
        written
            .reader("mix.wav")
            .expect("WAV reader")
            .read_into(&mut wav)
            .expect("read WAV through AssetStore");
        assert_eq!(&wav[..4], b"RIFF");
        assert_eq!(&wav[8..12], b"WAVE");
        assert_eq!(wav.len(), 44 + samples.len() * size_of::<f32>());

        let mut manifest = Vec::new();
        written
            .reader("manifest.json")
            .expect("manifest reader")
            .read_into(&mut manifest)
            .expect("read manifest through AssetStore");
        assert!(
            String::from_utf8(manifest)
                .expect("manifest UTF-8")
                .contains("roundtrip")
        );

        assert!(written.directory().starts_with(root.path()));
        let wav_path = written.directory().join("mix.wav");
        assert_eq!(
            written.reader("mix.wav").and_then(ReadSide::path),
            Some(wav_path.as_path()),
        );
    }

    #[kithara::test(native, flash(false))]
    fn wav_payload_survives_an_asset_store_batch_boundary() {
        let root = tempfile::tempdir().expect("artifact temp dir");
        let samples = (0..WAV_BATCH_SAMPLES + 2)
            .map(|index| {
                f32::from_bits(0x3e80_0000 + u32::try_from(index).expect("sample index fits u32"))
            })
            .collect::<Vec<_>>();
        let written = write_audio_artifact_to(
            root.path(),
            &BytePool::default(),
            "batch-boundary",
            48_000,
            1,
            &[("mix", &samples)],
            &Manifest { case: "batch" },
        )
        .expect("write artifact bundle");

        let mut wav = Vec::new();
        written
            .reader("mix.wav")
            .expect("WAV reader")
            .read_into(&mut wav)
            .expect("read WAV through AssetStore");
        let expected = samples
            .iter()
            .flat_map(|sample| sample.to_le_bytes())
            .collect::<Vec<_>>();
        assert_eq!(&wav[44..], expected);
    }

    #[kithara::test(native, flash(false))]
    fn repeated_case_writes_use_distinct_bundle_roots() {
        let root = tempfile::tempdir().expect("artifact temp dir");
        let pool = BytePool::default();
        let samples = [0.25, -0.25];
        let write = || {
            write_audio_artifact_to(
                root.path(),
                &pool,
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

        assert_ne!(first.directory(), second.directory());
        assert!(first.reader("mix.wav").is_some());
        assert!(second.reader("mix.wav").is_some());
    }
}
