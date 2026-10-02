pub use kithara_record::assets::{AssetPartSink, AssetPartSinkError};

#[cfg(test)]
mod tests {
    use kithara_assets::{AssetResource, AssetSource, AssetStore, ReadSide, StorageBackend};
    use kithara_encode::EncodeConfig;
    use kithara_record::{RecordingConfig, RecordingCore};
    use kithara_test_fixtures::play_fixtures::recording as recording_pcm;
    use kithara_test_utils::kithara;

    use super::AssetPartSink;
    use crate::pools;

    struct RecordingArtifact;

    #[kithara::test]
    fn recording_core_commits_a_readable_wav_to_memory_assets(recording_pcm: Vec<f32>) {
        let pool = pools::build(&pools::PoolsSection::default())
            .unwrap_or_else(|error| panic!("app pools: {error}"));
        let store = AssetStore::builder(pool)
            .backend(StorageBackend::Memory)
            .build();
        let source = AssetSource::Local {
            path: std::env::temp_dir().join("kithara-recording-core-test"),
        };
        let key = store
            .scope::<RecordingArtifact>(&source)
            .and_then(|scope| {
                scope.key(&AssetResource::Named {
                    namespace: "recordings".to_owned(),
                    name: "master.wav".to_owned(),
                })
            })
            .unwrap_or_else(|error| panic!("recording asset key: {error}"));
        let sink = AssetPartSink::acquire(&store, &key)
            .unwrap_or_else(|error| panic!("recording sink: {error}"));
        let config = RecordingConfig::builder()
            .encode(
                EncodeConfig::builder()
                    .sample_rate(48_000)
                    .channels(2)
                    .build(),
            )
            .build();
        let mut recording = RecordingCore::new(&config, sink, Some(2))
            .unwrap_or_else(|error| panic!("recording session: {error}"));

        recording
            .push(&recording_pcm)
            .unwrap_or_else(|error| panic!("record PCM: {error}"));
        let _reader = recording
            .finish()
            .unwrap_or_else(|error| panic!("finish recording: {error}"));

        let reader = store
            .open_resource(&key, None)
            .unwrap_or_else(|error| panic!("reopen committed recording: {error}"));
        let len = reader.len().expect("committed WAV length");
        let mut bytes = vec![0_u8; usize::try_from(len).expect("test WAV length fits usize")];
        let read = reader
            .read_at(0, &mut bytes)
            .unwrap_or_else(|error| panic!("read committed recording: {error}"));

        assert_eq!(read, 60);
        assert_eq!(&bytes[0..4], b"RIFF");
        assert_eq!(&bytes[8..12], b"WAVE");
        assert_eq!(u16::from_le_bytes([bytes[20], bytes[21]]), 3);
        assert_eq!(&bytes[44..48], &0.25_f32.to_le_bytes());
    }
}
