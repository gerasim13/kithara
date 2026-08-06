/// Algorithmic latency in the source and output coordinate spaces.
#[derive(Clone, Copy, Debug, Eq, PartialEq, fieldwork::Fieldwork)]
#[fieldwork(get)]
#[non_exhaustive]
pub struct ElasticLatency {
    /// Delayed output in frames.
    #[field(get, copy)]
    output_frames: usize,
    /// Required source history in frames.
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
