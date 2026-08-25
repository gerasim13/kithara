const COCHLEA_SHA: &str = "faf7517df90a586f1459fdb0519b9a20d8dabd99";
const PR_118_SHA: &str = "820388954cb43be8560101293e75d7da7b20ce8c";
const PR_150_SHA: &str = "b93921fc97dedd4a43a40a2788f73ad072372019";
const PR_187_SHA: &str = "ccde033c8f4c3e958349d6ab903782fdce8fbd26";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum OracleState {
    ActiveOracleSelfTest,
    BlockedProduct,
    BlockedFixture,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum ActivationWave {
    Foundation,
    ResidentPlan,
    QueueAdapter,
    AppToggle,
    Acceptance,
}

#[derive(Clone, Debug)]
#[non_exhaustive]
pub enum OracleSource {
    Frozen {
        pr: Option<u16>,
        sha: &'static str,
        path: &'static str,
        test: String,
    },
    ConfirmedGap {
        gap: &'static str,
    },
}

#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct OracleRegistration {
    oracle_id: String,
    source: OracleSource,
    observable: &'static str,
    preserved_contract: &'static str,
    destination_path: &'static str,
    destination_test: String,
    state: OracleState,
    activation_wave: ActivationWave,
}

impl OracleRegistration {
    #[must_use]
    pub fn oracle_id(&self) -> &str {
        &self.oracle_id
    }

    #[must_use]
    pub const fn state(&self) -> OracleState {
        self.state
    }

    #[must_use]
    pub const fn activation_wave(&self) -> ActivationWave {
        self.activation_wave
    }

    /// Returns the frozen source or confirmed coverage gap.
    #[must_use]
    pub const fn source(&self) -> &OracleSource {
        &self.source
    }

    #[must_use]
    pub fn has_complete_provenance(&self) -> bool {
        let source_complete = match &self.source {
            OracleSource::Frozen {
                sha, path, test, ..
            } => !sha.is_empty() && !path.is_empty() && !test.is_empty(),
            OracleSource::ConfirmedGap { gap } => !gap.is_empty(),
        };
        source_complete
            && !self.oracle_id.is_empty()
            && !self.observable.is_empty()
            && !self.preserved_contract.is_empty()
            && !self.destination_path.is_empty()
            && !self.destination_test.is_empty()
    }
}

#[must_use]
pub fn registrations() -> Vec<OracleRegistration> {
    let mut rows = active_oracle_rows();
    rows.extend(asset_provider_rows());
    rows.extend(sync_group_rows());
    rows.extend(beat_map_rows());
    rows.extend(transport_rows());
    rows.extend(cochlea_product_rows());
    rows.extend(renderer_source_rows());
    rows
}

fn active_oracle_rows() -> Vec<OracleRegistration> {
    let mut rows = vec![
        frozen(FrozenRegistration {
            oracle_id: "SYNC-ORACLE-001",
            pr: None,
            sha: COCHLEA_SHA,
            source_path: "tests/src/sync_matrix/oracle.rs",
            source_test: "continuity_comparator_rejects_one_render_quantum_dropout",
            observable: "a 512-frame dropout creates an additional Cochlea silence segment",
            preserved_contract: "render quantum 512; independent Cochlea control; final interleaved PCM",
            destination_path: "tests/src/sync_matrix/oracle.rs",
            destination_test: "continuity_comparator_rejects_one_render_quantum_dropout",
            state: OracleState::ActiveOracleSelfTest,
            activation_wave: ActivationWave::Foundation,
        }),
        frozen(FrozenRegistration {
            oracle_id: "SYNC-ORACLE-002",
            pr: None,
            sha: COCHLEA_SHA,
            source_path: "tests/src/sync_matrix/oracle.rs",
            source_test: "continuity_comparator_rejects_one_missing_rhythmic_event",
            observable: "a removed rhythmic event changes the independent onset count",
            preserved_contract: "120 BPM calibration; event removal; Cochlea onset timeline",
            destination_path: "tests/src/sync_matrix/oracle.rs",
            destination_test: "continuity_comparator_rejects_one_missing_rhythmic_event",
            state: OracleState::ActiveOracleSelfTest,
            activation_wave: ActivationWave::Foundation,
        }),
        frozen(FrozenRegistration {
            oracle_id: "SYNC-ORACLE-003",
            pr: None,
            sha: COCHLEA_SHA,
            source_path: "tests/src/sync_matrix/oracle.rs",
            source_test: "post_sync_tempo_oracle_rejects_120_bpm_for_127_bpm_target",
            observable: "post-SYNC tempo is measured from PCM instead of trusted metadata",
            preserved_contract: "120 BPM calibration; 127 BPM mismatch; exact nearest representable Cochlea tempo bin",
            destination_path: "tests/src/sync_matrix/oracle.rs",
            destination_test: "post_sync_tempo_oracle_rejects_120_bpm_for_127_bpm_target",
            state: OracleState::ActiveOracleSelfTest,
            activation_wave: ActivationWave::Foundation,
        }),
        frozen(FrozenRegistration {
            oracle_id: "SYNC-ORACLE-004",
            pr: None,
            sha: COCHLEA_SHA,
            source_path: "tests/src/cochlea.rs",
            source_test: "comparator_rejects_one_missing_quantum_and_one_clipped_frame",
            observable: "shared continuity comparator rejects dropout and clipped PCM",
            preserved_contract: "512 missing frames; exactly one clipped interleaved frame",
            destination_path: "tests/src/cochlea.rs",
            destination_test: "comparator_rejects_one_missing_quantum_and_one_clipped_frame",
            state: OracleState::ActiveOracleSelfTest,
            activation_wave: ActivationWave::Foundation,
        }),
        frozen(FrozenRegistration {
            oracle_id: "SYNC-ORACLE-005",
            pr: None,
            sha: COCHLEA_SHA,
            source_path: "tests/src/cochlea.rs",
            source_test: "loudness_fields_match_the_cochlea_probe",
            observable: "serialized loudness fields retain Cochlea probe semantics",
            preserved_contract: "48 kHz stereo PCM; LUFS; sample peak; true peak",
            destination_path: "tests/src/cochlea.rs",
            destination_test: "loudness_fields_match_the_cochlea_probe",
            state: OracleState::ActiveOracleSelfTest,
            activation_wave: ActivationWave::Foundation,
        }),
    ];
    rows.push(gap(
        "SYNC-ORACLE-006",
        "exact one-frame stamp/PCM shift",
        "an activation stamp shifted by exactly one frame is rejected",
        "exact admitted MapStamp and activation frame; audible phase tolerance remains one frame",
        "tests/src/sync_matrix/oracle.rs",
        "exact_stamp_oracle_rejects_one_frame_activation_shift",
    ));
    rows.push(gap(
        "SYNC-ORACLE-007",
        "one-beat/bar-phase error",
        "a one-beat ordinal shift is rejected even when tempo remains correct",
        "exact BeatOrdinal and Meter downbeat phase",
        "tests/src/sync_matrix/oracle.rs",
        "exact_phase_oracle_rejects_one_beat_bar_phase_error",
    ));
    rows
}

fn asset_provider_rows() -> Vec<OracleRegistration> {
    let behaviors = [
        "four_deck_sync",
        "paused_sync",
        "play_seek_sync",
        "play_sync_seek",
        "seek_play_sync",
        "seek_sync_play",
        "sequential_sync",
        "sync_play_seek",
        "sync_seek_play",
        "tempo_down_30hz",
        "tempo_up_120hz",
    ];
    let mut rows = behaviors
        .iter()
        .copied()
        .enumerate()
        .map(|(index, suffix)| {
            gap(
                &format!("SYNC-ASSET-{:03}", index + 1),
                "Player/Queue oracle lacked an independent known-good asset provider",
                "the shared signal oracle accepts a prepared aligned asset for the paired behavioral case",
                "same SyncCase and SignalOracle; direct Resource decode; canonical cached FLAC fixture pipeline",
                "tests/tests/kithara_queue/sync_behavioral_matrix.rs",
                &format!("prepared_assets_validate_each_behavioral_oracle_case_{suffix}"),
            )
        })
        .collect::<Vec<_>>();
    rows.extend(
        behaviors
            .iter()
            .copied()
            .enumerate()
            .map(|(index, suffix)| {
                gap(
                    &format!("SYNC-ASSET-UNSYNC-{:03}", index + 1),
                    "a green oracle case did not prove that the same oracle rejects an unsynchronized asset",
                    "the paired known-bad asset is rejected specifically by post-SYNC phase evidence",
                    "same SyncCase and SignalOracle; 1024-frame phase defect beyond the 512-frame budget; no unrelated failure category",
                    "tests/tests/kithara_queue/sync_behavioral_matrix.rs",
                    &format!(
                        "prepared_unsynced_assets_are_rejected_for_each_behavioral_oracle_case_{suffix}"
                    ),
                )
            }),
    );
    let defects = [
        ("one_frame", "exact one-frame activation defect"),
        ("beat_ordinal", "one-bar beat ordinal defect"),
        ("bar_phase", "one-beat bar-phase defect"),
        ("drift", "two-BPM rhythmic drift"),
        ("discontinuity", "one missing rhythmic event"),
    ];
    rows.extend(
        defects
            .into_iter()
            .enumerate()
            .map(|(index, (suffix, defect))| {
                gap(
                    &format!("SYNC-ASSET-NEG-{:03}", index + 1),
                    "Player/Queue oracle lacked independent known-bad prepared assets",
                    "the shared signal oracle rejects a prepared asset with the named defect",
                    defect,
                    "tests/tests/kithara_queue/sync_behavioral_matrix.rs",
                    &format!(
                        "signal_oracle_negative_controls_are_rejected_for_the_intended_reason_{suffix}"
                    ),
                )
            }),
    );
    rows
}

fn sync_group_rows() -> Vec<OracleRegistration> {
    const PATH: &str = "crates/kithara-audio/tests/sync_group_contract.rs";
    const CONTRACT: &str =
        "heterogeneous BeatMap/SyncGroup tree; immutable alignment; topology revision";
    [
        (
            "SYNC-GROUP-001",
            "member_routes_through_nested_root",
            PATH,
            "group_promotes_map_geometry_without_self_membership",
            "a promoted group owns coordinates without a leader or self-member",
        ),
        (
            "SYNC-GROUP-002",
            "erased_nested_component_keeps_one_root_transaction_owner",
            PATH,
            "nested_groups_and_maps_share_one_object_safe_contract",
            "ordinary maps and nested groups use one object-safe contract",
        ),
        (
            "SYNC-GROUP-003",
            "registration_rejects_duplicate_canonical_player",
            PATH,
            "topology_rejects_self_membership",
            "a group map cannot be inserted as its own leaf",
        ),
        (
            "SYNC-GROUP-004",
            "member_routes_through_nested_root",
            PATH,
            "topology_rejects_cycle_through_a_nested_group",
            "a nested path cannot return to the root map",
        ),
        (
            "SYNC-GROUP-005",
            "component_with_multiple_nested_roots_is_rejected",
            PATH,
            "topology_rejects_a_nested_group_with_two_parent_edges",
            "one nested group cannot have two parent edges",
        ),
        (
            "SYNC-GROUP-006",
            "registration_rejects_duplicate_canonical_player",
            PATH,
            "topology_rejects_one_leaf_repeated_across_nested_groups",
            "one leaf identity cannot occur on two nested paths",
        ),
        (
            "SYNC-GROUP-007",
            "registration_advances_typed_topology_revision",
            "crates/kithara-play/src/session/sync/tests.rs",
            "topology_batch_publishes_one_revision_with_only_the_replacement",
            "ordered edits prepare one later immutable candidate",
        ),
        (
            "SYNC-GROUP-008",
            "active_transaction_owns_the_composition_topology",
            "crates/kithara-play/src/session/sync/tests.rs",
            "unknown_member_detach_leaves_the_published_snapshot_unchanged",
            "a rejected edit sequence cannot partially publish",
        ),
        (
            "SYNC-GROUP-009",
            "registration_freezes_the_validated_canonical_player_leaves",
            PATH,
            "topology_rejects_a_stale_parent_alignment",
            "alignment edges are frozen to the exact parent map revision",
        ),
        (
            "SYNC-GROUP-010",
            "registration_freezes_the_validated_canonical_player_leaves",
            PATH,
            "topology_rejects_a_stale_member_alignment",
            "alignment edges are frozen to the exact member map revision",
        ),
        (
            "SYNC-GROUP-011",
            "registration_advances_typed_topology_revision",
            "crates/kithara-play/src/session/sync/tests.rs",
            "stale_topology_base_leaves_the_published_snapshot_unchanged",
            "membership edits are stamped against one topology revision",
        ),
    ]
    .into_iter()
    .map(|(id, old, destination_path, new, observable)| {
        frozen(FrozenRegistration {
            oracle_id: id,
            pr: Some(118),
            sha: PR_118_SHA,
            source_path: "crates/kithara-play/src/player/multi/tests.rs",
            source_test: old,
            observable,
            preserved_contract: CONTRACT,
            destination_path,
            destination_test: new,
            state: OracleState::ActiveOracleSelfTest,
            activation_wave: ActivationWave::Foundation,
        })
    })
    .collect()
}

fn beat_map_rows() -> Vec<OracleRegistration> {
    const PATH: &str = "crates/kithara-audio/tests/beat_map_contract.rs";
    const CONTRACT: &str =
        "asset-native stamped coordinates; explicit evidence; immutable revisioned queries";
    [
        (
            "SYNC-MAP-001",
            "track_beat_map_interpolates_non_uniform_markers_and_inverts",
            "sparse_snapshot_is_honest_and_invertible",
        ),
        (
            "SYNC-MAP-002",
            "track_beat_map_does_not_extrapolate_outside_markers",
            "building_gap_is_uncovered_but_complete_extent_is_outside_domain",
        ),
        (
            "SYNC-MAP-003",
            "first_downbeat_defines_zero_and_preserves_pickup_beats",
            "pickup_ordinals_keep_canonical_downbeat_zero",
        ),
        (
            "SYNC-MAP-004",
            "track_beat_map_preserves_non_zero_source_anchor_and_meter",
            "scalar_tempo_and_segments_share_declared_topology",
        ),
        (
            "SYNC-MAP-005",
            "missing_invalid_and_tempo_only_analysis_are_sync_unavailable",
            "tempo_only_geometry_does_not_fabricate_meter",
        ),
        (
            "SYNC-MAP-006",
            "marker_past_decoded_source_end_is_rejected",
            "analyzer_snapshot_requires_marker_span_evidence",
        ),
        (
            "SYNC-MAP-007",
            "binding_keeps_signed_phase_before_non_zero_anchor",
            "asset_axis_is_independent_of_host_sample_rate",
        ),
        (
            "SYNC-MAP-008",
            "track_beat_map_source_extent_uses_decoder_rounding",
            "progressive_extrapolation_is_immediate_and_refines_immutably",
        ),
    ]
    .into_iter()
    .map(|(id, old, new)| {
        frozen(FrozenRegistration {
            oracle_id: id,
            pr: Some(118),
            sha: PR_118_SHA,
            source_path: "tests/tests/kithara_play/track_binding.rs",
            source_test: old,
            observable: "legacy track-grid observable is owned by the canonical BeatMap contract",
            preserved_contract: CONTRACT,
            destination_path: PATH,
            destination_test: new,
            state: OracleState::ActiveOracleSelfTest,
            activation_wave: ActivationWave::Foundation,
        })
    })
    .collect()
}

fn transport_rows() -> Vec<OracleRegistration> {
    const PATH: &str = "crates/kithara-play/src/session/transport/tests.rs";
    const CONTRACT: &str =
        "one committed transport snapshot carries tempo, anchor, map stamp, epoch, and revision";
    [
        (
            "SYNC-TRANSPORT-001",
            "scheduled_tempo_change_is_exact_and_offline_partition_independent",
            "tempo_commit_waits_for_the_matching_render_boundary",
        ),
        (
            "SYNC-TRANSPORT-002",
            "tempo_change_preserves_beat_and_changes_slope_at_scheduled_boundary",
            "relocation_commit_reanchors_the_exact_target_beat",
        ),
        (
            "SYNC-TRANSPORT-003",
            "tempo_revision_is_not_observed_before_render_commit",
            "transport_commit_publishes_anchor_and_map_stamp_atomically",
        ),
        (
            "SYNC-TRANSPORT-004",
            "setting_the_same_tempo_does_not_create_a_new_revision",
            "transport_abort_is_idempotent",
        ),
        (
            "SYNC-TRANSPORT-005",
            "manual_session_transport_is_shared_and_render_driven",
            "route_restart_advances_host_epoch_and_map_revision",
        ),
    ]
    .into_iter()
    .map(|(id, old, new)| {
        frozen(FrozenRegistration {
            oracle_id: id,
            pr: Some(118),
            sha: PR_118_SHA,
            source_path: "tests/tests/kithara_play/session_transport.rs",
            source_test: old,
            observable: "render-bound transport publication remains exact and revisioned",
            preserved_contract: CONTRACT,
            destination_path: PATH,
            destination_test: new,
            state: OracleState::ActiveOracleSelfTest,
            activation_wave: ActivationWave::Foundation,
        })
    })
    .collect()
}

fn cochlea_product_rows() -> Vec<OracleRegistration> {
    let mut rows = Vec::new();
    let synthetic = [
        "four_deck_sync",
        "paused_sync",
        "play_seek_sync",
        "play_sync_seek",
        "seek_play_sync",
        "seek_sync_play",
        "sequential_sync",
        "sync_play_seek",
        "sync_seek_play",
        "tempo_down_30hz",
        "tempo_up_120hz",
    ];
    for (index, suffix) in synthetic.into_iter().enumerate() {
        let test = format!("synthetic_behavioral_matrix_uses_final_pcm_and_cochlea_{suffix}");
        rows.push(frozen_owned_test(FrozenProductRegistration {
            oracle_id: format!("SYNC-PRODUCT-SYN-{:03}", index + 1),
            sha: COCHLEA_SHA,
            path: "tests/tests/kithara_queue/sync_behavioral_matrix.rs",
            test,
            observable:
                "synthetic Queue/Player lifecycle produces continuous tempo- and phase-locked PCM",
            preserved_contract:
                "44.1/48 kHz; 2/4 decks; operation ordering; 30/60/120 Hz tempo ride; six beats",
            state: OracleState::BlockedProduct,
            activation_wave: ActivationWave::QueueAdapter,
        }));
    }

    let media_behaviors = [
        "four_decks",
        "paused",
        "play_seek_sync",
        "play_sync_seek",
        "seek_play_sync",
        "seek_sync_play",
        "sequential",
        "sync_play_seek",
        "sync_seek_play",
        "tempo_down_30",
        "tempo_up_120",
    ];
    let media_kinds = ["hls_mp3", "hls_same", "mp3_distinct", "mp3_same"];
    let mut index = 1;
    for kind in media_kinds {
        for behavior in media_behaviors {
            let test = format!("media_source_axis_runs_the_full_behavioral_row_{kind}_{behavior}");
            rows.push(frozen_owned_test(FrozenProductRegistration {
                oracle_id: format!("SYNC-PRODUCT-MEDIA-{index:03}"),
                sha: COCHLEA_SHA,
                path: "tests/tests/kithara_queue/sync_media.rs",
                test,
                observable:
                    "real file/HLS source combinations retain the complete behavioral oracle",
                preserved_contract:
                    "HLS same; MP3 same/distinct; HLS+MP3; ABR switch; source-native analysis",
                state: OracleState::BlockedProduct,
                activation_wave: ActivationWave::QueueAdapter,
            }));
            index += 1;
        }
    }

    for (index, suffix) in media_behaviors.into_iter().enumerate() {
        let test = format!("opt_in_library_pair_runs_the_full_behavioral_row_{suffix}");
        rows.push(frozen_owned_test(FrozenProductRegistration {
            oracle_id: format!("SYNC-PRODUCT-LIB-{:03}", index + 1),
            sha: COCHLEA_SHA,
            path: "tests/tests/kithara_queue/sync_library.rs",
            test,
            observable:
                "two content-distinct local-library records run the complete behavioral oracle",
            preserved_contract:
                "explicit KITHARA_SYNC_LIBRARY fixture; deterministic seed; content-addressed analysis",
            state: OracleState::BlockedFixture,
            activation_wave: ActivationWave::Acceptance,
        }));
    }

    for (index, test) in [
        "bound_tempo_retarget_reaches_pcm_within_twenty_ms",
        "latest_sync_target_wins_in_pcm",
        "running_sync_command_changes_audible_pcm_within_one_block",
    ]
    .into_iter()
    .enumerate()
    {
        rows.push(frozen_owned_test(FrozenProductRegistration {
            oracle_id: format!("SYNC-PRODUCT-LATENCY-{:03}", index + 1),
            sha: COCHLEA_SHA,
            path: "tests/tests/kithara_queue/sync_latency.rs",
            test: test.to_owned(),
            observable: "sync control reaches resident PCM within the frozen callback deadline",
            preserved_contract:
                "128/256/512-frame blocks; early/middle/late phase; final PCM and exact frame ledger",
            state: OracleState::BlockedProduct,
            activation_wave: ActivationWave::ResidentPlan,
        }));
    }
    for (index, test) in [
        "bound_sync_pcm_stays_clean_under_shared_worker_deadline_load",
        "bound_sync_render_is_rtsan_clean",
    ]
    .into_iter()
    .enumerate()
    {
        rows.push(frozen_owned_test(FrozenProductRegistration {
            oracle_id: format!("SYNC-PRODUCT-RT-{:03}", index + 1),
            sha: COCHLEA_SHA,
            path: "tests/tests/kithara_queue/sync_rt.rs",
            test: test.to_owned(),
            observable: "resident bound render remains continuous and real-time safe",
            preserved_contract:
                "shared worker deadline load; RTSan; underrun and Cochlea continuity evidence",
            state: OracleState::BlockedProduct,
            activation_wave: ActivationWave::ResidentPlan,
        }));
    }
    rows.push(OracleRegistration {
        oracle_id: "SYNC-PRODUCT-UI-001".to_owned(),
        source: OracleSource::ConfirmedGap {
            gap: "the application has no public AppToggle contract or sync state endpoint",
        },
        observable:
            "raw UI controls create and reuse one Session-owned group without replacing tracks",
        preserved_contract:
            "real Queue/Player path; two analysed tracks; sync lights; Cochlea deck and mix captures",
        destination_path: "tests/tests/kithara_app/sync.rs",
        destination_test: "raw_sync_controls_adopt_one_grid_and_bind_the_actual_tracks".to_owned(),
        state: OracleState::BlockedProduct,
        activation_wave: ActivationWave::AppToggle,
    });
    rows
}

fn renderer_source_rows() -> Vec<OracleRegistration> {
    [
        (
            "SYNC-LEGACY-150-EXACT",
            Some(150),
            PR_150_SHA,
            "crates/kithara-audio/src/tempo/bound/tests.rs",
            "two_decks_mid_block_at_different_offsets_apply_one_commit_at_the_same_output_frame",
            "tests/tests/kithara_queue/sync_latency.rs",
            "bound_tempo_retarget_reaches_pcm_within_twenty_ms",
            ActivationWave::ResidentPlan,
        ),
        (
            "SYNC-LEGACY-150-RUNNING",
            Some(150),
            PR_150_SHA,
            "tests/tests/kithara_app/beat_match.rs",
            "two_matched_records_strike_together",
            "tests/tests/kithara_queue/sync_media.rs",
            "media_source_axis_runs_the_full_behavioral_row",
            ActivationWave::QueueAdapter,
        ),
        (
            "SYNC-LEGACY-187-BOUNDARY",
            Some(187),
            PR_187_SHA,
            "crates/kithara-audio/src/effects/timestretch/processor/tests/lifecycle.rs",
            "repeated_off_rt_service_preserves_the_exact_decoder_boundary",
            "tests/tests/kithara_queue/sync_rt.rs",
            "bound_sync_render_is_rtsan_clean",
            ActivationWave::ResidentPlan,
        ),
        (
            "SYNC-LEGACY-187-DEADLINE",
            Some(187),
            PR_187_SHA,
            "crates/kithara-audio/src/renderer/presentation/tests/tempo.rs",
            "deep_raw_buffer_does_not_become_post_effect_latency",
            "tests/tests/kithara_queue/sync_rt.rs",
            "bound_sync_pcm_stays_clean_under_shared_worker_deadline_load",
            ActivationWave::ResidentPlan,
        ),
        (
            "SYNC-LEGACY-187-NO-SYNC",
            Some(187),
            PR_187_SHA,
            "tests/tests/kithara_audio/no_sync_passthrough.rs",
            "no_sync_unity_playback_is_bit_exact_and_cochlea_clean_under_load",
            "tests/tests/kithara_audio/no_sync_passthrough.rs",
            "no_sync_unity_playback_is_bit_exact_and_cochlea_clean_under_load",
            ActivationWave::ResidentPlan,
        ),
    ]
    .into_iter()
    .map(|(id, pr, sha, old_path, old_test, new_path, new_test, wave)| {
        frozen(FrozenRegistration {
            oracle_id: id,
            pr,
            sha,
            source_path: old_path,
            source_test: old_test,
            observable: "legacy renderer/session observable is retained without its old owner",
            preserved_contract:
                "resident one-reader path; exact activation/frontier; continuous PCM; no fallback",
            destination_path: new_path,
            destination_test: new_test,
            state: OracleState::BlockedProduct,
            activation_wave: wave,
        })
    })
    .collect()
}

#[derive(Clone, Copy)]
struct FrozenRegistration {
    oracle_id: &'static str,
    pr: Option<u16>,
    sha: &'static str,
    source_path: &'static str,
    source_test: &'static str,
    observable: &'static str,
    preserved_contract: &'static str,
    destination_path: &'static str,
    destination_test: &'static str,
    state: OracleState,
    activation_wave: ActivationWave,
}

fn frozen(row: FrozenRegistration) -> OracleRegistration {
    OracleRegistration {
        oracle_id: row.oracle_id.to_owned(),
        source: OracleSource::Frozen {
            pr: row.pr,
            sha: row.sha,
            path: row.source_path,
            test: row.source_test.to_owned(),
        },
        observable: row.observable,
        preserved_contract: row.preserved_contract,
        destination_path: row.destination_path,
        destination_test: row.destination_test.to_owned(),
        state: row.state,
        activation_wave: row.activation_wave,
    }
}

struct FrozenProductRegistration {
    oracle_id: String,
    sha: &'static str,
    path: &'static str,
    test: String,
    observable: &'static str,
    preserved_contract: &'static str,
    state: OracleState,
    activation_wave: ActivationWave,
}

fn frozen_owned_test(row: FrozenProductRegistration) -> OracleRegistration {
    OracleRegistration {
        oracle_id: row.oracle_id,
        source: OracleSource::Frozen {
            pr: None,
            sha: row.sha,
            path: row.path,
            test: row.test.clone(),
        },
        observable: row.observable,
        preserved_contract: row.preserved_contract,
        destination_path: row.path,
        destination_test: row.test,
        state: row.state,
        activation_wave: row.activation_wave,
    }
}

fn gap(
    oracle_id: &str,
    gap: &'static str,
    observable: &'static str,
    preserved_contract: &'static str,
    destination_path: &'static str,
    destination_test: &str,
) -> OracleRegistration {
    OracleRegistration {
        oracle_id: oracle_id.to_owned(),
        source: OracleSource::ConfirmedGap { gap },
        observable,
        preserved_contract,
        destination_path,
        destination_test: destination_test.to_owned(),
        state: OracleState::ActiveOracleSelfTest,
        activation_wave: ActivationWave::Foundation,
    }
}
