use kithara::{
    beat::BeatGridModel,
    platform::time::Duration,
    signal::{SegmentId, SessionEpoch, SessionFrame},
    warp::{
        AssetAxis, AssetExtent, AssetFrame, Beat, BeatGridId, BeatGridQuery, BeatGridRevision,
        BeatGridSnapshot, MapPoint, MapPosition, SessionAnchor,
    },
};
use kithara_render::{
    LaneFrame,
    bridge::{SlotMark, SlotSnapshot, SlotState},
};

use super::{cases::*, harness::*, imports::*, providers::*};

#[derive(Debug)]
pub(super) struct GridBinding {
    mark: SlotMark,
    media_origin: Duration,
    source_beat: f64,
    host_beat: f64,
    expected_frame: SessionFrame,
    expected_speed: f32,
    actual_speed: f32,
}

impl GridBinding {
    pub(super) fn failures(&self) -> Vec<String> {
        let mut failures = Vec::new();
        if self.expected_frame != self.mark.session {
            failures.push(format!(
                "source grid beat {:.12} binds to Host beat {:.12} at {:?}, consumed at {:?}",
                self.source_beat, self.host_beat, self.expected_frame, self.mark.session
            ));
        }
        if self.expected_speed != self.actual_speed {
            failures.push(format!(
                "source/Host grid ratio requires speed {}, consumed track reports {}",
                self.expected_speed, self.actual_speed
            ));
        }
        failures
    }

    pub(super) fn evidence(&self) -> serde_json::Value {
        serde_json::json!({
            "source_beat": self.source_beat,
            "host_beat": self.host_beat,
            "expected_frame": i64::from(self.expected_frame),
            "consumed_session_frame": i64::from(self.mark.session),
            "consumed_lane": format!("{:?}", self.mark.lane),
            "consumed_raw_ns": self.mark.position.as_nanos().to_string(),
            "media_origin_ns": self.media_origin.as_nanos().to_string(),
            "expected_speed": self.expected_speed,
            "actual_speed": self.actual_speed,
        })
    }
}

pub(super) fn check(
    model: &BeatGridModel,
    source_rate: NonZeroU32,
    host_grid: &BeatGridSnapshot,
    consumed: SlotSnapshot,
    actual_speed: f32,
    host_beats_per_bar: u16,
) -> Result<GridBinding, String> {
    if consumed.state != SlotState::Playing {
        return Err(format!(
            "binding has no consumed playing slot: {:?}",
            consumed.state
        ));
    }
    let mark = consumed
        .mark
        .ok_or_else(|| "binding has no consumed PCM mark".to_owned())?;
    let media_origin = consumed
        .media_origin
        .ok_or_else(|| "binding has no known media origin".to_owned())?;
    let media = mark
        .position
        .checked_sub(media_origin)
        .ok_or_else(|| "consumed raw position is before media zero".to_owned())?;
    let meter = model
        .as_raw()
        .meter
        .ok_or_else(|| "source grid has no declared meter".to_owned())?;
    if meter.beats_per_bar.get() != host_beats_per_bar {
        return Err(
            "direct four-beat binding proof requires the same declared source/Host meter"
                .to_owned(),
        );
    }
    let source_grid = BeatGridSnapshot::model(
        BeatGridId::allocate().map_err(|error| error.to_string())?,
        BeatGridRevision::first(),
        model,
        AssetAxis::new(source_rate, AssetExtent::Unknown),
    )
    .map_err(|error| error.to_string())?;
    let source_frame = AssetFrame::new(media.as_secs_f64() * f64::from(source_rate.get()))
        .map_err(|error| error.to_string())?;
    let source_position = MapPoint::new(source_grid.stamp(), MapPosition::Asset(source_frame));
    let BeatGridQuery::Resolved(source_beat) = source_grid.beat_at(source_position) else {
        return Err("consumed source position lies outside the supplied grid".to_owned());
    };
    let source_beat = f64::from(*source_beat.value().value());
    let host_position = MapPoint::new(host_grid.stamp(), MapPosition::Session(mark.session));
    let BeatGridQuery::Resolved(observed_host_beat) = host_grid.beat_at(host_position) else {
        return Err("consumed session frame has no published Host beat".to_owned());
    };
    let observed_host_beat = f64::from(*observed_host_beat.value().value());
    let bar = f64::from(host_beats_per_bar);
    let origin: f64 = meter.origin_beat_ordinal.as_();
    let phase = (source_beat - origin).rem_euclid(bar);
    let host_beat = ((observed_host_beat - phase) / bar).round() * bar + phase;
    let host_target = MapPoint::new(
        host_grid.stamp(),
        Beat::new(host_beat).map_err(|error| error.to_string())?,
    );
    let BeatGridQuery::Resolved(expected_position) = host_grid.position_at(host_target) else {
        return Err("source-grid phase has no Host target frame".to_owned());
    };
    let MapPosition::Session(expected_frame) = *expected_position.value().value() else {
        return Err("published Host grid has a different coordinate axis".to_owned());
    };
    let BeatGridQuery::Resolved(source_tempo) = source_grid.tempo_at(source_position) else {
        return Err("consumed source grid has no local tempo".to_owned());
    };
    let BeatGridQuery::Resolved(host_tempo) = host_grid.tempo_at(host_position) else {
        return Err("consumed Host grid has no local tempo".to_owned());
    };
    let expected_speed: f32 =
        (f64::from(*host_tempo.value()) / f64::from(*source_tempo.value())).as_();
    Ok(GridBinding {
        mark,
        media_origin,
        source_beat,
        host_beat,
        expected_frame,
        expected_speed,
        actual_speed,
    })
}

#[derive(Clone, Copy, Debug)]
pub(super) struct SelectedBinding {
    pub(super) media: Duration,
    pub(super) source_beat: f64,
    pub(super) host_beat: f64,
    pub(super) frame: SessionFrame,
}

/// Selects the nearest in-phase position of the constant-120, four-beat fixture.
pub(super) fn select_nearest(
    model: &BeatGridModel,
    evaluation_rate: NonZeroU32,
    host_grid: &BeatGridSnapshot,
    requested: Duration,
    frame: SessionFrame,
) -> Result<SelectedBinding, String> {
    let origin = fixture_origin(model)?;
    let source_grid = declared_source_grid(model, evaluation_rate)?;
    let requested_position = media_position(requested, evaluation_rate)?;
    let requested_beat = grid_beat_at(&source_grid, requested_position)?;
    let host_beat = grid_beat_at(host_grid, MapPosition::Session(frame))?;
    let phase = host_beat.rem_euclid(4.0) + origin;
    let earlier = phase + ((requested_beat - phase) / 4.0).floor() * 4.0;
    let candidates = [earlier, earlier + 4.0]
        .into_iter()
        .filter(|beat| *beat >= 0.0)
        .map(|beat| {
            source_media_at(&source_grid, evaluation_rate, beat)
                .map(|media| media.map(|media| (beat, media)))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let (source_beat, media) = candidates
        .into_iter()
        .flatten()
        .min_by_key(|(_, media)| requested.abs_diff(*media))
        .ok_or_else(|| {
            "no nonnegative in-phase source position lies in the declared grid".to_owned()
        })?;
    Ok(SelectedBinding {
        media,
        source_beat,
        host_beat,
        frame,
    })
}

/// Binds the first declared downbeat at or after cue to the earliest Host entry.
pub(super) fn select_entry(
    model: &BeatGridModel,
    evaluation_rate: NonZeroU32,
    host_grid: &BeatGridSnapshot,
    cue: Duration,
    bound: SessionFrame,
) -> Result<SelectedBinding, String> {
    let origin = fixture_origin(model)?;
    let downbeat = model
        .as_raw()
        .downbeats
        .iter()
        .find(|beat| beat.at >= cue.as_secs_f64())
        .ok_or_else(|| "cue has no declared following downbeat".to_owned())?;
    let media = Duration::try_from_secs_f64(downbeat.at).map_err(|error| error.to_string())?;
    let source_grid = declared_source_grid(model, evaluation_rate)?;
    let source_beat = grid_beat_at(&source_grid, media_position(media, evaluation_rate)?)?;
    let bound_beat = grid_beat_at(host_grid, MapPosition::Session(bound))?;
    let phase = (source_beat - origin).rem_euclid(4.0);
    let earlier = phase + ((bound_beat - phase) / 4.0).floor() * 4.0;
    let candidates = [earlier, earlier + 4.0]
        .into_iter()
        .map(|beat| session_frame_at(host_grid, beat).map(|frame| (beat, frame)))
        .collect::<Result<Vec<_>, _>>()?;
    let (host_beat, frame) = candidates
        .into_iter()
        .filter(|(_, frame)| *frame >= bound)
        .min_by_key(|(_, frame)| *frame)
        .ok_or_else(|| "declared Host grid has no entry at or after the bound".to_owned())?;
    Ok(SelectedBinding {
        media,
        source_beat,
        host_beat,
        frame,
    })
}

impl SelectedBinding {
    pub(super) fn command_failures(
        &self,
        model: &BeatGridModel,
        evaluation_rate: NonZeroU32,
        host_grid: &BeatGridSnapshot,
        actual_media: Duration,
        actual_frame: SessionFrame,
    ) -> Result<Vec<String>, String> {
        let expected_frame =
            self.frame_for_media(model, evaluation_rate, host_grid, actual_media)?;
        let mut failures = Vec::new();
        if expected_frame != self.frame {
            failures.push(format!("selected source beat {:.12} at {:?} was replaced by command position {:?}, which binds to Host frame {:?} instead of {:?}", self.source_beat, self.media, actual_media, expected_frame, self.frame));
        }
        if actual_frame != self.frame {
            failures.push(format!(
                "selected command belongs to Host frame {:?}, admitted at {:?}",
                self.frame, actual_frame
            ));
        }
        Ok(failures)
    }

    pub(super) fn consumption_failures(
        &self,
        model: &BeatGridModel,
        evaluation_rate: NonZeroU32,
        host_grid: &BeatGridSnapshot,
        consumed: SlotSnapshot,
        segment: SegmentId,
    ) -> Result<Vec<String>, String> {
        if consumed.state != SlotState::Playing {
            return Err(format!(
                "selected binding has no consumed playing slot: {:?}",
                consumed.state
            ));
        }
        let mark = consumed
            .mark
            .ok_or_else(|| "selected binding has no consumed PCM mark".to_owned())?;
        let media_origin = consumed
            .media_origin
            .ok_or_else(|| "selected binding has no known media origin".to_owned())?;
        let media = mark
            .position
            .checked_sub(media_origin)
            .ok_or_else(|| "consumed raw position is before media zero".to_owned())?;
        let expected_frame = self.frame_for_media(model, evaluation_rate, host_grid, media)?;
        let mut failures = Vec::new();
        if expected_frame != mark.session {
            failures.push(format!("consumed media position {:?} binds to Host frame {:?} from selected source/Host beats {:.12}/{:.12}, consumed at {:?}", media, expected_frame, self.source_beat, self.host_beat, mark.session));
        }
        if mark.lane.segment != segment {
            failures.push(format!(
                "selected segment {:?} was replaced by consumed segment {:?}",
                segment, mark.lane.segment
            ));
        }
        Ok(failures)
    }

    fn frame_for_media(
        &self,
        model: &BeatGridModel,
        evaluation_rate: NonZeroU32,
        host_grid: &BeatGridSnapshot,
        media: Duration,
    ) -> Result<SessionFrame, String> {
        let source_grid = declared_source_grid(model, evaluation_rate)?;
        let source_beat = grid_beat_at(&source_grid, media_position(media, evaluation_rate)?)?;
        let host_beat = self.host_beat + (source_beat - self.source_beat);
        session_frame_at(host_grid, host_beat)
    }
}

fn fixture_origin(model: &BeatGridModel) -> Result<f64, String> {
    let raw = model.as_raw();
    let meter = raw
        .meter
        .ok_or_else(|| "selected binding has no declared source meter".to_owned())?;
    if meter.beats_per_bar.get() != 4
        || raw.bpm != 120.0
        || raw.beats.windows(2).any(|pair| {
            let previous: f64 = pair[0].ordinal.as_();
            let next: f64 = pair[1].ordinal.as_();
            pair[1].at - pair[0].at != (next - previous) * 0.5
        })
    {
        return Err(
            "selected binding requires the declared constant-120, four-beat fixture".to_owned(),
        );
    }
    Ok(meter.origin_beat_ordinal.as_())
}

fn declared_source_grid(
    model: &BeatGridModel,
    evaluation_rate: NonZeroU32,
) -> Result<BeatGridSnapshot, String> {
    BeatGridSnapshot::model(
        BeatGridId::allocate().map_err(|error| error.to_string())?,
        BeatGridRevision::first(),
        model,
        AssetAxis::new(evaluation_rate, AssetExtent::Unknown),
    )
    .map_err(|error| error.to_string())
}

fn media_position(media: Duration, evaluation_rate: NonZeroU32) -> Result<MapPosition, String> {
    AssetFrame::new(media.as_secs_f64() * f64::from(evaluation_rate.get()))
        .map(MapPosition::Asset)
        .map_err(|error| error.to_string())
}

fn grid_beat_at(grid: &BeatGridSnapshot, position: MapPosition) -> Result<f64, String> {
    let point = MapPoint::new(grid.stamp(), position);
    let BeatGridQuery::Resolved(beat) = grid.beat_at(point) else {
        return Err(format!("position {:?} has no declared grid beat", position));
    };
    Ok(f64::from(*beat.value().value()))
}

fn session_frame_at(grid: &BeatGridSnapshot, beat: f64) -> Result<SessionFrame, String> {
    let target = MapPoint::new(
        grid.stamp(),
        Beat::new(beat).map_err(|error| error.to_string())?,
    );
    let BeatGridQuery::Resolved(position) = grid.position_at(target) else {
        return Err("selected source position has no Host target frame".to_owned());
    };
    let MapPosition::Session(frame) = *position.value().value() else {
        return Err("selected Host grid has a different coordinate axis".to_owned());
    };
    Ok(frame)
}

fn source_media_at(
    grid: &BeatGridSnapshot,
    evaluation_rate: NonZeroU32,
    beat: f64,
) -> Result<Option<Duration>, String> {
    let point = MapPoint::new(
        grid.stamp(),
        Beat::new(beat).map_err(|error| error.to_string())?,
    );
    let query = grid.position_at(point);
    let position = match query {
        BeatGridQuery::Resolved(position) => position,
        BeatGridQuery::OutsideDomain => return Ok(None),
        _ => {
            return Err(format!(
                "source beat {beat:.12} has no declared grid position: {query:?}"
            ));
        }
    };
    let MapPosition::Asset(frame) = *position.value().value() else {
        return Err("declared source grid has a different coordinate axis".to_owned());
    };
    Duration::try_from_secs_f64(f64::from(frame) / f64::from(evaluation_rate.get()))
        .map(Some)
        .map_err(|error| error.to_string())
}

#[cfg(not(target_os = "android"))]
#[kithara::test(native, tokio, multi_thread, serial, timeout(Duration::from_secs(60)))]
#[case::plain_tempo_hold(
    Provider::HlsMp3(HlsProtection::Plain),
    SyncCase { id: "tempo-hold-30hz", ride: TempoRide::Hold(120.0), ..TEMPO_DOWN_30 },
    None
)]
#[case::plain_tempo_down(Provider::HlsMp3(HlsProtection::Plain), TEMPO_DOWN_30, None)]
#[case::plain_sync_seek_play(Provider::HlsMp3(HlsProtection::Plain), SYNC_SEEK_PLAY, None)]
#[case::drm_tempo_down(Provider::HlsMp3(HlsProtection::Drm), TEMPO_DOWN_30, None)]
#[case::drm_sync_seek_play(Provider::HlsMp3(HlsProtection::Drm), SYNC_SEEK_PLAY, None)]
#[case::symphonia_tempo_down(
    Provider::HlsMp3(HlsProtection::Plain),
    TEMPO_DOWN_30,
    Some(kithara::decode::DecoderBackend::Symphonia)
)]
#[case::symphonia_sync_seek_play(
    Provider::HlsMp3(HlsProtection::Plain),
    SYNC_SEEK_PLAY,
    Some(kithara::decode::DecoderBackend::Symphonia)
)]
async fn supplied_grids_bind_consumed_pcm_marks_to_the_host(
    #[case] provider: Provider,
    #[case] case: SyncCase,
    #[case] decoder: Option<kithara::decode::DecoderBackend>,
) {
    let prepared = prepared_sources(provider).await;
    let mut harness = match decoder {
        Some(decoder) => {
            ProductHarness::new_with_decoder(case, &prepared, CUE, Audible::Deck(0), decoder).await
        }
        None => ProductHarness::new(case, &prepared, CUE, Audible::Deck(0)).await,
    };
    if decoder.is_some() {
        harness.assert_decoder_backend(kithara::audio::DecoderBackend::Symphonia);
    }
    harness.run_operations(case).await;
    harness.ride_tempo(case).await;
    let pcm = harness.capture(case).await;
    let mut failures = harness.grid_binding_failures(case).await;
    failures.extend(harness.grid_anchor_failures(case).await);
    drop(harness);
    failures.extend(synchronization_failures(
        "supplementary PCM",
        &[&pcm],
        CHANNELS,
        case.sample_rate,
        case.final_bpm(),
    ));
    assert!(
        failures.is_empty(),
        "direct grid binding failed: {}",
        failures.join("\n")
    );
}

#[kithara::test(native)]
fn direct_grid_binding_rejects_a_frame_a_beat_and_a_missing_origin() {
    let source_rate = NonZeroU32::new(48_000).expect("source rate");
    let output_rate = NonZeroU32::new(44_100).expect("Host rate");
    let anchor = SessionAnchor::new(
        SessionFrame::new(0),
        SessionBeat::default(),
        1.8,
        output_rate,
    )
    .expect("Host anchor");
    let host_grid = BeatGridSnapshot::session(
        BeatGridId::allocate().expect("Host grid id"),
        BeatGridRevision::first(),
        SessionEpoch::new(0),
        anchor,
        None,
    );
    let ArtifactSource::Value(model) = synthetic_grid() else {
        panic!("declared source grid");
    };
    let consumed = SlotSnapshot {
        state: SlotState::Playing,
        mark: Some(SlotMark {
            session: SessionFrame::new(196_000),
            lane: LaneFrame {
                segment: SegmentId::FIRST,
                frame: 196_000,
            },
            position: Duration::from_millis(4_012),
        }),
        media_origin: Some(Duration::from_millis(12)),
        ..SlotSnapshot::default()
    };
    let binding = check(&model, source_rate, &host_grid, consumed, 0.9, 4)
        .expect("declared source and Host grids");
    assert!(binding.failures().is_empty(), "{binding:?}");
    assert_eq!(binding.source_beat, 8.0);
    assert_eq!(binding.host_beat, 8.0);
    assert_eq!(binding.expected_frame, SessionFrame::new(196_000));

    let shifted_mark = SlotSnapshot {
        mark: consumed.mark.map(|mark| SlotMark {
            session: SessionFrame::new(196_001),
            ..mark
        }),
        ..consumed
    };
    let shifted =
        check(&model, source_rate, &host_grid, shifted_mark, 0.9, 4).expect("shifted source mark");
    assert_eq!(shifted.expected_frame, SessionFrame::new(196_000));
    assert_eq!(shifted.mark.session, SessionFrame::new(196_001));
    assert_eq!(shifted.failures().len(), 1);

    let mut wrong_grid = model.as_raw().clone();
    for beat in &mut wrong_grid.beats {
        beat.at += 0.5;
    }
    for downbeat in &mut wrong_grid.downbeats {
        downbeat.at += 0.5;
    }
    let wrong_grid = BeatGridModel::try_from(wrong_grid).expect("a valid misplaced source grid");
    let misplaced = check(&wrong_grid, source_rate, &host_grid, consumed, 0.9, 4)
        .expect("one-beat misplaced grid");
    assert_eq!(misplaced.expected_frame, SessionFrame::new(171_500));
    assert_eq!(misplaced.failures().len(), 1);

    let mut one_frame_grid = model.as_raw().clone();
    let frame_seconds = 1.0 / f64::from(output_rate.get());
    for beat in &mut one_frame_grid.beats {
        beat.at += frame_seconds;
    }
    for downbeat in &mut one_frame_grid.downbeats {
        downbeat.at += frame_seconds;
    }
    let one_frame_grid =
        BeatGridModel::try_from(one_frame_grid).expect("a one-frame displaced source grid");
    let misplaced_frame = check(&one_frame_grid, source_rate, &host_grid, consumed, 0.9, 4)
        .expect("one-frame displaced source grid");
    assert_eq!(misplaced_frame.expected_frame, SessionFrame::new(195_999));
    assert_eq!(misplaced_frame.failures().len(), 1);

    assert!(
        check(
            &model,
            source_rate,
            &host_grid,
            SlotSnapshot {
                media_origin: None,
                ..consumed
            },
            0.9,
            4
        )
        .is_err()
    );
    let ignored_origin = check(
        &model,
        source_rate,
        &host_grid,
        SlotSnapshot {
            media_origin: Some(Duration::ZERO),
            ..consumed
        },
        0.9,
        4,
    )
    .expect("incorrect raw/media translation");
    assert_eq!(ignored_origin.expected_frame, SessionFrame::new(196_588));
    assert_eq!(ignored_origin.failures().len(), 1);
}

#[kithara::test(native)]
fn direct_grid_anchor_rejects_a_whole_bar_and_a_wrong_selected_cue() {
    let evaluation_rate = NonZeroU32::new(48_000).expect("grid evaluation rate");
    let output_rate = NonZeroU32::new(44_100).expect("Host rate");
    let host_anchor = SessionAnchor::new(
        SessionFrame::new(0),
        SessionBeat::default(),
        1.8,
        output_rate,
    )
    .expect("Host anchor");
    let host_grid = BeatGridSnapshot::session(
        BeatGridId::allocate().expect("Host grid id"),
        BeatGridRevision::first(),
        SessionEpoch::new(0),
        host_anchor,
        None,
    );
    let ArtifactSource::Value(model) = synthetic_grid() else {
        panic!("declared source grid");
    };
    let frame = SessionFrame::new(196_000);
    let selected = select_nearest(
        &model,
        evaluation_rate,
        &host_grid,
        Duration::from_secs(4),
        frame,
    )
    .expect("selected source position");
    assert_eq!(selected.media, Duration::from_secs(4));
    assert_eq!(selected.source_beat, 8.0);
    assert_eq!(selected.host_beat, 8.0);
    assert_eq!(selected.frame, frame);
    assert!(
        selected
            .command_failures(&model, evaluation_rate, &host_grid, selected.media, frame)
            .expect("exact selected command")
            .is_empty()
    );
    let consumed = SlotSnapshot {
        state: SlotState::Playing,
        mark: Some(SlotMark {
            session: frame,
            lane: LaneFrame {
                segment: SegmentId::FIRST,
                frame: 196_000,
            },
            position: Duration::from_millis(4_012),
        }),
        media_origin: Some(Duration::from_millis(12)),
        ..SlotSnapshot::default()
    };
    assert!(
        selected
            .consumption_failures(
                &model,
                evaluation_rate,
                &host_grid,
                consumed,
                SegmentId::FIRST
            )
            .expect("exact selected consumption")
            .is_empty()
    );

    for seconds in [2, 6] {
        let media = Duration::from_secs(seconds);
        let wrong = SlotSnapshot {
            mark: consumed.mark.map(|mark| SlotMark {
                position: media + Duration::from_millis(12),
                ..mark
            }),
            ..consumed
        };
        assert!(
            check(&model, evaluation_rate, &host_grid, wrong, 0.9, 4)
                .expect("whole-bar phase-only binding")
                .failures()
                .is_empty()
        );
        assert_eq!(
            selected
                .consumption_failures(&model, evaluation_rate, &host_grid, wrong, SegmentId::FIRST)
                .expect("wrong whole-bar consumption")
                .len(),
            1
        );
        assert_eq!(
            selected
                .command_failures(&model, evaluation_rate, &host_grid, media, frame)
                .expect("wrong whole-bar command target")
                .len(),
            1
        );
    }
    let next_frame = SessionFrame::new(196_001);
    assert_eq!(
        selected
            .command_failures(
                &model,
                evaluation_rate,
                &host_grid,
                selected.media,
                next_frame
            )
            .expect("wrong command moment")
            .len(),
        1
    );
    let one_frame_media =
        selected.media + Duration::from_secs_f64(1.0 / f64::from(output_rate.get()));
    assert_eq!(
        selected
            .command_failures(&model, evaluation_rate, &host_grid, one_frame_media, frame)
            .expect("one-frame command target displacement")
            .len(),
        1
    );
    let shifted = SlotSnapshot {
        mark: consumed.mark.map(|mark| SlotMark {
            session: next_frame,
            ..mark
        }),
        ..consumed
    };
    assert_eq!(
        selected
            .consumption_failures(
                &model,
                evaluation_rate,
                &host_grid,
                shifted,
                SegmentId::FIRST
            )
            .expect("one-frame consumption displacement")
            .len(),
        1
    );
    let wrong_segment = SlotSnapshot {
        mark: consumed.mark.map(|mark| SlotMark {
            lane: LaneFrame {
                segment: SegmentId::FIRST.next(),
                ..mark.lane
            },
            ..mark
        }),
        ..consumed
    };
    assert_eq!(
        selected
            .consumption_failures(
                &model,
                evaluation_rate,
                &host_grid,
                wrong_segment,
                SegmentId::FIRST
            )
            .expect("foreign consumed segment")
            .len(),
        1
    );
    assert!(
        selected
            .consumption_failures(
                &model,
                evaluation_rate,
                &host_grid,
                SlotSnapshot {
                    media_origin: None,
                    ..consumed
                },
                SegmentId::FIRST
            )
            .is_err()
    );
    let advanced = SlotSnapshot {
        mark: consumed.mark.map(|mark| SlotMark {
            session: SessionFrame::new(240_100),
            lane: LaneFrame {
                frame: 240_100,
                ..mark.lane
            },
            position: Duration::from_millis(4_912),
        }),
        ..consumed
    };
    assert!(
        selected
            .consumption_failures(
                &model,
                evaluation_rate,
                &host_grid,
                advanced,
                SegmentId::FIRST
            )
            .expect("consumption advances from the frozen source/Host pair")
            .is_empty()
    );

    let tied = select_nearest(
        &model,
        evaluation_rate,
        &host_grid,
        Duration::from_secs(5),
        frame,
    )
    .expect("equidistant source positions");
    assert_eq!(
        tied.media,
        Duration::from_secs(4),
        "nearest jump resolves an exact tie backward"
    );
    let nonnegative = select_nearest(
        &model,
        evaluation_rate,
        &host_grid,
        Duration::ZERO,
        SessionFrame::new(73_500),
    )
    .expect("nearest nonnegative source position");
    assert_eq!(nonnegative.media, Duration::from_millis(1_500));
    assert_eq!(nonnegative.source_beat, 3.0);
    let entry = select_entry(
        &model,
        evaluation_rate,
        &host_grid,
        Duration::from_millis(5_250),
        frame,
    )
    .expect("selected paused entry");
    assert_eq!(entry.media, Duration::from_secs(6));
    assert_eq!(entry.source_beat, 12.0);
    assert_eq!(entry.host_beat, 8.0);
    assert_eq!(entry.frame, frame);
    assert!(
        entry
            .command_failures(
                &model,
                evaluation_rate,
                &host_grid,
                entry.media,
                entry.frame
            )
            .expect("exact selected entry")
            .is_empty()
    );
    let entry_consumed = SlotSnapshot {
        mark: consumed.mark.map(|mark| SlotMark {
            position: Duration::from_millis(6_012),
            ..mark
        }),
        ..consumed
    };
    assert!(
        entry
            .consumption_failures(
                &model,
                evaluation_rate,
                &host_grid,
                entry_consumed,
                SegmentId::FIRST
            )
            .expect("different absolute source and Host ordinals remain bound")
            .is_empty()
    );
    assert_eq!(
        entry
            .command_failures(
                &model,
                evaluation_rate,
                &host_grid,
                Duration::from_secs(4),
                entry.frame
            )
            .expect("wrong paused selected cue")
            .len(),
        1
    );
    let after = select_entry(
        &model,
        evaluation_rate,
        &host_grid,
        Duration::from_millis(5_250),
        next_frame,
    )
    .expect("entry after the previous Host downbeat");
    assert_eq!(after.source_beat, 12.0);
    assert_eq!(after.host_beat, 12.0);
    assert_eq!(after.frame, SessionFrame::new(294_000));
}
