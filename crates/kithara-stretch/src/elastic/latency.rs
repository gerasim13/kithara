/// Unity-rate latency split between source and output coordinates. Unprimed
/// startup is their sum; [`prime`](crate::ElasticEngine::prime) absorbs it.
#[derive(Clone, Copy, Debug, Eq, PartialEq, fieldwork::Fieldwork)]
#[fieldwork(get)]
#[non_exhaustive]
pub struct ElasticLatency {
    /// Output frames between the native processing center and emitted audio.
    #[field(get, copy)]
    output_frames: usize,
    /// Source frames of native history/lookahead around the processing center.
    #[field(get, copy)]
    source_frames: usize,
}

impl ElasticLatency {
    pub(crate) const fn new(source_frames: usize, output_frames: usize) -> Self {
        Self {
            output_frames,
            source_frames,
        }
    }
}
