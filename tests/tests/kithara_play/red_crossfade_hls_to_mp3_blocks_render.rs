#![cfg(not(target_arch = "wasm32"))]
#![forbid(unsafe_code)]

use std::num::NonZeroU32;

use kithara::{
    audio::AudioConfig,
    file::{File as FileSource, FileConfig, FileSrc},
    hls::{Hls, HlsConfig},
    host::HostConfig,
    platform::{sync::Arc, time::Duration},
    play::{PlayWorker, PlayWorkerConfig, Resource},
    stream::{AudioCodec, ContainerFormat, MediaInfo},
};
use kithara_integration_tests::{offline::resource_from_reader, temp_dir};
use kithara_test_fixtures::{
    assets::signal_mp3_track_sine440_187s,
    signal::{self, Wave},
};
use tracing::info;

use crate::{
    bufpool_ext::{TestPools, pools},
    common::test_defaults::Consts as Shared,
    continuity::render_offline_window,
};

struct Consts;
impl Consts {
    const READ_TIMEOUT: Duration = Shared::READ_TIMEOUT;
    const BLOCK: usize = 512;
    const SR: u32 = Shared::SAMPLE_RATE;
}

#[kithara::test(
    tokio,
    timeout(Duration::from_secs(30)),
    hang_timeout_secs(10),
    tracing("kithara_audio=debug,kithara_decode=debug,kithara_play=debug,kithara_stream=debug"),
    sync_session
)]
async fn red_hls_to_mp3_crossfade_no_render_budget_violations() {
    use kithara::assets::{AssetStore, StorageBackend};
    use kithara_integration_tests::{
        hls_server::{HlsTestServer, HlsTestServerConfig},
        offline::OfflinePlayer,
    };

    const HLS_SEGMENT_COUNT: usize = 3;
    const HLS_SEGMENT_SIZE: usize = 200_000;
    const HLS_TOTAL_BYTES: usize = HLS_SEGMENT_COUNT * HLS_SEGMENT_SIZE;
    const HLS_SAMPLE_RATE: f64 = 44_100.0;
    const HLS_CHANNELS: f64 = 2.0;

    let segment_duration = HLS_SEGMENT_SIZE as f64 / (HLS_SAMPLE_RATE * HLS_CHANNELS * 2.0);
    let hls_server = HlsTestServer::new(HlsTestServerConfig {
        custom_data: Some(Arc::new(signal::wav_of_size(
            44_100u32,
            2u16,
            HLS_TOTAL_BYTES,
            Wave::Sawtooth,
        ))),
        segment_duration_secs: segment_duration,
        segment_size: HLS_SEGMENT_SIZE,
        segments_per_variant: HLS_SEGMENT_COUNT,
        ..Default::default()
    })
    .await;
    let pools = pools();
    let store = AssetStore::builder(pools.clone())
        .backend(StorageBackend::Memory)
        .cache_capacity(std::num::NonZeroUsize::new(4).expect("nonzero"))
        .max_assets(8)
        .build();
    let hls_url = hls_server.url("/master.m3u8");

    let worker = PlayWorker::new(PlayWorkerConfig::builder(pools.clone()).build());
    let mut player = OfflinePlayer::new(
        HostConfig::offline(pools.clone())
            .sample_rate(NonZeroU32::new(Consts::SR).expect("sample rate is non-zero"))
            .build(),
    );

    let media_dir = temp_dir();
    let local_mp3 = media_dir.write("track.mp3", signal_mp3_track_sine440_187s().bytes());

    let make_mp3 = |w: PlayWorker<TestPools>| {
        let p = local_mp3.clone();
        let store = store.clone();
        async move {
            let file_cfg = FileConfig::for_src(FileSrc::Local(p))
                .store(store)
                .pools(w.pools().clone())
                .build();
            let audio_cfg = AudioConfig::<FileSource<TestPools>>::for_stream(file_cfg)
                .hint("mp3".to_string())
                .build();
            let audio = w.open(audio_cfg).await.expect("create local MP3 audio");
            resource_from_reader(audio)
        }
    };

    let make_hls = |w: PlayWorker<TestPools>, s: AssetStore<TestPools>| {
        let u = hls_url.clone();
        async move {
            let wav_info = MediaInfo::builder()
                .maybe_codec(Some(AudioCodec::Pcm))
                .maybe_container(Some(ContainerFormat::Wav))
                .build();
            let cfg = HlsConfig::for_url(u)
                .store(s)
                .pools(w.pools().clone())
                .build();
            let audio_cfg = AudioConfig::<Hls<TestPools>>::for_stream(cfg)
                .media_info(wav_info)
                .build();
            let audio = w.open(audio_cfg).await.expect("create HLS audio");
            let mut r: Resource = resource_from_reader(audio);
            time::timeout(Consts::READ_TIMEOUT, r.preload())
                .await
                .expect("HLS preload")
                .expect("HLS preload result");
            r
        }
    };

    let mut worst_slow_renders: u32 = 0;
    let mut worst_max_render: Duration = Duration::ZERO;
    let mut worst_label = String::new();

    for iter in 0..10 {
        let hls = make_hls(worker.clone(), store.clone()).await;
        player.load_and_fadein(hls);
        let _hls_warmup = render_offline_window(
            &mut player,
            40,
            &format!("HLS warmup #{iter}"),
            Consts::BLOCK,
            Consts::SR,
        );

        let mut mp3 = make_mp3(worker.clone()).await;
        time::timeout(Consts::READ_TIMEOUT, mp3.preload())
            .await
            .expect("MP3 preload")
            .expect("MP3 preload result");
        let before_fade = Instant::now();
        player.load_and_fadein(mp3);
        let fade_stats = render_offline_window(
            &mut player,
            60,
            &format!("HLS→MP3 red #{iter}"),
            Consts::BLOCK,
            Consts::SR,
        );
        info!(
            "iter {iter}: {fade_stats}, wall={:?}",
            before_fade.elapsed()
        );

        if fade_stats.slow_renders > worst_slow_renders {
            worst_slow_renders = fade_stats.slow_renders;
            worst_max_render = fade_stats.max_render;
            worst_label = fade_stats.label.clone();
        }
    }

    assert!(
        worst_slow_renders <= 1,
        "red: HLS→MP3 crossfade exceeded render budget on {} blocks \
         (worst label={worst_label}, max_render={worst_max_render:?}) — \
         render thread was blocked synchronously waiting for MP3 PCM chunks \
         while the shared worker was busy on HLS",
        worst_slow_renders,
    );
}
