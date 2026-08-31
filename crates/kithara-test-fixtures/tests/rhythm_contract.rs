#![cfg(not(target_arch = "wasm32"))]

use kithara_analysis::BeatArtifact;
use kithara_test_fixtures::assets::by_name;
use kithara_test_utils::kithara;

const STYLES: [(&str, f64); 6] = [
    ("ambient_dub_62", 62.0),
    ("trip_hop_74", 74.0),
    ("downtempo_96", 96.0),
    ("house_124", 124.0),
    ("techno_132", 132.0),
    ("breakbeat_140", 140.0),
];
const CONTROLS: [&str; 4] = [
    "aligned",
    "one_frame_late",
    "one_beat_bar_late",
    "missing_beat",
];

fn artifact(style: &str, control: &str) -> BeatArtifact {
    let name = format!("rhythm_expected_beat_{style}_{control}");
    let asset = by_name(&name).unwrap_or_else(|| panic!("missing `{name}`"));
    BeatArtifact::try_from(asset.bytes()).unwrap_or_else(|error| panic!("decode `{name}`: {error}"))
}

#[kithara::test(native, flash(false))]
fn six_styles_expose_every_rhythm_control() {
    for (style, bpm) in STYLES {
        for control in CONTROLS {
            let wav = format!("rhythm_wav_{style}_{control}");
            let wav = by_name(&wav).unwrap_or_else(|| panic!("missing `{wav}`"));
            assert_eq!(wav.entry().content_type, "audio/wav");
            assert!(wav.bytes().starts_with(b"RIFF"));

            let artifact = artifact(style, control);
            assert_eq!(artifact.bpm(), bpm);
            assert!(!artifact.beats().is_empty());
            assert!(!artifact.downbeats().is_empty());
        }
    }
}

#[kithara::test(native, flash(false))]
fn score_truth_distinguishes_all_three_negative_controls() {
    for (style, _) in STYLES {
        let aligned = artifact(style, "aligned");
        let one_frame = artifact(style, "one_frame_late");
        let bar_phase = artifact(style, "one_beat_bar_late");
        let missing = artifact(style, "missing_beat");

        assert_eq!(aligned.beats().len(), one_frame.beats().len());
        assert!(
            aligned
                .beats()
                .iter()
                .zip(one_frame.beats())
                .all(|(expected, shifted)| shifted == &expected.saturating_add(1)),
            "{style}: one-frame control must move every beat by exactly one frame"
        );
        assert_eq!(aligned.beats(), bar_phase.beats());
        assert_ne!(aligned.downbeats(), bar_phase.downbeats());
        assert_eq!(aligned.beats().len(), missing.beats().len() + 1);
        assert!(
            missing
                .beats()
                .windows(2)
                .any(|pair| pair[1].saturating_sub(pair[0])
                    > aligned.beats()[1] - aligned.beats()[0]),
            "{style}: missing-beat control must leave a real hole"
        );
    }
}
