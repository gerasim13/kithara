use std::num::NonZeroU32;

use anyhow::{Context, Result, bail};
use kithara::{
    audio::{BeatMapId, BeatMapRevision, BeatOrdinal, MapStamp, Meter, ReadOutcome},
    platform::time::{self, Duration},
    play::{Resource, ResourceConfig, ResourceSrc},
};

use super::{
    CHANNELS, COCHLEA_PHASE_SPREAD_BUDGET_FRAMES, CaptureBundle, LockedPhaseObservation,
    PcmCapture, RENDER_FRAMES, RhythmicTrack, SignalEvidence, SignalOracle, SignalOracleReport,
    SyncCase, run_synthetic_behavioral_row,
};
use crate::{
    SignalFormat, SignalSpec, SignalSpecLength, TestServerHelper,
    sync_fixture::SyncFixtureResources,
};

const ASSET_TIMEOUT: Duration = Duration::from_secs(30);
const OUT_OF_SYNC_OFFSET_FRAMES: usize = COCHLEA_PHASE_SPREAD_BUDGET_FRAMES as usize * 2;

/// A deterministic defect injected by the prepared-asset provider.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum SignalDefect {
    None,
    OneFrame,
    BeatOrdinal,
    BarPhase,
    Drift,
    Discontinuity,
    OutOfSync,
}

/// Captures prepared media directly, without the Player/Queue chain.
#[derive(Clone, Copy, Debug)]
pub struct AssetProvider {
    defect: SignalDefect,
}

impl AssetProvider {
    #[must_use]
    pub const fn new(defect: SignalDefect) -> Self {
        Self { defect }
    }
}

/// Captures the same case through the production Player/Queue chain.
#[derive(Clone, Copy, Debug, Default)]
pub struct PlayerQueueProvider;

#[derive(Debug)]
enum SignalCaptureKind {
    Evidence(Box<SignalEvidence>),
    PlayerQueue(Box<CaptureBundle>),
}

/// A provider result exposing shared audio evidence and optional runtime facts.
#[derive(Debug)]
pub struct SignalCapture(SignalCaptureKind);

impl SignalCapture {
    fn from_evidence(evidence: SignalEvidence) -> Self {
        Self(SignalCaptureKind::Evidence(Box::new(evidence)))
    }

    fn from_player_queue(bundle: CaptureBundle) -> Self {
        Self(SignalCaptureKind::PlayerQueue(Box::new(bundle)))
    }

    #[must_use]
    pub const fn evidence(&self) -> &SignalEvidence {
        match &self.0 {
            SignalCaptureKind::Evidence(evidence) => evidence,
            SignalCaptureKind::PlayerQueue(bundle) => &bundle.signal,
        }
    }

    pub fn into_player_queue(self) -> Result<CaptureBundle> {
        match self.0 {
            SignalCaptureKind::PlayerQueue(bundle) => Ok(*bundle),
            SignalCaptureKind::Evidence(_) => {
                bail!("signal provider did not exercise Player/Queue")
            }
        }
    }
}

/// Produces oracle-ready signal evidence for one behavioral matrix case.
#[async_trait::async_trait]
pub trait SignalProvider {
    async fn capture(
        self,
        case: SyncCase,
        resources: SyncFixtureResources,
    ) -> Result<SignalCapture>;
}

#[async_trait::async_trait]
impl SignalProvider for PlayerQueueProvider {
    async fn capture(
        self,
        case: SyncCase,
        resources: SyncFixtureResources,
    ) -> Result<SignalCapture> {
        run_synthetic_behavioral_row(case, resources)
            .await
            .map(SignalCapture::from_player_queue)
    }
}

#[async_trait::async_trait]
impl SignalProvider for AssetProvider {
    async fn capture(
        self,
        case: SyncCase,
        resources: SyncFixtureResources,
    ) -> Result<SignalCapture> {
        let server = TestServerHelper::new().await;
        let control_tracks = aligned_tracks(case);
        let candidate_tracks = defective_tracks(case, self.defect);
        let continuity_tracks = match self.defect {
            SignalDefect::Drift | SignalDefect::OutOfSync => &candidate_tracks,
            SignalDefect::None
            | SignalDefect::OneFrame
            | SignalDefect::BeatOrdinal
            | SignalDefect::BarPhase
            | SignalDefect::Discontinuity => &control_tracks,
        };
        let pre_sync_tracks = staggered_tracks(case);

        let control_mix = capture_prepared(
            &server,
            case,
            "asset-control-mix",
            continuity_tracks,
            &resources,
        )
        .await?;
        let mix = capture_prepared(
            &server,
            case,
            "asset-candidate-mix",
            &candidate_tracks,
            &resources,
        )
        .await?;
        let control_replays = capture_tracks(
            &server,
            case,
            "asset-control-deck",
            continuity_tracks,
            &resources,
        )
        .await?;
        let deck_replays = capture_tracks(
            &server,
            case,
            "asset-candidate-deck",
            &candidate_tracks,
            &resources,
        )
        .await?;
        let pre_sync_replays = capture_tracks(
            &server,
            case,
            "asset-pre-sync-deck",
            &pre_sync_tracks,
            &resources,
        )
        .await?;

        Ok(SignalCapture::from_evidence(SignalEvidence {
            control_mix,
            control_replays,
            deck_replays,
            mix,
            phase_observations: phase_observations(case, self.defect)?,
            pre_sync_replays,
        }))
    }
}

/// Capture a case through a provider and evaluate its shared signal evidence.
pub async fn evaluate_signal<P>(
    provider: P,
    case: SyncCase,
    resources: SyncFixtureResources,
) -> Result<SignalOracleReport>
where
    P: SignalProvider,
{
    let capture = provider.capture(case, resources).await?;
    Ok(SignalOracle::evaluate(case, capture.evidence()))
}

fn aligned_tracks(case: SyncCase) -> Vec<RhythmicTrack> {
    let lead_in = beat_frames(case.sample_rate, case.tempo_ride.final_bpm());
    case.signal_tracks
        .iter()
        .map(|track| {
            RhythmicTrack::new(case.tempo_ride.final_bpm(), track.tone_hz)
                .with_phase_frames(lead_in)
        })
        .collect()
}

fn defective_tracks(case: SyncCase, defect: SignalDefect) -> Vec<RhythmicTrack> {
    let mut tracks = aligned_tracks(case);
    let Some(track) = tracks.get_mut(1) else {
        return tracks;
    };
    let lead_in = track.phase_frames;
    *track = match defect {
        SignalDefect::Drift => RhythmicTrack::new(case.tempo_ride.final_bpm() + 2.0, track.tone_hz)
            .with_phase_frames(lead_in),
        SignalDefect::Discontinuity => track.with_muted_beat(3),
        SignalDefect::OutOfSync => {
            track.with_phase_frames(lead_in.saturating_add(OUT_OF_SYNC_OFFSET_FRAMES))
        }
        SignalDefect::OneFrame
        | SignalDefect::BeatOrdinal
        | SignalDefect::BarPhase
        | SignalDefect::None => *track,
    };
    tracks
}

fn staggered_tracks(case: SyncCase) -> Vec<RhythmicTrack> {
    let beat_frames = beat_frames(case.sample_rate, case.session_bpm);
    case.signal_tracks
        .iter()
        .enumerate()
        .map(|(deck, track)| {
            let stagger = (beat_frames as f64 * case.stagger_beats * deck as f64).round() as usize;
            let phase = beat_frames.saturating_add(stagger);
            RhythmicTrack::new(case.session_bpm, track.tone_hz).with_phase_frames(phase)
        })
        .collect()
}

fn beat_frames(sample_rate: u32, bpm: f64) -> usize {
    (f64::from(sample_rate) * 60.0 / bpm).round() as usize
}

async fn capture_tracks(
    server: &TestServerHelper,
    case: SyncCase,
    label: &str,
    tracks: &[RhythmicTrack],
    resources: &SyncFixtureResources,
) -> Result<Vec<PcmCapture>> {
    let mut captures = Vec::with_capacity(tracks.len());
    for (deck, track) in tracks.iter().copied().enumerate() {
        captures.push(
            capture_prepared(
                server,
                case,
                &format!("{label}-{deck}"),
                &[track],
                resources,
            )
            .await?,
        );
    }
    Ok(captures)
}

async fn capture_prepared(
    server: &TestServerHelper,
    case: SyncCase,
    label: &str,
    tracks: &[RhythmicTrack],
    resources: &SyncFixtureResources,
) -> Result<PcmCapture> {
    let spec = SignalSpec {
        format: SignalFormat::Flac,
        length: SignalSpecLength::Seconds(case.signal_seconds as f64),
        channels: CHANNELS,
        sample_rate: case.sample_rate,
        bit_rate: None,
    };
    let url = server.rhythmic_mix(&spec, tracks).await;
    let config: ResourceConfig = ResourceConfig::for_src(ResourceSrc::Url(url))
        .store(resources.store().clone())
        .byte_pool(resources.byte_pool().clone())
        .pcm_pool(resources.pcm_pool().clone())
        .host_sample_rate(
            NonZeroU32::new(case.sample_rate).context("asset sample rate must be non-zero")?,
        )
        .build();
    let mut resource = time::timeout(ASSET_TIMEOUT, Resource::new(config))
        .await
        .with_context(|| format!("{case}: opening rhythmic asset '{label}' timed out"))?
        .with_context(|| format!("{case}: open prepared rhythmic asset '{label}'"))?;
    time::timeout(ASSET_TIMEOUT, resource.preload())
        .await
        .with_context(|| format!("{case}: preloading rhythmic asset '{label}' timed out"))?
        .with_context(|| format!("{case}: preload prepared rhythmic asset '{label}'"))?;
    let samples = time::timeout(
        ASSET_TIMEOUT,
        read_pcm(&mut resource, case.capture_frames(), label),
    )
    .await
    .with_context(|| format!("{case}: reading rhythmic asset '{label}' timed out"))??;
    Ok(PcmCapture {
        channels: CHANNELS,
        label: label.to_owned(),
        sample_rate: case.sample_rate,
        samples,
        start_session_frame: 0,
    })
}

async fn read_pcm(resource: &mut Resource, frames: usize, label: &str) -> Result<Vec<f32>> {
    if resource.spec().channels != CHANNELS {
        bail!(
            "prepared asset '{label}' has {} channels, expected {CHANNELS}",
            resource.spec().channels
        );
    }
    let mut samples = Vec::with_capacity(frames.saturating_mul(usize::from(CHANNELS)));
    let mut left = vec![0.0_f32; RENDER_FRAMES];
    let mut right = vec![0.0_f32; RENDER_FRAMES];
    loop {
        let completed = samples.len() / usize::from(CHANNELS);
        if completed >= frames {
            return Ok(samples);
        }
        let requested = (frames - completed).min(RENDER_FRAMES);
        let mut planar = [&mut left[..requested], &mut right[..requested]];
        match resource.read_planar(&mut planar)? {
            ReadOutcome::Frames { count, .. } => {
                for frame in 0..count.get() {
                    samples.push(left[frame]);
                    samples.push(right[frame]);
                }
            }
            ReadOutcome::Pending { .. } => time::sleep(Duration::from_millis(1)).await,
            ReadOutcome::Eof { position } => bail!(
                "prepared asset '{label}' reached EOF at {:.6}s after {completed}/{frames} frames",
                position.as_secs_f64()
            ),
        }
    }
}

fn phase_observations(case: SyncCase, defect: SignalDefect) -> Result<Vec<LockedPhaseObservation>> {
    let meter = Meter::new(4).context("fixture meter must be valid")?;
    (0..case.decks)
        .map(|deck| {
            let map = MapStamp::new(
                BeatMapId::allocate().context("fixture map identity must be available")?,
                BeatMapRevision::first(),
            );
            let affected = deck == 1;
            let expected_beat = BeatOrdinal::new(8);
            let observed_beat = match defect {
                SignalDefect::BeatOrdinal if affected => BeatOrdinal::new(12),
                SignalDefect::BarPhase if affected => BeatOrdinal::new(9),
                _ => expected_beat,
            };
            Ok(LockedPhaseObservation {
                admitted_map: map,
                applied_activation_frame: if defect == SignalDefect::OneFrame && affected {
                    96_001
                } else {
                    96_000
                },
                applied_map: map,
                deck,
                expected_activation_frame: 96_000,
                expected_beat,
                expected_phase_frame: 96_000,
                meter,
                observed_beat,
                observed_phase_frame: 96_000,
            })
        })
        .collect()
}
