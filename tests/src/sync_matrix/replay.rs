use anyhow::{Context, Result, bail};
use kithara::{
    audio::{Beat, BeatMap, MapPoint, MapPosition, MapQuery},
    events::TrackStatus,
    play::{Cmd, Reply, SessionDispatcher, apply_mix},
};

use super::{
    CHANNELS, CaptureBundle, CaptureSource, DeckOutcome, Operation, PcmCapture, RENDER_FRAMES,
    ScenarioFacts, SignalEvidence, SyncCase, SyncHarness, SyncMedia, SyncOracle, SyncOracleReport,
    persist_then_assert,
};
use crate::{
    offline::MixTapProbe, sync_control::SyncDeckControl, sync_fixture::SyncFixtureResources,
};

const LIFECYCLE_CAPTURE_MULTIPLIER: usize = 32;

impl SyncHarness {
    async fn exercise_behavioral_row(&mut self) -> Result<()> {
        self.start_initial().await?;
        self.apply_order().await?;
        self.ride_tempo(self.case.tempo_ride).await?;
        self.unbind_all().await?;
        self.rebind_all().await?;
        self.exercise_map_refinement().await?;
        self.reload_all().await?;
        self.switch_abr_variants().await
    }

    async fn exercise_free_behavioral_row(&mut self) -> Result<()> {
        self.start_initial().await?;
        self.apply_free_order().await?;
        self.ride_free_tempo(self.case.tempo_ride).await?;

        self.record("control-unbind-slot");
        self.render_frames(RENDER_FRAMES * 4).await?;
        self.record("control-rebind-slot");
        self.render_frames(RENDER_FRAMES * 8).await?;

        for _ in 0..self.decks.len() {
            self.record("control-map-unavailable-slot");
            self.render_frames(RENDER_FRAMES * 2).await?;
            self.record("control-map-republish-slot");
            self.render_frames(RENDER_FRAMES * 2).await?;
        }

        self.reload_all_free().await?;
        self.switch_abr_variants().await
    }

    async fn rebind_all(&mut self) -> Result<()> {
        for deck_index in 0..self.decks.len() {
            self.apply(deck_index, Operation::Sync)?;
        }
        self.rebinds = self.rebinds.saturating_add(1);
        self.record("rebind-all");
        let _ = self.render_frames(RENDER_FRAMES * 8).await?;
        Ok(())
    }

    async fn capture_candidate_lifecycle(&mut self) -> Result<PcmCapture> {
        self.apply_uniform_gain()?;
        let (start_session_frame, mut tap) = self.start_pcm_capture()?;
        if let Err(error) = self.exercise_behavioral_row().await {
            self.capture_failures
                .push(format!("candidate lifecycle: {error:#}"));
        }
        if let Err(error) = self.render_frames(self.case.capture_frames()).await {
            self.capture_failures
                .push(format!("candidate tail capture: {error:#}"));
        }
        Ok(self.finish_pcm_capture("candidate-lifecycle-mix", start_session_frame, &mut tap))
    }

    async fn capture_control_lifecycle(&mut self) -> Result<PcmCapture> {
        self.apply_uniform_gain()?;
        let (start_session_frame, mut tap) = self.start_pcm_capture()?;
        if let Err(error) = self.exercise_free_behavioral_row().await {
            self.capture_failures
                .push(format!("control lifecycle: {error:#}"));
        }
        if let Err(error) = self.render_frames(self.case.capture_frames()).await {
            self.capture_failures
                .push(format!("control tail capture: {error:#}"));
        }
        Ok(self.finish_pcm_capture("free-control-lifecycle-mix", start_session_frame, &mut tap))
    }

    pub(super) fn start_pcm_capture(&mut self) -> Result<(i64, MixTapProbe)> {
        let start_session_frame = self.current_session_frame()?;
        let capacity = self
            .case
            .capture_frames()
            .saturating_mul(LIFECYCLE_CAPTURE_MULTIPLIER)
            .saturating_mul(usize::from(CHANNELS));
        let tap = self
            .session
            .enable_mix_tap(capacity)
            .with_context(|| format!("{}: enable session mix tap", self.case))?;
        self.start_backend_capture()?;
        Ok((start_session_frame, tap))
    }

    pub(super) fn finish_pcm_capture(
        &mut self,
        label: impl Into<String>,
        start_session_frame: i64,
        tap: &mut MixTapProbe,
    ) -> PcmCapture {
        let backend = match self.finish_backend_capture() {
            Ok(backend) => backend,
            Err(error) => {
                self.capture_failures
                    .push(format!("finish backend capture: {error:#}"));
                Vec::new()
            }
        };
        let tapped = tap.drain();
        let drops = tap.drops();
        if let Err(error) = self.disable_mix_tap() {
            self.capture_failures
                .push(format!("disable mix tap: {error:#}"));
        }
        let label = label.into();
        if tapped != backend {
            self.capture_failures.push(format!(
                "{label}: final session mix tap differs from rendered backend PCM"
            ));
        }
        if drops != 0 {
            self.capture_failures.push(format!(
                "{label}: final session mix tap dropped {drops} samples"
            ));
        }
        PcmCapture {
            channels: CHANNELS,
            label,
            sample_rate: self.case.sample_rate,
            samples: tapped,
            start_session_frame,
        }
    }

    async fn capture_current_mix(&mut self, label: impl Into<String>) -> Result<PcmCapture> {
        let start_session_frame = self.current_session_frame()?;
        let capacity = self
            .case
            .capture_frames()
            .saturating_mul(usize::from(CHANNELS))
            .saturating_add(RENDER_FRAMES * usize::from(CHANNELS));
        let mut tap = self
            .session
            .enable_mix_tap(capacity)
            .with_context(|| format!("{}: enable phase mix tap", self.case))?;
        self.start_backend_capture()?;
        if let Err(error) = self.render_frames(self.case.capture_frames()).await {
            self.capture_failures
                .push(format!("phase capture: {error:#}"));
        }
        Ok(self.finish_pcm_capture(label, start_session_frame, &mut tap))
    }

    fn disable_mix_tap(&self) -> Result<()> {
        match self.session.exec(Cmd::DisableMixTap) {
            Ok(Reply::Ok) => Ok(()),
            Ok(Reply::Err(error)) => bail!("{}: disable mix tap failed: {error}", self.case),
            Ok(_) => bail!(
                "{}: disable mix tap returned an unexpected reply",
                self.case
            ),
            Err(error) => bail!("{}: disable mix tap dispatch failed: {error}", self.case),
        }
    }

    fn current_session_frame(&self) -> Result<i64> {
        let transport = match self.session.exec(Cmd::QuerySessionTransport)? {
            Reply::SessionTransport(snapshot) => snapshot,
            Reply::Err(error) => bail!("{}: session frame query failed: {error}", self.case),
            _ => bail!(
                "{}: session frame query returned an unexpected reply",
                self.case
            ),
        };
        let position = transport.position();
        let host = transport.host_map().snapshot();
        let beat =
            Beat::new(f64::from(position)).context("convert session beat to host-map beat")?;
        let resolved = match host.position_at(MapPoint::new(host.stamp(), beat)) {
            MapQuery::Resolved(resolved) => resolved,
            query => bail!("{}: host-map frame query failed: {query:?}", self.case),
        };
        match *resolved.value().value() {
            MapPosition::Host(frame) => Ok(i64::from(frame)),
            _ => bail!("{}: host map returned a non-host frame", self.case),
        }
    }

    fn current_session_bpm(&self) -> Result<f64> {
        match self.session.exec(Cmd::QuerySessionTransport)? {
            Reply::SessionTransport(snapshot) => Ok(snapshot.tempo().beats_per_minute()),
            Reply::Err(error) => bail!("{}: session tempo query failed: {error}", self.case),
            _ => bail!(
                "{}: session tempo query returned an unexpected reply",
                self.case
            ),
        }
    }

    async fn capture_replays(mut self) -> Result<CaptureBundle> {
        let mix = self.capture_candidate_lifecycle().await?;
        let deck_replays = self.capture_synced_windows().await?;
        let facts = self.facts()?;
        let ledger = self.ledger.clone();
        let mut capture_failures = self.capture_failures.clone();

        let mut control = Self::open(self.case, self.media.clone()).await?;
        let control_mix = control.capture_control_lifecycle().await?;
        let control_replays = control.capture_control_windows().await?;
        capture_failures.extend(control.capture_failures.iter().cloned());
        drop(control);

        let mut pre_sync = Self::open(self.case, self.media.clone()).await?;
        pre_sync.start_staggered().await?;
        let pre_sync_replays = pre_sync.capture_pre_sync_windows().await?;
        capture_failures.extend(pre_sync.capture_failures.iter().cloned());

        let sources = (0..self.case.decks)
            .map(|deck| {
                let track = self.media.for_deck(deck);
                CaptureSource {
                    analysis_key: track.analysis_key.clone(),
                    deck: format!("deck-{deck}"),
                    media: track.label.clone(),
                }
            })
            .collect();
        Ok(CaptureBundle {
            capture_failures,
            facts,
            ledger,
            library_seed: self.media.library_seed,
            media_id: self.media.id.clone(),
            signal: SignalEvidence {
                control_mix,
                control_replays,
                deck_replays,
                mix,
                phase_observations: Vec::new(),
                pre_sync_replays,
            },
            sources,
        })
    }

    async fn capture_synced_windows(&mut self) -> Result<Vec<PcmCapture>> {
        let mut captures = Vec::with_capacity(self.case.decks);
        for audible_deck in 0..self.case.decks {
            self.apply_gain_mask(audible_deck)?;
            self.render_frames(RENDER_FRAMES * 4).await?;
            captures.push(
                self.capture_current_mix(format!("deck-replay-{audible_deck}"))
                    .await?,
            );
        }
        Ok(captures)
    }

    async fn capture_control_windows(&mut self) -> Result<Vec<PcmCapture>> {
        let mut captures = Vec::with_capacity(self.case.decks);
        for audible_deck in 0..self.case.decks {
            self.apply_gain_mask(audible_deck)?;
            self.render_frames(RENDER_FRAMES * 4).await?;
            captures.push(
                self.capture_current_mix(format!("control-replay-{audible_deck}"))
                    .await?,
            );
        }
        Ok(captures)
    }

    async fn capture_pre_sync_windows(&mut self) -> Result<Vec<PcmCapture>> {
        let mut captures = Vec::with_capacity(self.case.decks);
        for audible_deck in 0..self.case.decks {
            self.apply_gain_mask(audible_deck)?;
            self.render_frames(RENDER_FRAMES * 4).await?;
            captures.push(
                self.capture_current_mix(format!("pre-sync-replay-{audible_deck}"))
                    .await?,
            );
        }
        Ok(captures)
    }

    fn apply_uniform_gain(&self) -> Result<()> {
        let gain = self.deck_gain();
        apply_mix(self.decks.iter().map(|deck| (deck.player.as_ref(), gain)))
            .with_context(|| format!("{}: apply conservative shared gain", self.case))
    }

    fn apply_gain_mask(&self, audible_deck: usize) -> Result<()> {
        if audible_deck >= self.decks.len() {
            bail!("{}: no deck {audible_deck} to solo", self.case);
        }
        let gain = self.deck_gain();
        apply_mix(self.decks.iter().enumerate().map(|(index, deck)| {
            (
                deck.player.as_ref(),
                if index == audible_deck { gain } else { 0.0 },
            )
        }))
        .with_context(|| format!("{}: apply gain mask for deck {audible_deck}", self.case))
    }

    fn facts(&self) -> Result<ScenarioFacts> {
        Ok(ScenarioFacts {
            abr_switch_failures: self.abr_switch_failures,
            abr_switches: self.abr_switches,
            abr_switches_expected: self.abr_switches_expected,
            deck_outcomes: self
                .decks
                .iter()
                .map(|deck| DeckOutcome {
                    current_index: deck.queue.current_index(),
                    expected_index: deck.sync_index,
                    expected_rate: (self.case.tempo_ride.final_bpm() / deck.bpm) as f32,
                    playing: deck.player.is_playing(),
                    rate: deck.player.rate(),
                    track_failed: deck
                        .queue
                        .tracks()
                        .iter()
                        .any(|track| matches!(track.status, TrackStatus::Failed(_))),
                })
                .collect(),
            event_lagged: self.decks.iter().map(|deck| deck.event_lagged).sum(),
            event_streams_closed: self
                .decks
                .iter()
                .filter(|deck| deck.event_stream_closed)
                .count(),
            final_session_bpm: self.current_session_bpm()?,
            map_unavailable_errors: self.map_unavailable_errors,
            map_withdrawals: self.map_withdrawals,
            map_republishes: self.map_republishes,
            reloads: self.reloads,
            rebinds: self.rebinds,
            tempo_ride_points: self.tempo_ride_points,
            tempo_ride_requests: self.tempo_ride_requests,
            tempo_ride_transport_not_processed: self.tempo_ride_transport_not_processed,
            underruns: self.decks.iter().map(|deck| deck.underruns).sum(),
        })
    }

    async fn unbind_all(&mut self) -> Result<()> {
        for deck in &self.decks {
            deck.queue
                .unbind_from_map()
                .with_context(|| format!("{}: unbind deck", self.case))?;
        }
        self.record("unbind-all");
        let _ = self.render_frames(RENDER_FRAMES * 4).await?;
        Ok(())
    }
}

async fn run_behavioral_row(case: SyncCase, media: SyncMedia) -> Result<CaptureBundle> {
    SyncHarness::open(case, media)
        .await?
        .capture_replays()
        .await
}

pub async fn run_synthetic_behavioral_row(
    case: SyncCase,
    resources: SyncFixtureResources,
) -> Result<CaptureBundle> {
    SyncHarness::synthetic(case, resources)
        .await?
        .capture_replays()
        .await
}

pub async fn assert_behavioral_row(case: SyncCase, media: SyncMedia) -> Result<SyncOracleReport> {
    let bundle = run_behavioral_row(case, media).await?;
    let report = SyncOracle::evaluate(case, &bundle);
    persist_then_assert(case, &bundle, &report)?;
    Ok(report)
}
