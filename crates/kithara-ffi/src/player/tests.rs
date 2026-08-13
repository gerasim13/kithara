use crate::{
    config::FfiPlayerConfig, item::AudioPlayerItem, player::AudioPlayer, types::FfiItemConfig,
};

#[kithara::test]
fn create_player() {
    let _player = AudioPlayer::new(FfiPlayerConfig::default());
}

#[kithara::test]
fn playing_rate_roundtrip() {
    let player = AudioPlayer::new(FfiPlayerConfig::default());
    assert!((player.playing_rate() - 1.0).abs() < f32::EPSILON);
    player.set_playing_rate(0.5);
    assert!((player.playing_rate() - 0.5).abs() < f32::EPSILON);
}

#[kithara::test]
fn items_initially_empty() {
    let player = AudioPlayer::new(FfiPlayerConfig::default());
    assert!(player.items().is_empty());
}

#[kithara::test]
fn remove_all_items_on_empty_queue() {
    let player = AudioPlayer::new(FfiPlayerConfig::default());
    player.remove_all_items();
    assert!(player.items().is_empty());
}

#[kithara::test]
fn stop_clears_queue_and_releases_inserted_items() {
    let player = AudioPlayer::new(FfiPlayerConfig::default());
    let item = AudioPlayerItem::new(FfiItemConfig {
        abr_mode: None,
        audio_id: None,
        headers: None,
        uuid_i64: None,
        url: "https://example.com/song.mp3".to_string(),
        is_live_stream: false,
        preferred_peak_bitrate: 0.0,
        preferred_peak_bitrate_expensive: 0.0,
    });
    player
        .append(item.clone())
        .expect("valid item must enter the queue");
    assert_eq!(player.item_count(), 1, "setup must populate the queue");
    assert!(*item.inserted.lock(), "setup must mark the item inserted");

    player.stop();

    assert_eq!(player.item_count(), 0, "stop must clear the queue");
    assert!(
        player.items().is_empty(),
        "stop must drain the item registry"
    );
    assert!(
        !*item.inserted.lock(),
        "stop must release the old item for a later insertion"
    );

    player
        .append(item.clone())
        .expect("a stopped item must be insertable again");
    assert_eq!(player.item_count(), 1, "restart queue must accept the item");
    player.stop();
    assert_eq!(player.item_count(), 0, "repeated stop must stay idempotent");
}

#[kithara::test]
fn volume_roundtrip() {
    let player = AudioPlayer::new(FfiPlayerConfig::default());
    assert!((player.volume() - 1.0).abs() < f32::EPSILON);
    player.set_volume(0.5);
    assert!((player.volume() - 0.5).abs() < f32::EPSILON);
}

#[kithara::test]
fn muted_roundtrip() {
    let player = AudioPlayer::new(FfiPlayerConfig::default());
    assert!(!player.is_muted());
    player.set_muted(true);
    assert!(player.is_muted());
}

#[kithara::test]
fn eq_band_count_from_config() {
    let player = AudioPlayer::new(FfiPlayerConfig {
        eq_band_count: 3,
        ..FfiPlayerConfig::default()
    });
    assert_eq!(player.eq_band_count(), 3);
}

#[kithara::test]
fn eq_gain_default_zero() {
    let player = AudioPlayer::new(FfiPlayerConfig::default());
    assert!((player.eq_gain(0) - 0.0).abs() < f32::EPSILON);
}

#[kithara::test]
#[case::set_gain((|p: &AudioPlayer| p.set_eq_gain(0, 3.0).is_err()) as fn(&AudioPlayer) -> bool)]
#[case::reset((|p: &AudioPlayer| p.reset_eq().is_err()) as fn(&AudioPlayer) -> bool)]
fn eq_mutation_without_engine_returns_error(#[case] op_errs: fn(&AudioPlayer) -> bool) {
    let player = AudioPlayer::new(FfiPlayerConfig::default());
    assert!(op_errs(&player));
}

#[kithara::test]
fn eq_gain_out_of_range_band() {
    let player = AudioPlayer::new(FfiPlayerConfig {
        eq_band_count: 3,
        ..FfiPlayerConfig::default()
    });
    assert!((player.eq_gain(99) - 0.0).abs() < f32::EPSILON);
}

#[kithara::test]
fn current_time_zero_when_no_item() {
    let player = AudioPlayer::new(FfiPlayerConfig::default());
    assert!((player.current_time() - 0.0).abs() < f64::EPSILON);
}

#[kithara::test]
fn current_item_none_when_queue_empty() {
    let player = AudioPlayer::new(FfiPlayerConfig::default());
    assert!(player.current_item().is_none());
}

#[kithara::test]
fn snapshot_uses_playing_rate_field_name() {
    let player = AudioPlayer::new(FfiPlayerConfig::default());
    let snap = player.snapshot();
    assert!((snap.playing_rate - 1.0).abs() < f32::EPSILON);
}
