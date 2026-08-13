use crate::render::{WaveBucket, WaveformView};

#[derive(PartialEq)]
pub(crate) struct WaveformData {
    pub(crate) buckets: Box<[WaveBucket]>,
    pub(crate) beats: Box<[f32]>,
    pub(crate) downbeats: Box<[f32]>,
    pub(crate) loop_region: Option<[f32; 2]>,
    pub(crate) cues: Box<[f32]>,
}

impl From<WaveformView<'_>> for WaveformData {
    fn from(view: WaveformView<'_>) -> Self {
        Self {
            buckets: view.buckets.to_vec().into_boxed_slice(),
            beats: view.beats.to_vec().into_boxed_slice(),
            downbeats: view.downbeats.to_vec().into_boxed_slice(),
            loop_region: view.r#loop,
            cues: view.cues.to_vec().into_boxed_slice(),
        }
    }
}

impl WaveformData {
    #[cfg(any(feature = "masonry", test))]
    pub(crate) fn matches(&self, view: WaveformView<'_>) -> bool {
        self.buckets.as_ref() == view.buckets
            && self.beats.as_ref() == view.beats
            && self.downbeats.as_ref() == view.downbeats
            && self.loop_region == view.r#loop
            && self.cues.as_ref() == view.cues
    }
}

#[derive(PartialEq)]
pub(crate) struct OverlayData {
    pub(crate) title: String,
    pub(crate) artist: String,
    pub(crate) bpm: String,
    pub(crate) key: String,
    pub(crate) remain: String,
    pub(crate) badge: String,
}
