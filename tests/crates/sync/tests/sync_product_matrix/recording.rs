use super::{cases::*, imports::*};

#[cfg(not(target_os = "android"))]
struct FailingPartSink(AssetPartSink<TestPools>);

#[cfg(not(target_os = "android"))]
impl RecordingSink for FailingPartSink {
    type Error = io::Error;
    type Output = ();

    fn write_at(&mut self, _offset: u64, _bytes: &[u8]) -> Result<(), Self::Error> {
        Err(io::Error::other("injected recording sink failure"))
    }

    fn commit(&mut self, _final_len: u64) -> Result<Self::Output, Self::Error> {
        Err(io::Error::other("injected recording sink commit"))
    }

    fn abort(&mut self) {
        self.0.abort();
    }
}

#[cfg(not(target_os = "android"))]
struct OfflineRecordingArtifact;

#[cfg(not(target_os = "android"))]
fn recording_key(store: &AssetStore<TestPools>, name: &str) -> ResourceKey {
    let source = AssetSource::Local {
        path: env::temp_dir().join("kithara-offline-rendering-test"),
    };
    store
        .scope::<OfflineRecordingArtifact>(&source)
        .and_then(|scope| {
            scope.key(&AssetResource::Named {
                namespace: "offline-rendering".to_owned(),
                name: name.to_owned(),
            })
        })
        .unwrap_or_else(|error| panic!("offline recording key: {error}"))
}

#[cfg(not(target_os = "android"))]
fn recording_config(sample_rate: u32, packet_frames: usize) -> RecordingConfig {
    RecordingConfig::builder()
        .encode(
            EncodeConfig::builder()
                .sample_rate(sample_rate)
                .channels(CHANNELS)
                .packet_frames(packet_frames)
                .build(),
        )
        .build()
}

#[cfg(not(target_os = "android"))]
fn offline_render(sample_rate: NonZeroU32, frames: u64) -> (Host<TestPools>, OfflineRenderRequest) {
    let spec = AudioSpec::new(CHANNELS, sample_rate);
    let session = HostConfig::offline(pools())
        .settings(HostSettings::builder().sample_rate(sample_rate).build())
        .build();
    let host = Host::new(session).unwrap_or_else(|error| panic!("create offline Host: {error}"));
    let request = OfflineRenderRequest::builder()
        .spec(spec)
        .frames(0..frames)
        .build();
    (host, request)
}

#[cfg(not(target_os = "android"))]
#[kithara::test(native, timeout(Duration::from_secs(10)))]
fn offline_renderer_publishes_only_complete_recordings() {
    let sample_rate = NonZeroU32::new(48_000).expect("test sample rate");
    let frames = 8;
    let store = memory_asset_store();
    let config = recording_config(sample_rate.get(), 1);
    let success_key = recording_key(&store, "success.wav");
    let cancelled_key = recording_key(&store, "cancelled.wav");
    let failed_key = recording_key(&store, "failed.wav");

    let (mut success_host, success_request) = offline_render(sample_rate, frames);
    let success_sink = AssetPartSink::acquire(&store, &success_key)
        .unwrap_or_else(|error| panic!("acquire success sink: {error}"));
    let mut success = RecordingCore::new(&config, success_sink, Some(frames))
        .unwrap_or_else(|error| panic!("open success recording: {error}"));
    let success_cancel = CancelScope::new(None);
    let report = success_host
        .render(&success_request, &success_cancel.token(), &mut success)
        .unwrap_or_else(|error| panic!("render success recording: {error}"));
    assert_eq!(report.frames, frames);
    let reader = success
        .finish()
        .unwrap_or_else(|error| panic!("finish success recording: {error}"));
    let expected_len = 44 + frames * u64::from(CHANNELS) * 4;
    assert_eq!(reader.len(), Some(expected_len));
    let mut header = [0_u8; 44];
    let read = reader
        .read_at(0, &mut header)
        .unwrap_or_else(|error| panic!("read success recording: {error}"));
    assert_eq!(read, header.len());
    assert_eq!(&header[0..4], b"RIFF");
    assert_eq!(&header[8..12], b"WAVE");
    assert_eq!(u16::from_le_bytes([header[20], header[21]]), 3);
    assert!(matches!(
        store.resource_state(&success_key),
        Ok(AssetResourceState::Committed {
            final_len: Some(len)
        }) if len == expected_len
    ));

    let (mut cancelled_host, cancelled_request) = offline_render(sample_rate, frames);
    let cancelled_sink = AssetPartSink::acquire(&store, &cancelled_key)
        .unwrap_or_else(|error| panic!("acquire cancelled sink: {error}"));
    let mut cancelled = RecordingCore::new(&config, cancelled_sink, Some(frames))
        .unwrap_or_else(|error| panic!("open cancelled recording: {error}"));
    let cancelled_scope = CancelScope::new(None);
    cancelled_scope.cancel();
    assert!(matches!(
        cancelled_host.render(&cancelled_request, &cancelled_scope.token(), &mut cancelled),
        Err(kithara::output::OfflineRenderError::Cancelled { rendered_frames: 0 })
    ));
    drop(cancelled);
    assert_eq!(
        store
            .resource_state(&cancelled_key)
            .unwrap_or_else(|error| panic!("cancelled resource state: {error}")),
        AssetResourceState::Missing
    );

    let (mut failed_host, failed_request) = offline_render(sample_rate, frames);
    let failed_sink = AssetPartSink::acquire(&store, &failed_key).map_or_else(
        |error| panic!("acquire failing sink: {error}"),
        FailingPartSink,
    );
    let mut failed = RecordingCore::new(&config, failed_sink, Some(frames))
        .unwrap_or_else(|error| panic!("open failing recording: {error}"));
    assert!(matches!(
        failed_host.render(
            &failed_request,
            &CancelScope::new(None).token(),
            &mut failed
        ),
        Err(kithara::output::OfflineRenderError::Sink {
            rendered_frames: 0,
            ..
        })
    ));
    assert_eq!(
        store
            .resource_state(&failed_key)
            .unwrap_or_else(|error| panic!("failed resource state: {error}")),
        AssetResourceState::Missing
    );
}
