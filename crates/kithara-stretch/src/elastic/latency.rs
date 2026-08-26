/// Unity-rate algorithmic latency in the source and output coordinate spaces.
#[derive(Clone, Copy, Debug, Eq, PartialEq, fieldwork::Fieldwork)]
#[fieldwork(get)]
#[non_exhaustive]
pub struct ElasticLatency {
    /// Unity-rate delayed output in frames.
    #[field(get, copy)]
    output_frames: usize,
    /// Unity-rate source history in frames.
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
