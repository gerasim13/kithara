use std::sync::LazyLock;

use kithara_ui::{
    module::IconName,
    render::{TableCell, TableRow, TreeRow},
};
use serde::Deserialize;

#[derive(Deserialize)]
struct DemoData {
    vis_indices: (String, String, String),
    vis_presets: (String, String, String),
    pivot: PivotCopy,
    artist: String,
    breadcrumb: String,
    footer_tokens_anatomy: String,
    title: String,
    tracks: Vec<DemoTrack>,
    tree: Vec<DemoTreeRow>,
}

/// Pivot copy lives in the asset so the sources stay ASCII; `{name}` slots are
/// filled by `demo::pivot`.
#[derive(Deserialize)]
pub struct PivotCopy {
    pub duration: String,
    pub hint: String,
    pub hint_empty: String,
    pub pulse: String,
    pub pulse_empty: String,
    pub tracks: String,
    pub tracks_empty: String,
}

#[derive(Deserialize)]
struct DemoTreeRow {
    icon: IconName,
    #[serde(default)]
    count: Option<u32>,
    #[serde(default)]
    expanded: Option<bool>,
    label: String,
    #[serde(default)]
    muted: bool,
    #[serde(default)]
    selected: bool,
    #[serde(default)]
    depth: u8,
}

#[derive(Deserialize)]
struct DemoTrack {
    artist: String,
    bpm: String,
    deck: String,
    key: String,
    search: String,
    time: String,
    title: String,
    transition: String,
    energy: u8,
}

pub struct Catalog {
    pub pivot: &'static PivotCopy,
    /// The same tracks, played through enough times that the table overflows
    /// its viewport. Nothing else in the gallery makes a table scroll, and a
    /// table that never scrolls hides what a file list actually costs.
    pub long_rows: &'static [TableRow<'static>],
    pub rows: &'static [TableRow<'static>],
    pub tree: &'static [TreeRow<'static>],
    pub artist: &'static str,
    pub breadcrumb: &'static str,
    pub footer_tokens_anatomy: &'static str,
    pub title: &'static str,
    pub vis_indices: [&'static str; 3],
    pub vis_presets: [&'static str; 3],
}

pub static CATALOG: LazyLock<Catalog> = LazyLock::new(load_catalog);

fn load_catalog() -> Catalog {
    /// How many rows the long table carries: a little over four screens at the
    /// gallery's size, which is enough to scroll and short of the stress page.
    const LONG_ROWS: usize = 120;

    let data: DemoData = ron::from_str(include_str!("../../assets/demo-data.ron"))
        .expect("embedded gallery demo data must parse");
    let data: &'static DemoData = Box::leak(Box::new(data));
    let rows: Vec<TableRow<'static>> = data
        .tracks
        .iter()
        .enumerate()
        .map(|(index, track)| track_row(track, &track.title, index == 0))
        .collect();
    let takes = data.tracks.len();
    let titles: &'static [String] = Box::leak(
        (0..LONG_ROWS)
            .map(|index| {
                format!(
                    "{} {:02}",
                    data.tracks[index % takes].title,
                    index / takes + 1
                )
            })
            .collect::<Vec<_>>()
            .into_boxed_slice(),
    );
    let long_rows: Vec<TableRow<'static>> = titles
        .iter()
        .enumerate()
        .map(|(index, title)| track_row(&data.tracks[index % takes], title, index == 0))
        .collect();
    let tree: Vec<TreeRow<'static>> = data
        .tree
        .iter()
        .map(|row| TreeRow {
            depth: row.depth,
            label: &row.label,
            icon: row.icon,
            count: row.count,
            expanded: row.expanded,
            selected: row.selected,
            muted: row.muted,
        })
        .collect();
    Catalog {
        title: &data.title,
        artist: &data.artist,
        breadcrumb: &data.breadcrumb,
        footer_tokens_anatomy: &data.footer_tokens_anatomy,
        pivot: &data.pivot,
        vis_indices: [
            data.vis_indices.0.as_str(),
            data.vis_indices.1.as_str(),
            data.vis_indices.2.as_str(),
        ],
        vis_presets: [
            data.vis_presets.0.as_str(),
            data.vis_presets.1.as_str(),
            data.vis_presets.2.as_str(),
        ],
        long_rows: Box::leak(long_rows.into_boxed_slice()),
        rows: Box::leak(rows.into_boxed_slice()),
        tree: Box::leak(tree.into_boxed_slice()),
    }
}

/// One table row for `track`, shown under `title` so a repeated catalogue does
/// not read as the same track over and over.
fn track_row(track: &'static DemoTrack, title: &'static str, selected: bool) -> TableRow<'static> {
    TableRow::new(
        vec![
            TableCell::text("title", title),
            TableCell::text("artist", &track.artist),
            TableCell::text("time", &track.time),
            TableCell::text("search", &track.search),
            TableCell::text("deck", &track.deck),
            TableCell::text("bpm", &track.bpm),
            TableCell::text("key", &track.key),
            TableCell::number("energy", track.energy),
            TableCell::text("transition", &track.transition),
        ],
        selected,
    )
}
