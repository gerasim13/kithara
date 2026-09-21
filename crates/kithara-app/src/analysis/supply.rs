use kithara::{
    analysis::{AnalysisDemand, AnalysisFingerprint, AnalysisProgress, BeatGridModel},
    platform::sync::Arc,
    prelude::ArtifactSource,
    waveform::Waveform,
};

use crate::pools::AppResourceConfig;

/// What a track already holds for one artifact, before any analysis runs.
#[derive(Clone, Debug, Default)]
pub(crate) enum Supply<T> {
    /// A value the caller handed over, ready to publish.
    Ready(Arc<T>),
    /// Nothing prepared: this artifact is analysis's to produce.
    #[default]
    Missing,
}

impl<T> Supply<T> {
    /// Whether analysis still owes this artifact.
    pub(crate) const fn is_missing(&self) -> bool {
        matches!(self, Self::Missing)
    }

    pub(crate) const fn value(&self) -> Option<&Arc<T>> {
        match self {
            Self::Ready(value) => Some(value),
            Self::Missing => None,
        }
    }
}

impl<T> From<Option<&ArtifactSource<T>>> for Supply<T> {
    fn from(source: Option<&ArtifactSource<T>>) -> Self {
        match source {
            Some(ArtifactSource::Value(value)) => Self::Ready(Arc::clone(value)),
            _ => Self::Missing,
        }
    }
}

/// Every prepared artifact one track was opened with.
#[derive(Clone, Debug, Default)]
pub(crate) struct Prepared {
    pub(crate) beat_grid: Supply<BeatGridModel>,
    pub(crate) waveform: Supply<Waveform>,
}

impl Prepared {
    pub(crate) fn for_config(config: &AppResourceConfig) -> Self {
        Self {
            beat_grid: config.beat_grid().into(),
            waveform: config.waveform().into(),
        }
    }

    /// Whether the track was opened with nothing prepared.
    pub(crate) const fn is_empty(&self) -> bool {
        self.beat_grid.is_missing() && self.waveform.is_missing()
    }

    /// The artifacts a pass is opened for: what this runtime can analyse at
    /// all, minus what the track already carries. A checkpoint does not narrow
    /// this — a resumed pass continues the very artifacts it was opened for.
    pub(crate) fn demand(&self, fingerprint: &AnalysisFingerprint) -> AnalysisDemand {
        AnalysisDemand::new(
            fingerprint.beat().is_some() && self.beat_grid.is_missing(),
            fingerprint.waveform().is_some() && self.waveform.is_missing(),
        )
    }

    /// Whether the track needs nothing further: the pass ran its course and
    /// every artifact this runtime offers is either prepared or published.
    /// A prepared artifact excuses the pass from producing it.
    pub(crate) fn settled_for(
        &self,
        progress: &AnalysisProgress,
        fingerprint: &AnalysisFingerprint,
    ) -> bool {
        let analysis = progress.analysis();
        let beat = !self.beat_grid.is_missing()
            || fingerprint.beat().is_none()
            || analysis.beat().is_some();
        let waveform = !self.waveform.is_missing()
            || fingerprint.waveform().is_none()
            || analysis.waveform().is_some();
        analysis.is_settled() && beat && waveform
    }
}
