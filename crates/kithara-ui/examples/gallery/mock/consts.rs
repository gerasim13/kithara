use kithara_ui::module::{TableColumn, TableColumnStyle};

pub(super) struct Consts;

impl Consts {
    pub(super) const BPM: &str = "70.00";
    pub(super) const BPM_VALUE: f32 = 70.0;
    pub(super) const CUES: &[f32] = &[0.27, 0.31];
    pub(super) const DURATION_SECS: f64 = 360.0;
    pub(super) const KEY: &str = "4m";
    pub(super) const LOOP_REGION: [f32; 2] = [0.30, 0.34];
    pub(super) const POSITION_SECS: f64 = 103.0;
    pub(super) const REMAIN: &str = "−04:17";
    pub(super) const TEMPO: &str = "+0.0%";
    pub(super) const TABLE_LIBRARY: [bool; 9] =
        [true, true, true, true, true, true, true, false, false];
    pub(super) const TABLE_MICRO: [bool; 9] =
        [false, false, true, false, false, false, true, false, false];
    pub(super) const TABLE_QUEUE: [bool; 9] =
        [true, true, true, false, true, true, false, true, true];
    pub(super) const TABLE_QUEUE_PRESET: usize = 1;
    pub(super) const VIS_TICK_SECS: f64 = 0.016;
    pub(super) const WAVE_BUCKETS: u32 = 4_096;
    pub(super) const ZOOM: f64 = 0.12;

    pub(super) fn table_columns() -> [TableColumn; 9] {
        [
            TableColumn::new("index", "#", TableColumnStyle::Index, 28.0, false),
            TableColumn::new("deck", "DECK", TableColumnStyle::Badge, 64.0, false),
            TableColumn::new("title", "TITLE", TableColumnStyle::Primary, 180.0, true),
            TableColumn::new(
                "artist",
                "ARTIST",
                TableColumnStyle::Secondary,
                200.0,
                false,
            ),
            TableColumn::new("bpm", "BPM", TableColumnStyle::Metric, 70.0, false),
            TableColumn::new("key", "KEY", TableColumnStyle::Mono, 56.0, false),
            TableColumn::new("time", "TIME", TableColumnStyle::Time, 70.0, false),
            TableColumn::new("energy", "ENERGY", TableColumnStyle::Meter, 110.0, false),
            TableColumn::new(
                "transition",
                "TRANSITION",
                TableColumnStyle::Transition,
                130.0,
                false,
            ),
        ]
    }
}
