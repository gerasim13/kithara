use std::{
    f64::consts::TAU,
    fs,
    num::NonZeroU32,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use kithara::{
    audio::{BeatGrid, analysis::TrackAnalysis},
    queue::TrackSource,
};

use super::{CHANNELS, SyncCase};
use crate::{
    TestTempDir,
    signal_pcm::{Finite, SignalPcm, signal::SignalFn},
    wav::create_wav_from_signal,
};

const TRACK_SECONDS: usize = 30;
const TEMPOS: [f64; 4] = [96.0, 108.0, 132.0, 144.0];
const TONES: [f64; 4] = [220.0, 880.0, 1_760.0, 3_520.0];

#[derive(Clone, Copy)]
struct PulseTrack {
    bpm: f64,
    tone_hz: f64,
}

impl SignalFn for PulseTrack {
    fn sample(&self, frame: usize, sample_rate: u32) -> i16 {
        let beat_frames = (f64::from(sample_rate) * 60.0 / self.bpm).round() as usize;
        let into_beat = frame % beat_frames;
        let burst_frames = beat_frames / 10;
        if into_beat >= burst_frames {
            return 0;
        }
        let decay = 1.0 - into_beat as f64 / burst_frames as f64;
        let phase = TAU * self.tone_hz * into_beat as f64 / f64::from(sample_rate);
        (phase.sin() * decay * decay * f64::from(i16::MAX) * 0.6) as i16
    }
}

pub(super) struct SyntheticFixture {
    _temp: TestTempDir,
    tracks: Vec<SyntheticTrack>,
}

struct SyntheticTrack {
    analysis: TrackAnalysis,
    bpm: f64,
    path: PathBuf,
    tone_hz: f64,
}

impl SyntheticFixture {
    pub(super) fn new(case: SyncCase) -> Result<Self> {
        if !(2..=4).contains(&case.decks) {
            bail!("{case}: synthetic matrix supports two to four decks");
        }
        let temp = TestTempDir::new();
        let tracks = TEMPOS
            .iter()
            .zip(TONES)
            .enumerate()
            .take(case.decks)
            .map(|(index, (&bpm, tone_hz))| {
                let path = temp
                    .path()
                    .join(format!("sync-{index}-{bpm:.0}-{}.wav", case.sample_rate));
                write_pulse(&path, bpm, tone_hz, case.sample_rate)?;
                Ok(SyntheticTrack {
                    analysis: synthetic_analysis(bpm, case.sample_rate)?,
                    bpm,
                    path,
                    tone_hz,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(Self {
            _temp: temp,
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
                        track.path.display().to_string(),
                        track.path.to_string_lossy().into_owned(),
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

fn write_pulse(path: &Path, bpm: f64, tone_hz: f64, sample_rate: u32) -> Result<()> {
    let track_frames = sample_rate as usize * TRACK_SECONDS;
    let pcm = SignalPcm::new(
        PulseTrack { bpm, tone_hz },
        sample_rate,
        CHANNELS,
        Finite::new(track_frames),
    );
    fs::write(path, create_wav_from_signal(pcm))
        .with_context(|| format!("write deterministic pulse WAV '{}'", path.display()))
}

fn synthetic_analysis(bpm: f64, sample_rate: u32) -> Result<TrackAnalysis> {
    let track_frames = sample_rate as usize * TRACK_SECONDS;
    let rate = NonZeroU32::new(sample_rate).context("fixture sample rate must be non-zero")?;
    let beat_frames = (f64::from(sample_rate) * 60.0 / bpm).round() as u64;
    let markers = (0..=track_frames as u64 / beat_frames)
        .map(|beat| beat * beat_frames)
        .collect::<Vec<_>>();
    let downbeats = markers.iter().step_by(4).copied().collect();
    Ok(TrackAnalysis::with_source_rate(
        Some(BeatGrid::new(bpm, markers, downbeats, Vec::new())),
        None,
        track_frames as u64,
        rate,
    ))
}
