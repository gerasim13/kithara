#![cfg(not(target_arch = "wasm32"))]

use hls_m3u8::{MasterPlaylist, MediaPlaylist, tags::VariantStream};
use kithara_drm::{DecryptContext, aes128_cbc_process_chunk};
use kithara_test_fixtures::hls::{HlsBundle, gapless_drm, gapless_plain, long_drm, long_plain};
use kithara_test_utils::kithara;

const LABELS: [&str; 4] = ["slq", "smq", "shq", "slossless"];
const SEGMENTS: usize = 37;

fn bytes<'a>(bundle: &'a HlsBundle, route: &str) -> Vec<u8> {
    let resource = bundle
        .get(route)
        .unwrap_or_else(|| panic!("bundle has no `{route}`"));
    std::fs::read(resource.path())
        .unwrap_or_else(|error| panic!("read {}: {error}", resource.path().display()))
}

fn text(bundle: &HlsBundle, route: &str) -> String {
    String::from_utf8(bytes(bundle, route))
        .unwrap_or_else(|error| panic!("`{route}` is not UTF-8: {error}"))
}

fn route(uri: &str) -> String {
    if uri.starts_with('/') {
        uri.to_owned()
    } else {
        format!("/hls/{uri}")
    }
}

fn assert_route(bundle: &HlsBundle, uri: &str) {
    let route = route(uri);
    assert!(bundle.get(&route).is_some(), "bundle has no `{route}`");
}

fn assert_bundle(bundle: &HlsBundle, encrypted: bool, segments: usize) {
    let master_text = text(bundle, bundle.master_route());
    let master = MasterPlaylist::try_from(master_text.as_str()).expect("parse generated master");
    assert_eq!(master.variant_streams.len(), LABELS.len());

    for stream in &master.variant_streams {
        let uri = match stream {
            VariantStream::ExtXIFrame { uri, .. } | VariantStream::ExtXStreamInf { uri, .. } => {
                uri.as_ref()
            }
        };
        assert_route(bundle, uri);
        let playlist_route = route(uri);
        let playlist_text = text(bundle, &playlist_route);
        let playlist = playlist_text
            .parse::<MediaPlaylist>()
            .expect("parse generated media playlist");
        assert!(playlist.has_end_list, "`{playlist_route}` must be VOD");
        assert_eq!(
            playlist.segments.values().count(),
            segments,
            "`{playlist_route}` segment count"
        );
        let mut maps = 0;
        let mut keys = 0;
        for segment in playlist.segments.values() {
            assert_route(bundle, segment.uri());
            if let Some(map) = &segment.map {
                maps += 1;
                assert_route(bundle, map.uri());
            }
            for key in segment.keys.iter().filter_map(|key| key.as_ref()) {
                keys += 1;
                assert_route(bundle, key.uri());
            }
        }
        assert_eq!(maps, 1, "`{playlist_route}` EXT-X-MAP count");
        assert_eq!(
            keys,
            segments * usize::from(encrypted),
            "`{playlist_route}` EXT-X-KEY count"
        );
    }

    for label in LABELS {
        assert_route(bundle, &format!("index-{label}-a1.m3u8"));
    }
}

#[kithara::test(native, flash(false))]
fn generated_plain_and_drm_bundles_are_complete() {
    let plain = long_plain();
    let drm = long_drm();

    assert_bundle(plain, false, SEGMENTS);
    assert_bundle(drm, true, SEGMENTS);

    let key = bytes(drm, "/hls/slq.key");
    assert_eq!(key.len(), 16);

    let plain_init = bytes(plain, "/hls/init-slq-a1.mp4");
    let encrypted_init = bytes(drm, "/hls/init-slq-a1.mp4");
    assert_ne!(encrypted_init, plain_init);

    let plain_media = bytes(plain, "/hls/segment-1-slq-a1.m4s");
    let encrypted_media = bytes(drm, "/hls/segment-1-slq-a1.m4s");
    assert_ne!(encrypted_media, plain_media);

    let mut decrypted = vec![0; encrypted_media.len()];
    let mut context = DecryptContext::new(key.try_into().expect("16-byte AES key"), [0; 16]);
    let written = aes128_cbc_process_chunk(&encrypted_media, &mut decrypted, &mut context, true)
        .expect("decrypt generated media");
    assert_eq!(&decrypted[..written], plain_media);
}

#[kithara::test(native, flash(false))]
fn generated_gapless_bundles_have_the_live_segment_topology() {
    for (bundle, encrypted) in [(gapless_plain(), false), (gapless_drm(), true)] {
        assert_bundle(bundle, encrypted, 9);

        for label in LABELS {
            let tolerance = if label == "slossless" { 0.11 } else { 0.03 };
            let playlist = text(bundle, &format!("/hls/index-{label}-a1.m3u8"))
                .parse::<MediaPlaylist>()
                .expect("parse generated gapless playlist");
            let durations = playlist
                .segments
                .values()
                .map(|segment| segment.duration.duration().as_secs_f64())
                .collect::<Vec<_>>();
            assert_eq!(durations.len(), 9);
            for (actual, expected) in durations
                .iter()
                .zip([4.0, 4.0, 10.0, 10.0, 10.0, 10.0, 10.0, 10.0])
            {
                assert!(
                    (actual - expected).abs() < tolerance,
                    "{label} segment duration {actual:.3} differs from {expected:.3}"
                );
            }
            assert!(
                (3.0..4.0).contains(&durations[8]),
                "{label} tail duration must mirror the short live tail: {:.3}",
                durations[8]
            );
        }
    }
}
