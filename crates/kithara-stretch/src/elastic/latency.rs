pub enum ElasticLatencyTag {}

/// Unity-rate latency split between source and output coordinates. Unprimed
/// startup is their sum; [`prime`](crate::ElasticEngine::prime) absorbs it.
pub type ElasticLatency = kithara_signal::FramePair<ElasticLatencyTag>;
