#![cfg(feature = "render")]

mod common;

use std::{borrow::Cow, env, fmt::Write as _, fs, path::Path};

use iced::{
    Pixels, Size,
    advanced::{
        graphics::text::font_system,
        layout::{Layout, Limits},
        widget::Tree,
    },
};
use iced_renderer::fallback::Renderer as FallbackRenderer;
use iced_tiny_skia::Renderer as TinySkiaRenderer;
use kithara_test_utils::kithara;
use kithara_ui::{
    builtin,
    compile::{CompiledUi, compile},
    render::{
        ReadValue, Reads, Skin, StereoLevels, TrackRow, WaveBucket, WaveformView,
        fonts::{FONT_BYTES, SANS},
        tree,
    },
    source::UiConfig,
};

struct FixtureReads {
    beats: [f32; 4],
    buckets: [WaveBucket; 6],
    cues: [f32; 2],
    downbeats: [f32; 2],
    tracks: [TrackRow<'static>; 3],
}

impl Default for FixtureReads {
    fn default() -> Self {
        Self {
            beats: [0.125, 0.375, 0.625, 0.875],
            buckets: [
                WaveBucket {
                    low: 0.25,
                    mid: 0.50,
                    high: 0.75,
                },
                WaveBucket {
                    low: 0.65,
                    mid: 0.35,
                    high: 0.15,
                },
                WaveBucket {
                    low: 0.45,
                    mid: 0.80,
                    high: 0.30,
                },
                WaveBucket {
                    low: 0.90,
                    mid: 0.55,
                    high: 0.20,
                },
                WaveBucket {
                    low: 0.30,
                    mid: 0.60,
                    high: 0.85,
                },
                WaveBucket {
                    low: 0.70,
                    mid: 0.40,
                    high: 0.50,
                },
            ],
            cues: [0.25, 0.75],
            downbeats: [0.0, 0.5],
            tracks: [
                TrackRow {
                    title: "Midnight Circuit",
                    artist: Some("Neon Lines"),
                    time: Some("04:12"),
                    search: Some("midnight circuit neon lines"),
                    deck: Some("A"),
                    bpm: Some("128.0"),
                    key: Some("8A"),
                    energy: Some(82),
                    transition: Some("Blend"),
                    selected: true,
                },
                TrackRow {
                    title: "Signal Path",
                    artist: Some("Static Motion"),
                    time: Some("03:47"),
                    search: Some("signal path static motion"),
                    deck: None,
                    bpm: Some("124.5"),
                    key: Some("10B"),
                    energy: Some(68),
                    transition: Some("Cut"),
                    selected: false,
                },
                TrackRow {
                    title: "Afterimage",
                    artist: Some("Glass Avenue"),
                    time: Some("05:03"),
                    search: Some("afterimage glass avenue"),
                    deck: Some("B"),
                    bpm: Some("126.0"),
                    key: Some("7A"),
                    energy: Some(74),
                    transition: Some("Echo"),
                    selected: false,
                },
            ],
        }
    }
}

impl Reads for FixtureReads {
    fn get(&self, endpoint: &str) -> Option<ReadValue<'_>> {
        let id = endpoint.split_once('@').map_or(endpoint, |(id, _scope)| id);
        match id {
            "deck.playback.tempo" => Some(ReadValue::Text("128.0")),
            "deck.track.title" => Some(ReadValue::Text("Midnight Circuit")),
            "deck.playback.playing" | "deck.playback.looping" | "deck.playback.synced" => {
                Some(ReadValue::Bool(true))
            }
            "deck.playback.reverse" => Some(ReadValue::Bool(false)),
            "deck.playback.position_normalized" => Some(ReadValue::Scalar(0.375)),
            "deck.view.zoom" => Some(ReadValue::Scalar(0.25)),
            "player.output.volume" => Some(ReadValue::Scalar(0.8)),
            "player.output.levels" => Some(ReadValue::Stereo(StereoLevels {
                l: 0.64,
                r: 0.48,
                volume: 0.8,
            })),
            "deck.playback.waveform" => Some(ReadValue::Waveform(WaveformView {
                buckets: &self.buckets,
                beats: &self.beats,
                downbeats: &self.downbeats,
                bpm: Some(128.0),
                r#loop: Some([0.25, 0.5]),
                cues: &self.cues,
            })),
            "library.visible_tracks" => Some(ReadValue::TrackList(&self.tracks)),
            _ => None,
        }
    }
}

fn headless_renderer() -> iced::Renderer {
    let mut fonts = font_system()
        .write()
        .expect("iced font system lock must not be poisoned");
    for bytes in FONT_BYTES {
        fonts.load_font(Cow::Borrowed(bytes));
    }
    drop(fonts);

    FallbackRenderer::Secondary(TinySkiaRenderer::new(SANS, Pixels(14.0)))
}

fn dump(
    preset: &str,
    ui: &CompiledUi,
    reads: &FixtureReads,
    skin: &Skin,
    renderer: &iced::Renderer,
    viewport: Size,
) -> String {
    let mut element = tree::render(&ui.root, ui, reads, skin);
    let mut tree = Tree::new(element.as_widget());
    let node =
        element
            .as_widget_mut()
            .layout(&mut tree, renderer, &Limits::new(Size::ZERO, viewport));
    let mut output = String::new();
    writeln!(
        &mut output,
        "# {preset} @ {:.0}x{:.0}",
        viewport.width, viewport.height
    )
    .expect("writing to a String cannot fail");
    write_layout(&mut output, Layout::new(&node), 0);
    output
}

fn write_layout(output: &mut String, layout: Layout<'_>, depth: usize) {
    let bounds = layout.bounds();
    assert!(
        [bounds.x, bounds.y, bounds.width, bounds.height]
            .iter()
            .all(|value| value.is_finite()),
        "layout contains non-finite bounds: {bounds:?}"
    );
    for _ in 0..depth {
        output.push_str("  ");
    }
    writeln!(
        output,
        "{:.3} {:.3} {:.3} {:.3}",
        bounds.x, bounds.y, bounds.width, bounds.height
    )
    .expect("writing to a String cannot fail");
    for child in layout.children() {
        write_layout(output, child, depth + 1);
    }
}

fn first_difference<'a, 'b>(
    expected: &'a str,
    actual: &'b str,
) -> Option<(usize, &'a str, &'b str)> {
    let mut expected_lines = expected.split('\n');
    let mut actual_lines = actual.split('\n');
    let mut line = 1;
    loop {
        let expected_line = expected_lines.next();
        let actual_line = actual_lines.next();
        match (expected_line, actual_line) {
            (None, None) => return None,
            (Some(expected_line), Some(actual_line)) if expected_line == actual_line => {}
            (expected_line, actual_line) => {
                return Some((
                    line,
                    expected_line.unwrap_or("<end of fixture>"),
                    actual_line.unwrap_or("<end of fixture>"),
                ));
            }
        }
        line += 1;
    }
}

fn assert_fixture_matches(path: &Path, expected: &str, actual: &str) {
    if let Some((line, expected_line, actual_line)) = first_difference(expected, actual) {
        assert_eq!(
            actual_line,
            expected_line,
            "layout fixture mismatch in {} at line {line}\nexpected: {expected_line}\nactual: \
             {actual_line}",
            path.display()
        );
    }
}

#[kithara::test]
fn builtin_layouts_match_rect_fixtures() {
    let reads = FixtureReads::default();
    let renderer = headless_renderer();
    let fixture_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/layout");
    let update = env::var_os("KITHARA_UI_UPDATE_LAYOUT_FIXTURES").is_some();
    if update {
        fs::create_dir_all(&fixture_dir).expect("layout fixture directory must be writable");
    }

    for preset in [builtin::MICRO_PRESET, builtin::PLAYER_PRESET] {
        let ui = compile(
            preset,
            &builtin::resolver(),
            &common::player_registry(),
            builtin::skin_doc(),
            &UiConfig::default(),
        )
        .expect("builtin layout must compile");
        let mut actual = String::new();
        for viewport in [Size::new(1280.0, 720.0), Size::new(960.0, 600.0)] {
            actual.push_str(&dump(
                preset,
                &ui,
                &reads,
                builtin::skin(),
                &renderer,
                viewport,
            ));
        }

        let stem = preset
            .strip_suffix(".klayout.ron")
            .expect("builtin preset must use the klayout suffix");
        let fixture = fixture_dir.join(format!("{stem}.rects"));
        if update {
            fs::write(&fixture, actual).expect("layout fixture must be writable");
        } else {
            let expected =
                fs::read_to_string(&fixture).expect("layout fixture must exist and be readable");
            assert_fixture_matches(&fixture, &expected, &actual);
        }
    }
}
