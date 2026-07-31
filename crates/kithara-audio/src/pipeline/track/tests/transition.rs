use kithara_abr::{AbrMode, AbrReason, AbrState, VariantIndex};
use kithara_decode::{DecoderFactory as DecoderBuilder, GaplessMode};
use kithara_events::{DecoderChangeCause, DecoderEvent, DeferredBus, Event, EventBus};
use kithara_platform::{sync::Arc, time::Duration, tokio::task::yield_now};
use kithara_stream::{
    AudioCodec, ContainerFormat, MediaInfo, VariantPromotion, VariantReaderPlan, VariantTransition,
    VariantTransitionId,
};
use kithara_test_utils::kithara;

use super::rebuild::{
    Consts, RouteFixture, TestDecoder, media_info, produced_data, route_signal_source,
};
use crate::{
    pipeline::{
        decode::DecoderGeneration,
        rebuild::{DecoderBuildComplete, DecoderBuildPurpose, state::BuildId},
        track::{CurrentFsm, TrackStep},
    },
    renderer::AudioWorkerSource,
};

fn incoming_plan() -> VariantReaderPlan {
    let abr = AbrState::new(AbrMode::Auto(Some(VariantIndex::new(0))));
    request_incoming_plan(&abr)
}

fn successive_incoming_plans() -> (VariantReaderPlan, VariantReaderPlan) {
    let abr = AbrState::new(AbrMode::Auto(Some(VariantIndex::new(0))));
    let first = request_incoming_plan(&abr);
    let second = request_incoming_plan(&abr);
    (first, second)
}

fn request_incoming_plan(abr: &AbrState) -> VariantReaderPlan {
    abr.request_target(VariantIndex::new(1), AbrReason::ManualOverride);
    let claim = abr
        .claim_pending_decision(VariantIndex::new(0))
        .expect("exact transition fixture requires a pending ABR claim");
    let transition = VariantTransition::new(
        VariantTransitionId::new(claim.ticket(), 0),
        VariantIndex::new(0),
        VariantIndex::new(1),
    );
    VariantReaderPlan::new(transition, media_info(1), Duration::ZERO)
}

async fn wait_for_incoming_priming(fixture: &mut RouteFixture, transition: VariantTransition) {
    for _ in 0..64 {
        yield_now().await;
        fixture.source.flush_deferred();
        if fixture.source.decode.incoming_is_priming(transition) {
            return;
        }
    }
    panic!("incoming decoder build did not complete");
}

#[kithara::test(tokio)]
async fn exact_incoming_reader_pending_keeps_outgoing_pcm_running() {
    let mut fixture = route_signal_source(Consts::SAMPLE_RATE).await;
    fixture.control.set_exact_plan(incoming_plan());

    fixture.source.flush_deferred();

    let TrackStep::Produced(fetch) = fixture.source.step_track() else {
        panic!("outgoing must keep producing while the incoming reader is preparing");
    };
    let chunk = produced_data(fetch);
    assert!(!chunk.samples.is_empty());
    assert_eq!(chunk.meta.frame_offset, 0);
    assert_eq!(fixture.control.plan_calls(), 1);
    assert_eq!(fixture.control.prepare_calls(), 1);
    assert_eq!(fixture.control.take_calls(), 1);
    assert_eq!(
        fixture
            .source
            .decode
            .active()
            .media_info()
            .and_then(|info| info.variant_index),
        Some(0)
    );
    assert!(matches!(fixture.source.state, CurrentFsm::Decoding(_)));
}

#[kithara::test(tokio)]
async fn exact_incoming_build_pending_keeps_outgoing_pcm_running() {
    let mut fixture = route_signal_source(Consts::SAMPLE_RATE).await;
    let plan = incoming_plan();
    let transition = plan.transition();
    fixture.control.set_exact_plan(plan);
    fixture.control.set_exact_reader_ready();

    fixture.source.flush_deferred();

    assert!(fixture.source.decode.incoming_is_building(transition));
    let TrackStep::Produced(fetch) = fixture.source.step_track() else {
        panic!("outgoing must keep producing while the incoming decoder is building");
    };
    let chunk = produced_data(fetch);
    assert!(!chunk.samples.is_empty());
    assert_eq!(chunk.meta.frame_offset, 0);
    assert_eq!(fixture.control.plan_calls(), 1);
    assert_eq!(fixture.control.prepare_calls(), 1);
    assert_eq!(fixture.control.take_calls(), 1);
    assert_eq!(
        fixture
            .source
            .decode
            .active()
            .media_info()
            .and_then(|info| info.variant_index),
        Some(0)
    );
    assert!(matches!(fixture.source.state, CurrentFsm::Decoding(_)));
}

#[kithara::test(tokio)]
async fn incoming_completion_never_replaces_active_before_staged_pcm() {
    let mut fixture = route_signal_source(Consts::SAMPLE_RATE).await;
    let plan = incoming_plan();
    let transition = plan.transition();
    fixture.control.set_exact_plan(plan);
    fixture.control.set_exact_reader_ready();

    fixture.source.flush_deferred();
    wait_for_incoming_priming(&mut fixture, transition).await;

    assert_eq!(fixture.control.promote_calls(), 0);
    assert_eq!(
        fixture
            .source
            .decode
            .active()
            .media_info()
            .and_then(|info| info.variant_index),
        Some(0)
    );
    let TrackStep::Produced(fetch) = fixture.source.step_track() else {
        panic!("outgoing must remain authoritative while incoming PCM is staged");
    };
    let chunk = produced_data(fetch);
    assert_eq!(chunk.meta.frame_offset, 0);
    assert_eq!(fixture.control.promote_calls(), 0);
    assert_eq!(
        fixture
            .source
            .decode
            .active()
            .media_info()
            .and_then(|info| info.variant_index),
        Some(0)
    );
}

#[kithara::test(tokio)]
async fn exact_primed_generation_promotes_once_at_outgoing_frontier() {
    let mut fixture = route_signal_source(Consts::SAMPLE_RATE).await;
    let plan = incoming_plan();
    let transition = plan.transition();
    fixture.control.set_exact_plan(plan);
    fixture.control.set_exact_reader_ready();
    fixture.control.set_promotion(VariantPromotion::Promoted);

    fixture.source.flush_deferred();
    wait_for_incoming_priming(&mut fixture, transition).await;

    let TrackStep::Produced(outgoing) = fixture.source.step_track() else {
        panic!("outgoing must remain authoritative while incoming PCM is first staged");
    };
    assert_eq!(produced_data(outgoing).meta.frame_offset, 0);
    assert_eq!(fixture.control.promote_calls(), 0);

    fixture.source.flush_deferred();

    assert_eq!(fixture.control.promote_calls(), 1);
    assert_eq!(
        fixture
            .source
            .decode
            .active()
            .media_info()
            .and_then(|info| info.variant_index),
        Some(1)
    );
    let TrackStep::Produced(incoming) = fixture.source.step_track() else {
        panic!("promoted incoming must emit its already staged first chunk");
    };
    assert_eq!(
        produced_data(incoming).meta.frame_offset,
        u64::try_from(Consts::ROUTE_CHUNK_FRAMES).unwrap_or(u64::MAX)
    );
}

#[kithara::test(tokio)]
async fn locked_promotion_keeps_primed_incoming_and_outgoing_authoritative() {
    let mut fixture = route_signal_source(Consts::SAMPLE_RATE).await;
    let plan = incoming_plan();
    let transition = plan.transition();
    fixture.control.set_exact_plan(plan);
    fixture.control.set_exact_reader_ready();
    fixture.control.set_promotion(VariantPromotion::Deferred);

    fixture.source.flush_deferred();
    wait_for_incoming_priming(&mut fixture, transition).await;

    let TrackStep::Produced(first_outgoing) = fixture.source.step_track() else {
        panic!("outgoing must remain audible while incoming PCM is first staged");
    };
    assert_eq!(produced_data(first_outgoing).meta.frame_offset, 0);

    fixture.source.flush_deferred();

    assert_eq!(fixture.control.promote_calls(), 1);
    assert!(fixture.source.decode.incoming_is_priming(transition));
    assert_eq!(
        fixture
            .source
            .decode
            .active()
            .media_info()
            .and_then(|info| info.variant_index),
        Some(0)
    );
    let TrackStep::Produced(next_outgoing) = fixture.source.step_track() else {
        panic!("publication lock must not interrupt outgoing PCM");
    };
    assert_eq!(
        produced_data(next_outgoing).meta.frame_offset,
        u64::try_from(Consts::ROUTE_CHUNK_FRAMES).unwrap_or(u64::MAX)
    );
}

#[kithara::test(tokio)]
async fn newer_ticket_supersedes_only_incoming_generation() {
    let mut fixture = route_signal_source(Consts::SAMPLE_RATE).await;
    let (first, second) = successive_incoming_plans();
    let first_transition = first.transition();
    let second_transition = second.transition();
    fixture.control.set_exact_plan(first);
    fixture.control.set_exact_reader_ready();

    fixture.source.flush_deferred();
    wait_for_incoming_priming(&mut fixture, first_transition).await;
    let TrackStep::Produced(outgoing) = fixture.source.step_track() else {
        panic!("outgoing must remain audible while the first incoming generation is staged");
    };
    assert_eq!(produced_data(outgoing).meta.frame_offset, 0);

    fixture.control.set_exact_plan(second);
    fixture.control.set_exact_reader_ready();
    fixture.source.flush_deferred();

    assert!(
        fixture
            .source
            .decode
            .incoming_is_building(second_transition)
    );
    assert_eq!(
        fixture
            .source
            .decode
            .active()
            .media_info()
            .and_then(|info| info.variant_index),
        Some(0)
    );
    assert_eq!(fixture.drops.lock().as_slice(), &[99]);
}

#[kithara::test(tokio)]
async fn stale_incoming_completion_retires_generation_in_shell() {
    let mut fixture = route_signal_source(Consts::SAMPLE_RATE).await;
    let (first, second) = successive_incoming_plans();
    let first_transition = first.transition();
    let second_transition = second.transition();
    let first_build = BuildId::fixture(41);

    assert!(
        fixture
            .source
            .decode
            .begin_incoming(first_transition)
            .is_none()
    );
    assert!(
        fixture
            .source
            .decode
            .mark_incoming_building(first_transition, first_build)
    );
    assert!(
        fixture
            .source
            .decode
            .begin_incoming(second_transition)
            .is_none()
    );

    let pushed = fixture
        .source
        .rebuild
        .completion()
        .push(DecoderBuildComplete {
            build: first_build,
            purpose: DecoderBuildPurpose::Incoming(first_transition),
            result: Ok(DecoderGeneration::new(
                Box::new(TestDecoder::new(77, fixture.drops.clone())),
                Some(media_info(1)),
                0,
                0,
                None,
                GaplessMode::Disabled,
            )),
        });
    assert!(pushed.is_ok());
    assert!(fixture.drops.lock().is_empty());

    fixture.source.flush_deferred();

    assert_eq!(fixture.drops.lock().as_slice(), &[77]);
    assert_eq!(
        fixture
            .source
            .decode
            .active()
            .media_info()
            .and_then(|info| info.variant_index),
        Some(0)
    );
}

#[kithara::test(tokio)]
async fn incoming_media_facts_choose_reader_profile() {
    let mut fixture = route_signal_source(Consts::SAMPLE_RATE).await;
    fixture.control.enable_byte_map();
    let template = incoming_plan();
    let mut incoming_media =
        MediaInfo::new(Some(AudioCodec::Mp3), Some(ContainerFormat::MpegAudio));
    incoming_media.variant_index = Some(1);
    let plan = VariantReaderPlan::new(
        template.transition(),
        incoming_media.clone(),
        template.landing_time(),
    );
    fixture.control.set_exact_plan(plan);

    fixture.source.flush_deferred();

    let byte_map = fixture.source.shared_stream.byte_map();
    let expected = DecoderBuilder::reader_profile(&incoming_media, byte_map.as_deref());
    let active = DecoderBuilder::reader_profile(&media_info(0), byte_map.as_deref());
    assert_ne!(expected, active);
    assert_eq!(fixture.control.prepared_profile(), Some(expected));
}

#[kithara::test(tokio)]
async fn exact_promotion_emits_variant_switch_decoder_event() {
    let mut fixture = route_signal_source(Consts::SAMPLE_RATE).await;
    let bus = EventBus::new(16);
    let mut events = bus.subscribe();
    fixture.source = fixture
        .source
        .with_emit(Arc::new(DeferredBus::new(bus, 16)));
    let plan = incoming_plan();
    let transition = plan.transition();
    fixture.control.set_exact_plan(plan);
    fixture.control.set_exact_reader_ready();
    fixture.control.set_promotion(VariantPromotion::Promoted);

    fixture.source.flush_deferred();
    wait_for_incoming_priming(&mut fixture, transition).await;
    assert!(matches!(
        fixture.source.step_track(),
        TrackStep::Produced(_)
    ));
    fixture.source.flush_deferred();

    let mut changed = false;
    while let Ok(envelope) = events.try_recv() {
        changed |= matches!(
            envelope.event,
            Event::Decoder(DecoderEvent::DecoderChanged {
                cause: DecoderChangeCause::VariantSwitch,
                variant: Some(1),
                ..
            })
        );
    }
    assert!(changed);
}
