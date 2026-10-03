use kithara::platform::{
    CancelToken,
    time::{self, Instant},
    tokio::{self, sync::mpsc::UnboundedReceiver},
};

use super::{Engine, Envelope};

pub(crate) async fn run(
    mut engine: Engine,
    mut commands: UnboundedReceiver<Envelope>,
    cancel: CancelToken,
) -> bool {
    let mut next_tick = Instant::now() + engine.cadence();
    loop {
        tokio::select! {
            biased;
            () = cancel.cancelled() => break,
            () = time::sleep(next_tick.saturating_duration_since(Instant::now())) => {
                engine.tick();
                engine.publish();
                next_tick = Instant::now() + engine.cadence();
            }
            received = commands.recv() => {
                let Some(envelope) = received else { break };
                engine.apply(envelope);
                while !engine.is_shut_down()
                    && let Ok(envelope) = commands.try_recv()
                {
                    engine.apply(envelope);
                }
                engine.publish();
                if engine.is_shut_down() {
                    break;
                }
                next_tick = next_tick.min(Instant::now() + engine.cadence());
            }
        }
    }
    engine.is_shut_down()
}

#[cfg(all(test, not(feature = "broadcast")))]
mod tests {
    use std::convert::Infallible;

    use ::kithara::{
        platform::{
            time,
            tokio::{runtime::Handle, sync::oneshot},
        },
        ui::render::ControlAction,
    };
    use kithara_test_fixtures::SignalAsset;
    use kithara_test_utils::{kithara, off_thread::OffThread};

    use crate::{
        analysis::fixtures::{short_wav, tone_mp3},
        deck::DeckId,
        engine::{Command, DeckCmd, Envelope, MixCmd, serve},
        gui::rig::Rig,
    };

    #[kithara::test(native, tokio, flash(false))]
    async fn the_ui_draws_and_queues_while_the_engine_drains_nothing() {
        let rig = OffThread::spawn("engine", || Ok::<_, Infallible>(Rig::offline()))
            .await
            .expect("rig fixture is infallible");
        rig.call(|rig| {
            let echoed = rig.applied_seq();

            rig.send("mixer/xfade", ControlAction::SetScalar(1.0));
            rig.send("deck-a/play", ControlAction::Activate);
            for _ in 0..3 {
                rig.frame();
            }

            assert!((rig.scalar("mix.crossfader") - 1.0).abs() < f64::EPSILON);
            assert!(
                !rig.flag("deck.playback.playing@deck=a"),
                "whether the deck plays is the engine's to report"
            );
            assert_eq!(rig.applied_seq(), echoed, "the engine applied nothing");
            assert!((rig.snapshots.load().mix.position - 0.5).abs() < f32::EPSILON);

            let queued: Vec<Envelope> =
                std::iter::from_fn(|| rig.commands.try_recv().ok()).collect();
            assert!(queued.first().is_some_and(|first| first.seq > echoed));
            assert!(queued.windows(2).all(|pair| pair[0].seq < pair[1].seq));
            assert!(
                matches!(
                    queued.as_slice(),
                    [
                        Envelope {
                            command: Command::Mix(MixCmd::Crossfader(position)),
                            ..
                        },
                        Envelope {
                            command: Command::Deck {
                                deck: DeckId(0),
                                cmd: DeckCmd::Play,
                            },
                            ..
                        },
                    ] if (*position - 1.0).abs() < f32::EPSILON
                ),
                "every message queued its intent, in order"
            );
        })
        .await;
        rig.close().await;
    }

    #[kithara::test(native, tokio, flash(false))]
    #[case::original(None)]
    #[case::profile_flac_flac_192000_2ch_16bit(Some(
        SignalAsset::PROFILE_FLAC_FLAC_192000_2CH_16BIT
    ))]
    #[case::profile_flac_flac_192000_2ch_24bit(Some(
        SignalAsset::PROFILE_FLAC_FLAC_192000_2CH_24BIT
    ))]
    #[case::profile_flac_flac_22050_1ch_16bit(Some(
        SignalAsset::PROFILE_FLAC_FLAC_22050_1CH_16BIT
    ))]
    #[case::profile_flac_flac_22050_2ch_16bit(Some(
        SignalAsset::PROFILE_FLAC_FLAC_22050_2CH_16BIT
    ))]
    #[case::profile_flac_flac_44100_2ch_16bit(Some(
        SignalAsset::PROFILE_FLAC_FLAC_44100_2CH_16BIT
    ))]
    #[case::profile_flac_flac_44100_2ch_24bit(Some(
        SignalAsset::PROFILE_FLAC_FLAC_44100_2CH_24BIT
    ))]
    #[case::profile_flac_flac_48000_2ch_16bit(Some(
        SignalAsset::PROFILE_FLAC_FLAC_48000_2CH_16BIT
    ))]
    #[case::profile_flac_flac_48000_2ch_24bit(Some(
        SignalAsset::PROFILE_FLAC_FLAC_48000_2CH_24BIT
    ))]
    #[case::profile_flac_flac_88200_2ch_24bit(Some(
        SignalAsset::PROFILE_FLAC_FLAC_88200_2CH_24BIT
    ))]
    #[case::profile_flac_flac_96000_2ch_16bit(Some(
        SignalAsset::PROFILE_FLAC_FLAC_96000_2CH_16BIT
    ))]
    #[case::profile_flac_flac_96000_2ch_24bit(Some(
        SignalAsset::PROFILE_FLAC_FLAC_96000_2CH_24BIT
    ))]
    #[case::profile_mp3_libmp3lame_11025_1ch(Some(SignalAsset::PROFILE_MP3_LIBMP3LAME_11025_1CH))]
    #[case::profile_mp3_libmp3lame_22050_1ch(Some(SignalAsset::PROFILE_MP3_LIBMP3LAME_22050_1CH))]
    #[case::profile_mp3_libmp3lame_32000_2ch(Some(SignalAsset::PROFILE_MP3_LIBMP3LAME_32000_2CH))]
    #[case::profile_mp3_libmp3lame_44100_1ch(Some(SignalAsset::PROFILE_MP3_LIBMP3LAME_44100_1CH))]
    #[case::profile_mp3_libmp3lame_44100_2ch(Some(SignalAsset::PROFILE_MP3_LIBMP3LAME_44100_2CH))]
    #[case::profile_mp3_libmp3lame_48000_1ch(Some(SignalAsset::PROFILE_MP3_LIBMP3LAME_48000_1CH))]
    #[case::profile_mp3_libmp3lame_48000_2ch(Some(SignalAsset::PROFILE_MP3_LIBMP3LAME_48000_2CH))]
    #[case::profile_m4a_aac_44100_2ch(Some(SignalAsset::PROFILE_M4A_AAC_44100_2CH))]
    #[case::profile_m4a_alac_44100_2ch_16bit(Some(SignalAsset::PROFILE_M4A_ALAC_44100_2CH_16BIT))]
    #[case::profile_ogg_vorbis_44100_2ch(Some(SignalAsset::PROFILE_OGG_VORBIS_44100_2CH))]
    #[cfg_attr(
        target_os = "macos",
        case::profile_opus_libopus_48000_2ch(Some(SignalAsset::PROFILE_OPUS_LIBOPUS_48000_2CH))
    )]
    #[case::profile_aiff_pcm_s16be_44100_2ch_16bit(Some(
        SignalAsset::PROFILE_AIFF_PCM_S16BE_44100_2CH_16BIT
    ))]
    #[case::profile_wav_pcm_f32le_192000_2ch_32bit(Some(
        SignalAsset::PROFILE_WAV_PCM_F32LE_192000_2CH_32BIT
    ))]
    #[case::profile_wav_pcm_s16le_192000_2ch_16bit(Some(
        SignalAsset::PROFILE_WAV_PCM_S16LE_192000_2CH_16BIT
    ))]
    #[case::profile_wav_pcm_s16le_44100_2ch_16bit(Some(
        SignalAsset::PROFILE_WAV_PCM_S16LE_44100_2CH_16BIT
    ))]
    #[case::profile_wav_pcm_s24le_44100_2ch_24bit(Some(
        SignalAsset::PROFILE_WAV_PCM_S24LE_44100_2CH_24BIT
    ))]
    #[case::profile_wav_pcm_s32le_192000_2ch_32bit(Some(
        SignalAsset::PROFILE_WAV_PCM_S32LE_192000_2CH_32BIT
    ))]
    #[cfg_attr(
        target_os = "macos",
        case::profile_ape_multiframe_44100_2ch_16bit(Some(
            SignalAsset::PROFILE_APE_MULTIFRAME_44100_2CH_16BIT
        ))
    )]
    #[case::profile_tagged_flac_id3(Some(SignalAsset::PROFILE_TAGGED_FLAC_ID3))]
    #[case::profile_tagged_mp3_id3(Some(SignalAsset::PROFILE_TAGGED_MP3_ID3))]
    #[case::profile_tagged_wave_mp3_id3(Some(SignalAsset::PROFILE_TAGGED_WAVE_MP3_ID3))]
    #[case::profile_alac_silence_tail(Some(SignalAsset::PROFILE_ALAC_SILENCE_TAIL))]
    async fn playback_advances_on_the_engine_tick_alone(
        #[case] asset: Option<SignalAsset>,
        tone_mp3: String,
        short_wav: String,
    ) {
        let tone_mp3 = asset.map_or(tone_mp3, |asset| {
            let entry = kithara_test_fixtures::assets::MANIFEST
                .iter()
                .find(|entry| entry.name == asset.name())
                .expect("registered fixture");
            let path = kithara_test_fixtures::store::file(std::path::Path::new(entry.path))
                .expect("stored fixture");
            url::Url::from_file_path(path)
                .expect("absolute fixture path")
                .into()
        });
        let rig = OffThread::spawn("engine", || Ok::<_, Infallible>(Rig::realtime()))
            .await
            .expect("rig fixture is infallible");
        rig.call(move |rig| {
            let queue = rig.queues[0].clone();
            queue
                .append(tone_mp3.as_str())
                .expect("deck A takes the track");
            queue
                .append(short_wav.as_str())
                .expect("deck A takes the next track");
            let next = queue.tracks()[1].name.clone();
            let engine_tick = |rig: &mut Rig| {
                rig.engine.tick();
                rig.engine.publish();
            };

            rig.send("deck-a/play", ControlAction::Activate);
            rig.pump();
            rig.until("the first track plays", Rig::DEADLINE, engine_tick, |rig| {
                rig.queues[0].is_playing()
                    && rig.queues[0]
                        .duration_seconds()
                        .is_some_and(|seconds| seconds > 0.0)
                    && rig.queues[0]
                        .position_seconds()
                        .is_some_and(|seconds| seconds > 0.0)
            });
            rig.send("deck-a/wave", ControlAction::SetScalar(0.9));
            rig.pump();
            // The advance this waits for is the first track reaching its own
            // end, so the budget has to cover playing it. The deck already
            // reported that length to the wait above, and reading it back is
            // what keeps the fixture's duration in one place.
            let plays_out = Rig::playout(
                rig.queues[0]
                    .duration_seconds()
                    .and_then(|seconds| time::Duration::try_from_secs_f64(seconds).ok())
                    .unwrap_or_default(),
            );
            rig.until(
                "the queue advances with no UI frame",
                plays_out,
                engine_tick,
                |rig| rig.queues[0].current_index() == Some(1),
            );

            rig.until(
                "the next snapshot names the next track",
                Rig::DEADLINE,
                |rig| {
                    engine_tick(rig);
                    rig.frame();
                },
                |rig| rig.text("deck.track.title@deck=a").as_deref() == Some(next.as_str()),
            );
        })
        .await;
        rig.close().await;
    }

    #[kithara::test(native, tokio, flash(false))]
    async fn the_loop_ends_on_the_shutdown_command_while_the_ui_holds_its_sender() {
        let rig = OffThread::spawn("engine", || Ok::<_, Infallible>(Some(Rig::offline())))
            .await
            .expect("rig fixture is infallible");
        rig.call(|slot| {
            let mut rig = slot.take().expect("the rig is set up once");
            rig.close_window();
            let Rig {
                engine,
                ui,
                commands,
                shutdown,
                ..
            } = rig;
            let shut_down = Handle::current()
                .block_on(time::timeout(
                    Rig::DEADLINE,
                    crate::engine::run(engine, commands, shutdown.child()),
                ))
                .expect("the loop returns on the shutdown command");
            assert!(shut_down, "the loop reports that the shutdown ran");
            drop(ui);
        })
        .await;
        rig.close().await;
    }

    #[kithara::test(native, tokio, flash(false))]
    async fn a_built_engine_runs_no_loop_once_the_root_stopped_waiting() {
        let rig = OffThread::spawn("engine", || Ok::<_, Infallible>(Some(Rig::offline())))
            .await
            .expect("rig fixture is infallible");
        rig.call(|slot| {
            let Rig {
                engine,
                ui,
                commands,
                shutdown,
                ..
            } = slot.take().expect("the rig is set up once");
            let (built_tx, built) = oneshot::channel();
            drop(built);
            Handle::current()
                .block_on(time::timeout(
                    Rig::DEADLINE,
                    serve(move || Ok(engine), built_tx, commands, shutdown.child()),
                ))
                .expect("the loop would run while the UI holds its sender");
            drop(ui);
        })
        .await;
        rig.close().await;
    }
}
