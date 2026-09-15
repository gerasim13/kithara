use kithara::queue::TrackId;

use crate::pools::Pools;

pub(crate) struct AnalysisRuns;

impl AnalysisRuns {
    pub(crate) fn new(_pools: Pools) -> Self {
        Self
    }

    pub(crate) const fn cancel(&mut self, _id: TrackId) {}

    pub(crate) const fn clear(&mut self) {}
}
