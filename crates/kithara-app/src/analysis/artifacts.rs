use kithara::{
    analysis::{AnalysisToken, BeatGridModel, TrackAnalysis, Waveform},
    platform::sync::Arc,
};

use super::supply::Prepared;

/// Identity of the waveform a publication carries. A supplied waveform is
/// identified by the value the caller handed over, which never changes under
/// the track; an analysed one by the pass revision that produced it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum WaveformId {
    Supplied(usize),
    Analysed(AnalysisToken, u64),
}

/// One track's published artifacts, whatever their origin: a beat grid or a
/// waveform the caller supplied, a local analysis result, or one of each.
///
/// A consumer asks for an artifact, not for where it came from. A prepared
/// value outranks an analysed one for the same artifact: analysis is only ever
/// opened for what no prepared value covers, so the two cannot disagree.
#[derive(Clone, Debug, Default)]
pub(crate) struct TrackArtifacts {
    analysis: Option<TrackAnalysis>,
    prepared: Prepared,
}

impl TrackArtifacts {
    pub(crate) fn new(analysis: Option<TrackAnalysis>, prepared: Prepared) -> Self {
        Self { analysis, prepared }
    }

    /// The beat grid to paint and to clock against.
    pub(crate) fn grid(&self) -> Option<&BeatGridModel> {
        self.prepared
            .beat_grid
            .value()
            .map(Arc::as_ref)
            .or_else(|| self.analysis.as_ref().and_then(TrackAnalysis::grid))
    }

    /// The waveform to draw.
    pub(crate) fn waveform(&self) -> Option<&Waveform> {
        self.prepared
            .waveform
            .value()
            .map(Arc::as_ref)
            .or_else(|| self.analysis.as_ref().and_then(TrackAnalysis::waveform))
    }

    /// What names the waveform this publication draws.
    pub(crate) fn waveform_id(&self) -> Option<WaveformId> {
        self.prepared
            .waveform
            .value()
            .map(|waveform| WaveformId::Supplied(Arc::as_ptr(waveform).cast::<()>() as usize))
            .or_else(|| {
                self.analysis
                    .as_ref()
                    .filter(|analysis| analysis.waveform().is_some())
                    .map(|analysis| {
                        WaveformId::Analysed(analysis.token().clone(), analysis.revision())
                    })
            })
    }

    /// The local analysis result, when a pass produced one. Facts that belong
    /// to the pass itself — coverage, extent, what it settled on — are read
    /// from here; artifacts are read from this type instead, which does not
    /// care which origin served them.
    pub(crate) const fn analysis(&self) -> Option<&TrackAnalysis> {
        self.analysis.as_ref()
    }
}

impl From<TrackAnalysis> for TrackArtifacts {
    fn from(analysis: TrackAnalysis) -> Self {
        Self::new(Some(analysis), Prepared::default())
    }
}
