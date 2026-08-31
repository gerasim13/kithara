//! The reads every studio page is drawn against.
//!
//! One fixture answers every endpoint both hosts ask for, so the two sets
//! differ in how they draw and never in what they were given to draw.

use std::rc::Rc;

use kithara::ui::{
    app::App,
    module::IconName,
    registry::ValueKind,
    render::{
        PortalMapView, PortalTarget, ReadValue, Reads, ScalarRange, Skin, StereoLevels, TableCell,
        TableRow, TreeRow, UiEvent, WaveBucket, WaveformView,
    },
};

use crate::gui::ui::{cache::DeckLayout, endpoints::readable_kind, package::Package};

pub(super) struct Fixture {
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

    pub(super) fn new(layout: DeckLayout, package: Rc<Package>) -> Self {
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
