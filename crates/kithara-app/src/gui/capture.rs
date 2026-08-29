use std::{
    env,
    fs::{create_dir_all, write},
    path::{Path, PathBuf},
    rc::Rc,
};

use kithara_test_utils::kithara;
use kithara_ui::{
    app::{App, Config, Ui},
    builtin,
    capture::{Geometry, Offscreen, Photographer, write_geometry, write_png},
    draw::PoolStats,
    module::IconName,
    registry::ValueKind,
    render::{
        Clock, PortalMapView, PortalTarget, ReadValue, Reads, ScalarRange, Skin, StereoLevels,
        TableCell, TableRow, TreeRow, UiEvent, WaveBucket, WaveformView, tree,
    },
};

use super::{
    frontend::window_size,
    theme,
    ui::{
        self,
        cache::DeckLayout,
        endpoints::{Registry, readable_kind},
        package::Package,
    },
};
use crate::theme::Palette;

/// The window the studio opens at, which is what both hosts are photographed
/// at so the two sets can be compared at all.
fn studio() -> Geometry {
    let (width, height) = window_size();
    Geometry {
        height,
        scale: 1.0,
        width,
    }
}

struct PoolSample {
    first: PoolStats,
    second: PoolStats,
}

impl PoolSample {
    fn line(&self, page: &str) -> String {
        format!(
            "{page} first_misses={} second_misses={} first_home_hits={} second_home_hits={} \
             first_drops={} second_drops={}\n",
            self.first.alloc_misses,
            self.second.alloc_misses,
            self.first.home_hits,
            self.second.home_hits,
            self.first.put_drops,
            self.second.put_drops,
        )
    }

    fn stable(&self) -> bool {
        self.first.alloc_misses > 0
            && self.first.alloc_misses == self.second.alloc_misses
            && self.first.put_drops == self.second.put_drops
    }
}

struct Fixture {
    layout: DeckLayout,
    rows: Vec<TableRow<'static>>,
    package: Rc<Package>,
}

impl Fixture {
    const BPM: f32 = 124.0;
    const LEVELS: StereoLevels = StereoLevels {
        l: 0.58,
        r: 0.46,
        volume: 0.72,
    };
    const SCALAR: f64 = 0.5;

    /// The settings sheet is open in both captures, because a control only the
    /// sheet carries is compared across the hosts on no page otherwise. Its two
    /// sections cannot show at once, so each layout photographs one of them.
    fn on(&self, endpoint: &str) -> bool {
        match endpoint {
            "deck.eq.three_band" | "ui.settings.open" => true,
            "ui.settings.on_view" => self.layout == DeckLayout::Dual,
            "ui.settings.on_audio" => self.layout == DeckLayout::Single,
            _ => false,
        }
    }

    fn new(layout: DeckLayout, package: Rc<Package>) -> Self {
        Self {
            layout,
            package,
            rows: vec![
                TableRow::new(
                    vec![
                        TableCell::text("deck", "A"),
                        TableCell::text("title", "Midnight Signal"),
                        TableCell::text("artist", "Kithara"),
                    ],
                    true,
                ),
                TableRow::new(
                    vec![
                        TableCell::text("deck", "B"),
                        TableCell::text("title", "Parallel Lines"),
                        TableCell::text("artist", "Studio Fixture"),
                    ],
                    false,
                ),
            ],
        }
    }
}

impl Reads for Fixture {
    fn get(&self, endpoint: &str) -> Option<ReadValue<'_>> {
        /// The browser's source groups, in the order `LibraryView::groups`
        /// lists them. An empty tree would let the page pass its budget while
        /// neither host drew a single row.
        const TREE: [TreeRow<'static>; 3] = [
            TreeRow {
                label: "ALL",
                count: Some(2),
                expanded: None,
                icon: IconName::Collection,
                muted: false,
                selected: false,
                depth: 0,
            },
            TreeRow {
                label: "LOCAL",
                count: Some(2),
                expanded: None,
                icon: IconName::Folder,
                muted: false,
                selected: true,
                depth: 0,
            },
            TreeRow {
                label: "STREAM",
                count: Some(0),
                expanded: None,
                icon: IconName::Playlist,
                muted: false,
                selected: false,
                depth: 0,
            },
        ];
        /// Two decks on the tempo axis. An empty target list would draw an
        /// axis with nothing on it and still pass the page's budget.
        const TEMPOS: [PortalTarget; 2] = [
            PortalTarget {
                bpm: Fixture::BPM,
                is_selected: true,
            },
            PortalTarget {
                bpm: 128.0,
                is_selected: false,
            },
        ];
        const WAVE: [WaveBucket; 8] = [
            WaveBucket {
                high: 0.3,
                low: -0.2,
                mid: 0.05,
            },
            WaveBucket {
                high: 0.6,
                low: -0.4,
                mid: 0.1,
            },
            WaveBucket {
                high: 0.4,
                low: -0.3,
                mid: 0.0,
            },
            WaveBucket {
                high: 0.8,
                low: -0.7,
                mid: 0.08,
            },
            WaveBucket {
                high: 0.5,
                low: -0.4,
                mid: -0.04,
            },
            WaveBucket {
                high: 0.7,
                low: -0.5,
                mid: 0.03,
            },
            WaveBucket {
                high: 0.35,
                low: -0.25,
                mid: 0.0,
            },
            WaveBucket {
                high: 0.55,
                low: -0.45,
                mid: 0.05,
            },
        ];

        let base = endpoint.split_once('@').map_or(endpoint, |(base, _)| base);
        let value = match readable_kind(base)? {
            ValueKind::Bool => ReadValue::Bool(self.on(base)),
            ValueKind::Scalar => ReadValue::Scalar(Self::SCALAR),
            ValueKind::Stereo => ReadValue::Stereo(Self::LEVELS),
            ValueKind::Text => ReadValue::Text(text(base)),
            ValueKind::Waveform => ReadValue::Waveform(WaveformView {
                buckets: &WAVE,
                revision: 0,
                beats: &[],
                cues: &[],
                downbeats: &[],
                unready: &[],
                bpm: Some(Self::BPM),
                r#loop: None,
            }),
            ValueKind::PortalMap => ReadValue::PortalMap(PortalMapView {
                master: Self::BPM,
                min: 60.0,
                max: 200.0,
                targets: &TEMPOS,
            }),
            ValueKind::Range => ReadValue::Range(ScalarRange {
                min: 0.25,
                max: 0.8,
            }),
            ValueKind::Table => ReadValue::Table(&self.rows),
            ValueKind::Tree => ReadValue::Tree(&TREE),
            _ => return None,
        };
        Some(value)
    }
}

fn text(endpoint: &str) -> &'static str {
    match endpoint {
        "deck.playback.bpm" => "124.0",
        "library.breadcrumb" => "LOCAL \u{b7} 2",
        "library.query" => "",
        "deck.playback.remain" => "-03:42",
        "deck.playback.tempo" => "+0.0%",
        "deck.stream.quality" => "320 kbps",
        "broadcast.url" => "OFF AIR",
        "ui.drag.track" => "",
        _ => "Fixture",
    }
}

impl App for Fixture {
    fn skin(&self) -> &Skin {
        self.package.skin()
    }

    fn document(&self) -> &str {
        self.package.document(self.layout)
    }

    fn reads<R>(&self, with: impl FnOnce(&dyn Reads) -> R) -> R {
        with(self)
    }

    fn update(&mut self, _event: UiEvent) {}
}

#[kithara::test]
fn studio_capture_writes_both_hosts() {
    let Some(dir) = env::var_os("KITHARA_STUDIO_CAPTURE").map(PathBuf::from) else {
        return;
    };
    capture(&dir).unwrap_or_else(|error| panic!("studio capture failed: {error}"));
}

fn capture(dir: &Path) -> Result<(), String> {
    let geometry = studio();
    let iced_dir = dir.join("iced");
    let masonry_dir = dir.join("masonry");
    create_dir_all(&iced_dir).map_err(|error| format!("create {}: {error}", iced_dir.display()))?;
    create_dir_all(&masonry_dir)
        .map_err(|error| format!("create {}: {error}", masonry_dir.display()))?;
    write_geometry(&iced_dir, geometry)?;
    write_geometry(&masonry_dir, geometry)?;
    let mut photographer = Photographer::new()?;
    let mut offscreen = Offscreen::new(geometry.width, geometry.height)?;
    let mut iced_stats = String::new();
    let mut masonry_stats = String::new();
    // The retained host fills a buffer this owns, so both pages share one.
    let mut masonry_rgba = Vec::new();

    for (layout, name) in [
        (DeckLayout::Single, "studio-single.png"),
        (DeckLayout::Dual, "studio-dual.png"),
    ] {
        let (iced_rgba, iced_sample) = iced(layout, &mut photographer, geometry)?;
        let masonry_sample = masonry(layout, &mut offscreen, geometry, &mut masonry_rgba)?;
        if !iced_sample.stable() || !masonry_sample.stable() {
            return Err(format!(
                "draw pools allocated again on the second {name} frame: iced={} masonry={}",
                iced_sample.line(name).trim(),
                masonry_sample.line(name).trim(),
            ));
        }
        write_png(
            &iced_dir.join(name),
            &iced_rgba,
            geometry.width,
            geometry.height,
        )?;
        write_png(
            &masonry_dir.join(name),
            &masonry_rgba,
            geometry.width,
            geometry.height,
        )?;
        iced_stats.push_str(&iced_sample.line(name));
        masonry_stats.push_str(&masonry_sample.line(name));
    }
    write(iced_dir.join("draw-pools.txt"), iced_stats)
        .map_err(|error| format!("write iced draw-pools.txt: {error}"))?;
    write(masonry_dir.join("draw-pools.txt"), masonry_stats)
        .map_err(|error| format!("write masonry draw-pools.txt: {error}"))?;
    Ok(())
}

fn iced(
    layout: DeckLayout,
    photographer: &mut Photographer,
    geometry: Geometry,
) -> Result<(Vec<u8>, PoolSample), String> {
    let package = Package::load(None).map_err(|error| format!("package: {error}"))?;
    let compiled = ui::compile_ui(layout)
        .map_err(|error| format!("compile {}: {error}", package.document(layout)))?;
    let reads = Fixture::new(layout, package);
    let skin = builtin::skin();
    let theme = theme::kithara_theme(&Palette::default().into());
    let page = || {
        tree::render(
            &compiled.root,
            &compiled,
            &reads,
            skin,
            Clock::default(),
            None,
        )
    };
    let rgba = photographer.shoot(page(), &theme, geometry)?;
    // The second page is drawn for the pools alone: a frame that allocates
    // again once the pools are warm is the defect this watches for.
    let first = compiled.draw_pool_stats();
    drop(photographer.shoot(page(), &theme, geometry)?);
    Ok((
        rgba,
        PoolSample {
            first,
            second: compiled.draw_pool_stats(),
        },
    ))
}

fn masonry(
    layout: DeckLayout,
    offscreen: &mut Offscreen,
    geometry: Geometry,
    out: &mut Vec<u8>,
) -> Result<PoolSample, String> {
    let package = Package::load(None).map_err(|error| format!("package: {error}"))?;
    let entry = package.document(layout).to_owned();
    let endpoints = Registry::default();
    let mut ui = Ui::new(
        Fixture::new(layout, Rc::clone(&package)),
        Config::builder()
            .endpoints(&endpoints)
            .resolver(package.resolver())
            .text(package.text())
            .build(),
        (geometry.width, geometry.height),
        geometry.scale,
    )
    .map_err(|error| format!("mount {entry}: {error}"))?;
    let frame = ui
        .render()
        .map_err(|error| format!("draw {entry}: {error}"))?;
    offscreen.rasterise(&frame, geometry.scale, ui.background().into(), out)?;
    let first = ui.draw_pool_stats();
    drop(
        ui.render()
            .map_err(|error| format!("second draw {entry}: {error}"))?,
    );
    Ok(PoolSample {
        first,
        second: ui.draw_pool_stats(),
    })
}
