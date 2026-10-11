use kithara_play::{DeckMixSettings, DeckMixSettingsChange, DeckMixerConfig, HostedDeck};

use super::*;

fn mix_level(state: &mut TestState, id: BeatGridId) -> f32 {
    let mut level = None;
    state
        .owner
        .with_deck(id, &mut |_, _, pass| level = Some(pass.mix.level()))
        .expect("held deck");
    level.expect("owner lent the confirmed mix")
}

fn apply_level(
    state: &mut TestState,
    id: BeatGridId,
    level: f32,
) -> Result<kithara_command::Seq, PlayError> {
    let mut result = Err(PlayError::NotReady);
    state.owner.with_deck(id, &mut |_, out, _| {
        result = out.mix(When::Next, DeckMixSettingsChange::Level(level));
    })?;
    result
}

fn settle_block(state: &mut TestState, clock: &mut u64) -> Vec<crate::HostSettled> {
    state.owner.pass();
    render_left(state, clock, 1);
    state.owner.begin_pass();
    state.owner.pass()
}

fn assert_level_receipts(receipts: Vec<crate::HostSettled>, expected: [kithara_command::Seq; 2]) {
    let applied = receipts
        .into_iter()
        .filter_map(|receipt| match receipt {
            crate::HostSettled::Batch { seq, outcome } => {
                Some((seq, outcome.expect("each independent level must apply")))
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        applied.iter().map(|(seq, _)| *seq).collect::<Vec<_>>(),
        expected
    );
    assert_eq!(applied[0].1, applied[1].1);
}

#[kithara::test]
fn engine_initial_state() {
    let mut state = test_state(start_route_loss_stream);
    let prep = ResourcePrep::builder()
        .worker(PlayWorker::new(PlayWorkerConfig::builder(pools()).build()))
        .build();
    let deck = Queue::<TestPools>::new(QueueConfig::builder().prep(prep).build());
    let mut held = Vec::new();
    state.owner.each_deck(&mut |id, _, _, _| held.push(id));
    assert!(state.stream.is_none());
    assert_eq!(held.len(), 0);
    assert_eq!(deck.mixer_config().slots().get(), 4);
    assert!(held.is_empty());
    assert_eq!(state.deck_nodes.len(), 0);
}

#[kithara::test]
fn engine_master_volume_default() {
    let state = test_state(start_route_loss_stream);
    let prep = ResourcePrep::builder()
        .worker(PlayWorker::new(PlayWorkerConfig::builder(pools()).build()))
        .build();
    let deck = Queue::<TestPools>::new(QueueConfig::builder().prep(prep).build());
    assert!((deck.mixer_config().mix().level() - 1.0).abs() < f32::EPSILON);
    assert!(state.stream.is_none());
}

#[kithara::test]
fn apply_updates_desired_levels_for_two_players() {
    let mut state = test_state(start_route_loss_stream);
    let first = insert(&mut state);
    let second = insert(&mut state);
    state.owner.begin_pass();
    apply_level(&mut state, first, 0.25).expect("first level");
    apply_level(&mut state, second, 0.75).expect("second level");
    settle_block(&mut state, &mut 0);
    assert_eq!(mix_level(&mut state, first), 0.25);
    assert_eq!(mix_level(&mut state, second), 0.75);
}

#[kithara::test]
fn apply_rejects_invalid_level_without_mutation() {
    let mut state = test_state(start_route_loss_stream);
    let id = insert(&mut state);
    for bad in [f32::NAN, f32::INFINITY, 1.5, -0.1] {
        let error = apply_level(&mut state, id, bad).expect_err("invalid level");
        assert!(matches!(error, PlayError::MixLevel { .. }));
        assert_eq!(mix_level(&mut state, id), 1.0);
    }
}

#[kithara::test]
fn never_started_player_receives_desired_before_first_start() {
    let mut state = test_state(start_route_loss_stream);
    let prep = ResourcePrep::builder()
        .worker(PlayWorker::new(PlayWorkerConfig::builder(pools()).build()))
        .build();
    let deck = Queue::new(
        QueueConfig::builder()
            .prep(prep)
            .mixer(
                DeckMixerConfig::builder()
                    .mix(DeckMixSettings::builder().level(0.2).build())
                    .build(),
            )
            .build(),
    );
    assert_eq!(deck.mixer_config().mix().level(), 0.2);
    let id = BeatGridId::allocate().expect("fixture deck id");
    ask(
        &mut state,
        HostCommand::Register {
            id,
            deck: Box::new(deck),
        },
    )
    .expect("first registration");
    assert_eq!(
        mix_level(&mut state, id),
        0.2,
        "start must adopt the desired level stored before registration"
    );
}

#[kithara::test]
fn stopped_player_retains_level_on_restart() {
    let mut state = test_state(start_route_loss_stream);
    let id = insert(&mut state);
    state.owner.begin_pass();
    apply_level(&mut state, id, 0.4).expect("deck level");
    settle_block(&mut state, &mut 0);
    state.stream = None;
    assert_eq!(mix_level(&mut state, id), 0.4);
    ask(&mut state, HostCommand::Restart).expect("restart output");
    assert_eq!(mix_level(&mut state, id), 0.4);
}

#[kithara::test]
fn empty_batch_is_noop() {
    let mut state = test_state(start_route_loss_stream);
    let id = insert(&mut state);
    state
        .owner
        .with_deck(id, &mut |_, out, _| {
            let (_, sent) = out.together(When::Next, |_| Ok(())).expect("empty batch");
            assert_eq!(sent, None);
        })
        .expect("held deck");
    assert_eq!(mix_level(&mut state, id), 1.0);
}

#[kithara::test]
fn apply_rejects_duplicate_player_without_mutation() {
    let mut state = test_state(start_route_loss_stream);
    let id = insert(&mut state);
    state.owner.begin_pass();
    let first = apply_level(&mut state, id, 0.3).expect("first independent level");
    let second = apply_level(&mut state, id, 0.4).expect("second independent level");
    assert!(second > first);
    assert_eq!(mix_level(&mut state, id), 1.0);
    assert_level_receipts(settle_block(&mut state, &mut 0), [first, second]);
    assert_eq!(mix_level(&mut state, id), 0.4);
}

#[kithara::test]
fn host_mix_rejects_duplicate_player_without_mutation() {
    let mut state = test_state(start_route_loss_stream);
    let id = insert(&mut state);
    state.owner.begin_pass();
    let first = apply_level(&mut state, id, 0.3).expect("first independent level");
    let second = apply_level(&mut state, id, 0.4).expect("second independent level");
    assert!(second > first);
    assert_eq!(mix_level(&mut state, id), 1.0);
    assert_level_receipts(settle_block(&mut state, &mut 0), [first, second]);
    assert_eq!(mix_level(&mut state, id), 0.4);
}

#[kithara::test]
fn host_mix_rejects_invalid_level_without_mutation() {
    for bad in [f32::NAN, f32::INFINITY, 1.5, -0.1] {
        let mut state = test_state(start_route_loss_stream);
        let first = insert(&mut state);
        let second = insert(&mut state);
        state.owner.begin_pass();
        apply_level(&mut state, first, 0.5).expect("independent valid level");
        let error = apply_level(&mut state, second, bad).expect_err("invalid level");
        assert!(matches!(error, PlayError::MixLevel { .. }));
        assert_eq!(mix_level(&mut state, first), 1.0);
        assert_eq!(mix_level(&mut state, second), 1.0);
        settle_block(&mut state, &mut 0);
        assert_eq!(mix_level(&mut state, first), 0.5);
        assert_eq!(
            mix_level(&mut state, second),
            1.0,
            "level {bad} leaked a mutation"
        );
    }
}

#[kithara::test]
fn apply_rejects_foreign_session() {
    let mut state = test_state(start_route_loss_stream);
    let known = insert(&mut state);
    let foreign = BeatGridId::allocate().expect("foreign fixture grid id");
    state.owner.begin_pass();
    apply_level(&mut state, known, 0.5).expect("independent known level");
    let error = apply_level(&mut state, foreign, 0.5).expect_err("foreign deck");
    assert!(matches!(
        error,
        PlayError::Session(SessionError::DeckNotFound(id)) if id == foreign
    ));
    assert_eq!(mix_level(&mut state, known), 1.0);
    settle_block(&mut state, &mut 0);
    assert_eq!(mix_level(&mut state, known), 0.5);
}

#[kithara::test]
fn host_mix_rejects_foreign_member_leaving_known_unchanged() {
    let mut state = test_state(start_route_loss_stream);
    let known = insert(&mut state);
    let foreign = BeatGridId::allocate().expect("foreign fixture grid id");
    state.owner.begin_pass();
    apply_level(&mut state, known, 0.2).expect("independent known level");
    let error = apply_level(&mut state, foreign, 0.3).expect_err("foreign deck");
    assert!(matches!(
        error,
        PlayError::Session(SessionError::DeckNotFound(id)) if id == foreign
    ));
    assert_eq!(mix_level(&mut state, known), 1.0);
    settle_block(&mut state, &mut 0);
    assert_eq!(mix_level(&mut state, known), 0.2);
}
