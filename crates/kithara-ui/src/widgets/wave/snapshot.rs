use crate::render::WaveBucket;

pub(super) struct WaveformData {
    pub(super) buckets: Box<[WaveBucket]>,
    pub(super) beats: Box<[f32]>,
    pub(super) downbeats: Box<[f32]>,
    pub(super) loop_region: Option<[f32; 2]>,
    pub(super) cues: Box<[f32]>,
}

pub(super) struct OverlayData {
    pub(super) title: String,
    pub(super) artist: String,
    pub(super) bpm: String,
    pub(super) key: String,
    pub(super) remain: String,
    pub(super) badge: String,
}
