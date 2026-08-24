use std::num::NonZeroU32;

use anyhow::{Context, Result, bail};
use kithara::{
    audio::{BeatGrid, analysis::TrackAnalysis},
    queue::TrackSource,
};

use super::{CHANNELS, SyncCase};
use crate::{SignalFormat, SignalSpec, SignalSpecLength, TestServerHelper};

pub(super) struct SyntheticFixture {
    _server: TestServerHelper,
    tracks: Vec<SyntheticTrack>,
}

struct SyntheticTrack {
    analysis: TrackAnalysis,
    bpm: f64,
    source: String,
    tone_hz: f64,
}

impl SyntheticFixture {
    pub(super) async fn new(case: SyncCase) -> Result<Self> {
        if case.signal_tracks.len() < 2 {
            bail!("{case}: synthetic matrix requires at least two signal tracks");
        }
        let server = TestServerHelper::new().await;
        let spec = SignalSpec {
            format: SignalFormat::Flac,
            length: SignalSpecLength::Seconds(case.signal_seconds as f64),
            channels: CHANNELS,
            sample_rate: case.sample_rate,
            bit_rate: None,
        };
        let mut tracks = Vec::with_capacity(case.signal_tracks.len());
        for signal in case.signal_tracks {
            let source = server.rhythmic_mix(&spec, &[*signal]).await.to_string();
            tracks.push(SyntheticTrack {
                analysis: synthetic_analysis(signal.bpm, case.sample_rate, case.signal_seconds)?,
                bpm: signal.bpm,
                source,
                tone_hz: signal.tone_hz,
            });
        }
        Ok(Self {
            _server: server,
            tracks,
        })
    }

    pub(super) fn media(&self) -> SyncMedia {
        SyncMedia::new(
            "synthetic-pulse",
            self.tracks
                .iter()
                .map(|track| {
                    SyncTrackFixture::new(
                        track.source.clone(),
                        track.source.clone(),
                        track.analysis.clone(),
                        format!("synthetic:{}:{}", track.bpm, track.tone_hz),
                    )
                })
                .collect(),
        )
    }
}

#[derive(Clone)]
#[non_exhaustive]
pub struct SyncTrackFixture {
    pub(super) abr_target: Option<usize>,
    pub(super) analysis: TrackAnalysis,
    pub(super) analysis_key: String,
    pub(super) label: String,
    pub(super) source: TrackSource,
}

impl SyncTrackFixture {
    #[must_use]
    pub fn new(
        label: impl Into<String>,
        source: impl Into<TrackSource>,
        analysis: TrackAnalysis,
        analysis_key: impl Into<String>,
    ) -> Self {
        Self {
            abr_target: None,
            analysis,
            analysis_key: analysis_key.into(),
            label: label.into(),
            source: source.into(),
        }
    }

    #[must_use]
    pub const fn with_abr_target(mut self, target: usize) -> Self {
        self.abr_target = Some(target);
        self
    }

    pub(super) fn bpm(&self) -> Option<f64> {
        self.analysis.beat().map(BeatGrid::bpm)
    }
}

#[derive(Clone)]
#[non_exhaustive]
pub struct SyncMedia {
    pub(super) id: String,
    pub(super) library_seed: Option<u64>,
    pub(super) tracks: Vec<SyncTrackFixture>,
}

impl SyncMedia {
    #[must_use]
    pub fn new(id: impl Into<String>, tracks: Vec<SyncTrackFixture>) -> Self {
        Self {
            id: id.into(),
            library_seed: None,
            tracks,
        }
    }

    #[must_use]
    pub const fn with_library_seed(mut self, seed: u64) -> Self {
        self.library_seed = Some(seed);
        self
    }

    pub(super) fn validate(&self, case: SyncCase) -> Result<()> {
        if self.tracks.is_empty() {
            bail!("{case}: media '{}' has no tracks", self.id);
        }
        for (index, track) in self.tracks.iter().enumerate() {
            if track.bpm().is_none() {
                bail!(
                    "{case}: media '{}' track {index} ('{}') has no beat grid",
                    self.id,
                    track.label,
                );
            }
        }
        Ok(())
    }

    pub(super) fn for_deck(&self, deck: usize) -> &SyncTrackFixture {
        &self.tracks[deck % self.tracks.len()]
    }
}

fn synthetic_analysis(bpm: f64, sample_rate: u32, signal_seconds: usize) -> Result<TrackAnalysis> {
    let track_seconds =
        u64::try_from(signal_seconds).context("fixture duration must fit the source axis")?;
    let track_frames = u64::from(sample_rate) * track_seconds;
    let rate = NonZeroU32::new(sample_rate).context("fixture sample rate must be non-zero")?;
    let beat_frames = (f64::from(sample_rate) * 60.0 / bpm).round() as u64;
    let markers = (0..=track_frames / beat_frames)
        .map(|beat| beat * beat_frames)
        .take_while(|frame| *frame < track_frames)
        .collect::<Vec<_>>();
    let downbeats = markers.iter().step_by(4).copied().collect();
    Ok(TrackAnalysis::with_source_rate(
        Some(BeatGrid::new(bpm, markers, downbeats, Vec::new())),
        None,
        track_frames,
        rate,
    ))
}
