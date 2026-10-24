use std::{
    num::NonZeroU64,
    sync::{Mutex, PoisonError},
};

use kithara_fixture_media::reference::AudioReference;
use kithara_platform::tokio::process::Command;

pub(crate) fn format() -> Vec<u8> {
    Vec::try_from(&AudioReference {
        sample_rate: 48_000,
        channels: 1,
        samples: vec![0.0],
        presentation_frames: NonZeroU64::new(1),
    })
    .unwrap_or_else(|error| panic!("reference format sample: {error}"))
}

/// Bounds simultaneous independent decoder output buffers for whole tracks.
pub(crate) fn prepare(inputs: &[&[u8]], ext: &str) -> Vec<u8> {
    static DECODING: Mutex<()> = Mutex::new(());
    let _decoding = DECODING.lock().unwrap_or_else(PoisonError::into_inner);
    let [input] = inputs else {
        panic!("an audio reference requires one whole-track dependency");
    };
    let input = without_leading_id3v2(input);
    let directory =
        tempfile::tempdir().unwrap_or_else(|error| panic!("reference directory: {error}"));
    let path = directory.path().join(format!("source.{ext}"));
    std::fs::write(&path, input).unwrap_or_else(|error| panic!("reference input: {error}"));
    let metadata = output(
        Command::new("ffprobe")
            .args([
                "-v",
                "error",
                "-select_streams",
                "a",
                "-show_entries",
                "stream=sample_rate,channels,time_base,duration_ts",
                "-of",
                "json",
            ])
            .arg(&path)
            .stdin(std::process::Stdio::null()),
    );
    assert!(
        metadata.status.success(),
        "reference FFprobe: {}",
        String::from_utf8_lossy(&metadata.stderr)
    );
    let metadata: serde_json::Value = serde_json::from_slice(&metadata.stdout)
        .unwrap_or_else(|error| panic!("reference FFprobe metadata: {error}"));
    let streams = metadata["streams"]
        .as_array()
        .unwrap_or_else(|| panic!("reference has no audio streams"));
    let [stream] = streams.as_slice() else {
        panic!("reference must have exactly one audio stream");
    };
    let sample_rate = stream["sample_rate"]
        .as_str()
        .and_then(|rate| rate.parse().ok())
        .filter(|rate| *rate > 0)
        .unwrap_or_else(|| panic!("reference has no native sample rate"));
    let channels = stream["channels"]
        .as_u64()
        .and_then(|channels| u16::try_from(channels).ok())
        .filter(|channels| *channels > 0)
        .unwrap_or_else(|| panic!("reference has no native channel count"));
    let decoded = output(
        Command::new("ffmpeg")
            .args([
                "-nostdin",
                "-hide_banner",
                "-loglevel",
                "error",
                "-threads",
                "1",
                "-i",
            ])
            .arg(&path)
            .args(["-f", "f32le", "-acodec", "pcm_f32le", "-"])
            .stdin(std::process::Stdio::null()),
    );
    assert!(
        decoded.status.success(),
        "reference FFmpeg: {}",
        String::from_utf8_lossy(&decoded.stderr)
    );
    let (samples, remainder) = decoded.stdout.as_chunks::<4>();
    assert!(
        remainder.is_empty(),
        "reference FFmpeg returned a partial sample"
    );
    Vec::try_from(&AudioReference {
        sample_rate,
        channels,
        samples: samples
            .iter()
            .map(|sample| f32::from_le_bytes(*sample))
            .collect(),
        presentation_frames: presentation_frames(stream, sample_rate),
    })
    .unwrap_or_else(|error| panic!("invalid independent audio reference: {error}"))
}

#[cfg(any(feature = "wav", feature = "rhythm"))]
pub(crate) fn authored(inputs: &[&[u8]], window_frames: Option<usize>) -> Vec<u8> {
    let [input] = inputs else {
        panic!("authored PCM requires one fixture WAV dependency");
    };
    let (header, pcm) = input
        .split_at_checked(44)
        .expect("authored fixture has a complete WAV header");
    let sample_rate = u32::from_le_bytes(header[24..28].try_into().expect("WAV rate field"));
    let channels = u16::from_le_bytes(header[22..24].try_into().expect("WAV channel field"));
    assert_eq!(
        header,
        crate::signal::header(sample_rate, channels, Some(pcm.len()))
    );
    let (samples, remainder) = pcm.as_chunks::<2>();
    assert!(remainder.is_empty(), "authored PCM has complete samples");
    let sample_count = window_frames.map_or(samples.len(), |frames| frames * usize::from(channels));
    let samples: Vec<_> = samples[..sample_count]
        .iter()
        .map(|sample| f32::from(i16::from_le_bytes(*sample)) / f32::from(i16::MAX))
        .collect();
    let frames = u64::try_from(samples.len() / usize::from(channels)).expect("fixture frame count");
    Vec::try_from(&AudioReference {
        sample_rate,
        channels,
        samples,
        presentation_frames: NonZeroU64::new(frames),
    })
    .unwrap_or_else(|error| panic!("invalid authored PCM reference: {error}"))
}

fn without_leading_id3v2(mut input: &[u8]) -> &[u8] {
    while let Some(length) = kithara_stream::id3v2_tag_len(input) {
        let Ok(length) = usize::try_from(length) else {
            break;
        };
        let Some(payload) = input.get(length..) else {
            break;
        };
        input = payload;
    }
    input
}

fn output(command: &mut Command) -> std::process::Output {
    let runtime = kithara_platform::tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap_or_else(|error| panic!("reference process runtime: {error}"));
    let _io = kithara_platform::flash::real_io();
    runtime.block_on(async {
        kithara_platform::time::timeout(
            kithara_platform::time::Duration::from_secs(30),
            command.kill_on_drop(true).output(),
        )
        .await
        .unwrap_or_else(|error| panic!("reference media process exceeded 30 seconds: {error}"))
        .unwrap_or_else(|error| panic!("reference media process failed: {error}"))
    })
}

fn presentation_frames(stream: &serde_json::Value, sample_rate: u32) -> Option<NonZeroU64> {
    let duration = stream["duration_ts"].as_u64()?;
    let (numerator, denominator) = stream["time_base"].as_str()?.split_once('/')?;
    let numerator = numerator.parse::<u64>().ok()?;
    let denominator = denominator.parse::<NonZeroU64>().ok()?;
    let frames = u128::from(duration)
        .checked_mul(u128::from(numerator))?
        .checked_mul(u128::from(sample_rate))?;
    if !frames.is_multiple_of(u128::from(denominator.get())) {
        return None;
    }
    NonZeroU64::new(u64::try_from(frames / u128::from(denominator.get())).ok()?)
}

#[cfg(test)]
mod tests {
    use kithara_test_utils::kithara;

    use super::{presentation_frames, without_leading_id3v2};

    #[kithara::test]
    fn independent_reference_removes_complete_leading_metadata_only() {
        let first = b"ID3\x04\x00\x00\x00\x00\x00\x04body";
        let second = b"ID3\x04\x00\x10\x00\x00\x00\x03tag3DI\x04\x00\x10\x00\x00\x00\x03";
        let payload = b"RIFF\x04\x00\x00\x00WAVEID3";
        let tagged = [first.as_slice(), second.as_slice(), payload.as_slice()].concat();
        assert_eq!(without_leading_id3v2(&tagged), payload);
        assert_eq!(without_leading_id3v2(payload), payload);
    }

    #[kithara::test]
    fn independent_reference_preserves_partial_and_invalid_metadata() {
        let tag = b"ID3\x04\x00\x10\x00\x00\x00\x03tag3DI\x04\x00\x10\x00\x00\x00\x03";
        for length in 0..tag.len() {
            let partial = &tag[..length];
            assert_eq!(without_leading_id3v2(partial), partial);
        }
        for invalid in [
            b"ID3\x01\x00\x00\x00\x00\x00\x00RIFF",
            b"ID3\x04\xff\x00\x00\x00\x00\x00RIFF",
            b"ID3\x04\x00\x00\x00\x00\x00\x80RIFF",
        ] {
            assert_eq!(without_leading_id3v2(invalid), invalid);
        }
        let first = b"ID3\x04\x00\x00\x00\x00\x00\x00";
        let partial = &tag[..12];
        let tagged = [first.as_slice(), partial].concat();
        assert_eq!(without_leading_id3v2(&tagged), partial);
    }

    #[kithara::test]
    fn independent_reference_is_registered_after_its_whole_track() {
        let definition = inventory::iter::<crate::registry::AssetDef>
            .into_iter()
            .find(|definition| {
                definition.accessor_name() == "reference_rhythm_wav_deck_a_120bpm_48k"
            })
            .expect("whole-track reference registration");
        assert_eq!(definition.dependencies, &["rhythm_wav_deck_a_120bpm_48k"]);
        assert_eq!(definition.ext, "pcm-reference");
        let format = definition
            .format
            .expect("reference format participates in its cache key");
        assert!(kithara_fixture_media::reference::AudioReference::from_bytes(&format()).is_ok());
    }

    #[kithara::test]
    fn only_exact_positive_declared_presentations_have_a_frame_count() {
        for (ticks, base, expected) in [
            (6, "1/1", Some(264_600)),
            (1, "1/7", Some(6_300)),
            (1, "1/13", None),
            (0, "1/1", None),
            (1, "1/0", None),
        ] {
            let stream = serde_json::json!({"duration_ts": ticks, "time_base": base});
            assert_eq!(
                presentation_frames(&stream, 44_100).map(std::num::NonZeroU64::get),
                expected
            );
        }
        assert_eq!(presentation_frames(&serde_json::json!({}), 44_100), None);
    }
}
