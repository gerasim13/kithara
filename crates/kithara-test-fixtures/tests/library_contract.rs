#![cfg(not(target_arch = "wasm32"))]

use std::io::Cursor;

use kithara_analysis::{AnalysisFile, AnalysisFingerprint, BeatGridModel};
use kithara_decode::{DecoderConfig, DecoderFactory};
use kithara_resampler::NoResamplerBackend;
#[cfg(all(test, target_os = "android"))]
use kithara_test_dylib as _;
use kithara_test_fixtures::assets::{self, by_name};
use kithara_test_utils::{
    bufpool::{TestPools, pools},
    kithara,
};

mod consts {
    pub(super) const ANALYSIS_FINGERPRINT: &str = "rhythm-fixture:v1";
    /// The reference track sync is heard on: a steady 4/4 at 128 BPM.
    pub(super) const TUNNEL: &str = "library_mp3_zvuk_27390231";
}

fn analysis_of(track: &str) -> AnalysisFile {
    let name = format!("analysis_{track}");
    let asset = by_name(&name).unwrap_or_else(|| panic!("`{name}` is not registered"));
    AnalysisFile::parse(
        asset.bytes(),
        &AnalysisFingerprint::new(Some(consts::ANALYSIS_FINGERPRINT), None),
    )
    .unwrap_or_else(|error| panic!("decode `{name}`: {error}"))
}

/// A grid read against another rate than the track's puts every beat at the
/// wrong second.
#[kithara::test(native, flash(false))]
fn every_library_track_is_analysed_at_the_rate_it_decodes_at() {
    let tracks = assets::MANIFEST.iter().filter(|entry| {
        entry.name.starts_with("library_") && entry.content_type.starts_with("audio/")
    });
    for entry in tracks {
        let track = by_name(entry.name).unwrap_or_else(|| panic!("`{}` is registered", entry.name));
        let hint = entry.path.rsplit('.').next();
        let config = DecoderConfig::<NoResamplerBackend, TestPools>::builder()
            .pools(pools())
            .build();
        let decoder =
            DecoderFactory::create_with_probe(Cursor::new(track.bytes().to_vec()), hint, config)
                .unwrap_or_else(|error| panic!("open `{}`: {error}", entry.name));

        assert_eq!(
            analysis_of(entry.name)
                .latest()
                .analysis()
                .source_sample_rate(),
            decoder.spec().sample_rate,
            "`{}`",
            entry.name
        );
    }
}

#[kithara::test(native, flash(false))]
fn a_steady_four_four_track_states_every_beat_and_its_bars() {
    let file = analysis_of(consts::TUNNEL);
    let analysis = file.latest().analysis();
    let heard = analysis
        .beat()
        .expect("the reference track has beats")
        .artifact()
        .beats()
        .len();
    let grid = BeatGridModel::try_from(analysis).expect("the reference track states a grid");

    assert_eq!(
        grid.as_raw().beats.len(),
        heard,
        "no beat is lost to the grid"
    );
    assert_eq!(
        grid.as_raw().meter.map(|meter| meter.beats_per_bar.get()),
        Some(4)
    );
}
