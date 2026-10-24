use super::*;

#[kithara::test(tokio)]
async fn load_track_propagates_host_sample_rate() {
    let host_rate = 88_200u32;
    let rate = NonZeroU32::new(host_rate).expect("host rate");
    let dir = TestTempDir::new();
    let path = dir.path().join("rate.wav");
    std::fs::write(
        &path,
        kithara_test_fixtures::assets::fixed_constant_wav_cd_half_1024().bytes(),
    )
    .expect("float WAV fixture");
    let pools = pools();
    let prep = crate::ResourcePrep::builder()
        .worker(crate::PlayWorker::new(
            crate::PlayWorkerConfig::builder(pools.clone()).build(),
        ))
        .build();
    let output = crate::SessionOutputView::new(rate);
    let config = ResourceConfig::for_src(ResourceSrc::Path(path))
        .store(
            AssetStore::builder(pools)
                .backend(kithara_assets::StorageBackend::Memory)
                .build(),
        )
        .build();
    let prepared = prep
        .prepare(config, &output.get())
        .expect("prepared source");
    let mut rig = DeckRig::new(DeckMixerConfig::default()).expect("deck scope");
    let loaded = rig
        .load_fixture(
            &prep.worker,
            ResourceLoad::new(prepared, Box::new(AudioObserverSlot::default().relay())),
            Duration::ZERO,
        )
        .await
        .expect("worker opened source");
    assert_eq!(loaded.opened.pcm.spec().sample_rate.get(), host_rate);
}
