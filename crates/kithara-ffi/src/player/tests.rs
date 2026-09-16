use crate::{config::FfiPlayerConfig, player::AudioPlayer};

#[kithara::test]
fn create_player() {
    let _player = AudioPlayer::new(FfiPlayerConfig::for_test());
}

#[kithara::test]
fn playing_rate_roundtrip() {
    let player = AudioPlayer::new(FfiPlayerConfig::for_test());
    assert!((player.playing_rate() - 1.0).abs() < f32::EPSILON);
    player.set_playing_rate(0.5);
    assert!((player.playing_rate() - 0.5).abs() < f32::EPSILON);
}

#[kithara::test]
fn items_initially_empty() {
    let player = AudioPlayer::new(FfiPlayerConfig::for_test());
    assert!(player.items().is_empty());
}

#[kithara::test]
fn remove_all_items_on_empty_queue() {
    let player = AudioPlayer::new(FfiPlayerConfig::for_test());
    player.remove_all_items();
    assert!(player.items().is_empty());
}

#[kithara::test]
fn volume_roundtrip() {
    let player = AudioPlayer::new(FfiPlayerConfig::for_test());
    assert!((player.volume() - 1.0).abs() < f32::EPSILON);
    player.set_volume(0.5);
    assert!((player.volume() - 0.5).abs() < f32::EPSILON);
}

#[kithara::test]
fn muted_roundtrip() {
    let player = AudioPlayer::new(FfiPlayerConfig::for_test());
    assert!(!player.is_muted());
    player.set_muted(true);
    assert!(player.is_muted());
}

#[kithara::test]
fn eq_band_count_from_config() {
    let player = AudioPlayer::new(FfiPlayerConfig {
        eq_band_count: 3,
        ..FfiPlayerConfig::for_test()
    });
    assert_eq!(player.eq_band_count(), 3);
}

#[kithara::test]
fn eq_gain_default_zero() {
    let player = AudioPlayer::new(FfiPlayerConfig::for_test());
    assert!((player.eq_gain(0) - 0.0).abs() < f32::EPSILON);
}

#[kithara::test]
fn idle_player_eq_can_be_configured_and_reset() {
    let player = AudioPlayer::new(FfiPlayerConfig::for_test());
    player.set_eq_gain(0, 3.0).expect("configure idle EQ");
    assert_eq!(player.eq_gain(0), 3.0);
    player.reset_eq().expect("reset idle EQ");
    assert_eq!(player.eq_gain(0), 0.0);
    assert!(matches!(
        player.set_eq_gain(99, 3.0),
        Err(crate::types::FfiError::InvalidArgument { .. })
    ));
}

#[kithara::test]
fn eq_gain_out_of_range_band() {
    let player = AudioPlayer::new(FfiPlayerConfig {
        eq_band_count: 3,
        ..FfiPlayerConfig::for_test()
    });
    assert!((player.eq_gain(99) - 0.0).abs() < f32::EPSILON);
}

#[kithara::test]
fn current_time_zero_when_no_item() {
    let player = AudioPlayer::new(FfiPlayerConfig::for_test());
    assert!((player.current_time() - 0.0).abs() < f64::EPSILON);
}

#[kithara::test]
fn current_item_none_when_queue_empty() {
    let player = AudioPlayer::new(FfiPlayerConfig::for_test());
    assert!(player.current_item().is_none());
}

#[kithara::test]
fn snapshot_uses_playing_rate_field_name() {
    let player = AudioPlayer::new(FfiPlayerConfig::for_test());
    let snap = player.snapshot();
    assert!((snap.playing_rate - 1.0).abs() < f32::EPSILON);
}
