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

/// Number of unique tests in the frozen PR #118 source universe.
pub const PR_118_TEST_COUNT: usize = 181;

/// How one frozen PR #118 test is accounted for in the current foundation.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
#[non_exhaustive]
pub enum Pr118Disposition {
    /// The same test is active under the same observable contract.
    ExactActive,
    /// A current active test proves an equivalent or stronger contract.
    EquivalentActive,
    /// A current ignored red oracle preserves the future contract.
    EquivalentIgnoredRed,
    /// The source test still needs an active executable transfer.
    TransferActive,
    /// The source test still needs an ignored red-oracle transfer.
    TransferIgnoredRed,
    /// The source test must not be transplanted into the current architecture.
    NonTransplant,
}

/// Row-level provenance for one frozen PR #118 test.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct Pr118Provenance {
    source_path: &'static str,
    source_test: &'static str,
    disposition: Pr118Disposition,
    destination_path: &'static str,
    destination_test: &'static str,
    note: &'static str,
}

impl Pr118Provenance {
    /// Returns the frozen source file.
    #[must_use]
    pub const fn source_path(&self) -> &'static str {
        self.source_path
    }

    /// Returns the frozen source test name.
    #[must_use]
    pub const fn source_test(&self) -> &'static str {
        self.source_test
    }

    /// Returns how this source test is accounted for.
    #[must_use]
    pub const fn disposition(&self) -> Pr118Disposition {
        self.disposition
    }

    /// Returns the current or planned target file.
    #[must_use]
    pub const fn destination_path(&self) -> &'static str {
        self.destination_path
    }

    /// Returns the current or planned target test name.
    #[must_use]
    pub const fn destination_test(&self) -> &'static str {
        self.destination_test
    }

    /// Returns the row-level disposition rationale.
    #[must_use]
    pub const fn note(&self) -> &'static str {
        self.note
    }

    /// Returns whether every provenance field is concrete and non-placeholder.
    #[must_use]
    pub fn has_complete_provenance(&self) -> bool {
        [
            self.source_path,
            self.source_test,
            self.destination_path,
            self.destination_test,
            self.note,
        ]
        .into_iter()
        .all(|value| !value.trim().is_empty() && !is_placeholder(value))
    }
}

const CURRENT_ACTIVE: &str = "current active test preserves the source observable";
const CURRENT_IGNORED_RED: &str =
    "current ignored red oracle preserves the source observable until product activation";
const TRANSFER_ACTIVE: &str =
    "transfer as an active green control; this ledger is not the executable test";
const TRANSFER_IGNORED_RED: &str =
    "transfer as an ignored red oracle; this ledger is not the executable test";
const NON_TRANSPLANT_WINDOW: &str =
    "do not transplant the retired mutable window owner; the immutable plan contract is stronger";
const NON_TRANSPLANT_DSP: &str = "do not transplant the retired DSP copy loop; the backend contract and final PCM oracle are stronger";
const NON_TRANSPLANT_GATE: &str =
    "do not restore the untimed gate API; the bounded edge and wake tests are stronger";
const NON_TRANSPLANT_RANGE: &str = "do not restore the non-empty range rule; an empty retained window is now a typed pending state";
const NON_TRANSPLANT_LAYOUT: &str = "do not transplant an inline storage limit as a domain contract; the current snapshot contract is stronger";

const AP: &str = "crates/kithara-audio/tests/alignment_plan_contract.rs";
const BM: &str = "crates/kithara-audio/tests/beat_map_contract.rs";
const SG: &str = "crates/kithara-audio/tests/sync_group_contract.rs";
const SO: &str = "crates/kithara-audio/tests/sync_operation_contract.rs";
const SS: &str = "crates/kithara-play/src/session/sync/tests.rs";
const ST: &str = "crates/kithara-play/src/session/transport/tests.rs";
const IT: &str = "tests/tests/kithara_play/session_transport.rs";
const EL: &str = "crates/kithara-stretch/tests/elastic.rs";
const ES: &str = "crates/kithara-stretch/tests/elastic_span.rs";
const NSP: &str = "tests/tests/kithara_audio/no_sync_passthrough.rs";
const QBM: &str = "tests/tests/kithara_queue/sync_behavioral_matrix.rs";
const QL: &str = "tests/tests/kithara_queue/sync_latency.rs";
const QRT: &str = "tests/tests/kithara_queue/sync_rt.rs";
const QM: &str = "tests/tests/kithara_queue/sync_media.rs";

fn is_placeholder(value: &str) -> bool {
    ["none", "todo", "tbd", "unknown"]
        .into_iter()
        .any(|placeholder| value.eq_ignore_ascii_case(placeholder))
}

macro_rules! pr_118_rows {
    (
        $(
            $source_path:literal, $source_test:literal => $disposition:ident,
            $destination_path:expr, $destination_test:literal, $note:expr;
        )*
    ) => {
        &[
            $(
                Pr118Provenance {
                    source_path: $source_path,
                    source_test: $source_test,
                    disposition: Pr118Disposition::$disposition,
                    destination_path: $destination_path,
                    destination_test: $destination_test,
                    note: $note,
                },
            )*
        ]
    };
}

const PR_118_ROWS: &[Pr118Provenance] = pr_118_rows! {
    "crates/kithara-audio/src/analysis/analyzer/set.rs", "analysis_rejects_mid_pass_format_change" => TransferIgnoredRed, "crates/kithara-audio/tests/analysis_stream_contract.rs", "analysis_rejects_mid_pass_format_change", TRANSFER_IGNORED_RED;
    "crates/kithara-audio/src/analysis/analyzer/set.rs", "analysis_rejects_source_frame_count_overflow" => TransferIgnoredRed, "crates/kithara-audio/tests/analysis_stream_contract.rs", "analysis_rejects_source_frame_count_overflow", TRANSFER_IGNORED_RED;
    "crates/kithara-audio/src/audio/core.rs", "bounded_range_copies_exactly_across_decoder_chunks" => NonTransplant, AP, "peek_is_pure_and_render_commit_advances_only_the_rendered_frontier", NON_TRANSPLANT_WINDOW;
    "crates/kithara-audio/src/audio/core.rs", "bounded_range_defers_peer_wake_to_the_worker_shell" => NonTransplant, AP, "missing_future_source_is_pending_without_progress", NON_TRANSPLANT_WINDOW;
    "crates/kithara-audio/src/audio/core.rs", "bounded_range_recycles_a_malformed_chunk_before_returning_error" => NonTransplant, AP, "evicted_required_source_is_behind_window_without_progress", NON_TRANSPLANT_WINDOW;
    "crates/kithara-audio/src/audio/core.rs", "bounded_range_reports_eof_without_replaying_partial_pcm" => NonTransplant, AP, "finite_coverage_distinguishes_exhaustion_from_completion", NON_TRANSPLANT_WINDOW;
    "crates/kithara-audio/src/audio/core.rs", "ordinary_seek_invalidates_a_bounded_request_and_restores_linear_mode" => NonTransplant, AP, "cursor_from_another_plan_revision_is_stale", NON_TRANSPLANT_WINDOW;
    "crates/kithara-audio/src/audio/core.rs", "transport_bend_control_updates_consume_rate" => EquivalentIgnoredRed, QL, "running_sync_command_changes_audible_pcm_within_one_block", CURRENT_IGNORED_RED;
    "crates/kithara-audio/src/audio/cursor.rs", "bend_scales_source_cursor_and_playhead_slope" => EquivalentIgnoredRed, QL, "running_sync_command_changes_audible_pcm_within_one_block", CURRENT_IGNORED_RED;
    "crates/kithara-audio/src/audio/cursor.rs", "bend_step_applies_to_next_read" => EquivalentIgnoredRed, QL, "latest_sync_target_wins_in_pcm", CURRENT_IGNORED_RED;
    "crates/kithara-audio/src/audio/cursor.rs", "overall_audible_rate_is_capped" => EquivalentIgnoredRed, QBM, "synthetic_behavioral_matrix_uses_final_pcm_and_cochlea", CURRENT_IGNORED_RED;
    "crates/kithara-audio/src/audio/cursor.rs", "unity_is_bit_exact_for_interleaved_and_planar_reads" => EquivalentActive, NSP, "no_sync_unity_playback_is_bit_exact_and_cochlea_clean_under_load", CURRENT_ACTIVE;
    "crates/kithara-audio/src/audio/cursor.rs", "varispeed_carries_residual_across_chunk_seam" => NonTransplant, QRT, "bound_sync_pcm_stays_clean_under_shared_worker_deadline_load", NON_TRANSPLANT_DSP;
    "crates/kithara-audio/src/audio/cursor.rs", "varispeed_interpolates_across_chunk_seams" => NonTransplant, QBM, "synthetic_behavioral_matrix_uses_final_pcm_and_cochlea", NON_TRANSPLANT_DSP;
    "crates/kithara-audio/src/audio/ring.rs", "seek_recycles_current_chunk_and_varispeed_lookahead" => NonTransplant, AP, "evicted_required_source_is_behind_window_without_progress", NON_TRANSPLANT_WINDOW;
    "crates/kithara-audio/src/effects/transport.rs", "bend_multiplier_is_clamped_and_strictly_positive" => EquivalentActive, IT, "tempo_rejects_values_outside_the_representable_range", CURRENT_ACTIVE;
    "crates/kithara-audio/src/effects/transport.rs", "defaults_to_unity" => EquivalentActive, NSP, "no_sync_unity_playback_is_bit_exact_and_cochlea_clean_under_load", CURRENT_ACTIVE;
    "crates/kithara-audio/src/elastic/reader/preparation.rs", "decoded_frontier_uses_the_directional_window_extent" => NonTransplant, AP, "missing_future_source_is_pending_without_progress", NON_TRANSPLANT_WINDOW;
    "crates/kithara-audio/src/elastic/reader/preparation.rs", "direction_and_rate_stress_stays_inside_configured_preparation_budgets" => NonTransplant, QRT, "bound_sync_pcm_stays_clean_under_shared_worker_deadline_load", NON_TRANSPLANT_WINDOW;
    "crates/kithara-audio/src/elastic/reader/preparation.rs", "forward_and_reverse_window_stress_never_exceeds_the_ready_bound" => NonTransplant, AP, "finite_coverage_distinguishes_exhaustion_from_completion", NON_TRANSPLANT_WINDOW;
    "crates/kithara-audio/src/elastic/reader/preparation.rs", "reverse_preparation_fetches_extra_tempo_history_as_a_separate_range" => NonTransplant, EL, "history_and_output_warmup_remove_the_initial_gap", NON_TRANSPLANT_WINDOW;
    "crates/kithara-audio/src/elastic/reader/preparation.rs", "reverse_preparation_requests_one_ascending_bounded_range" => NonTransplant, ES, "reverse_quantization_keeps_a_descending_cursor_inside_the_rate_envelope", NON_TRANSPLANT_WINDOW;
    "crates/kithara-audio/src/elastic/reader/preparation.rs", "reverse_window_renewal_keeps_the_active_window_until_successor_use" => NonTransplant, AP, "cursor_from_another_plan_revision_is_stale", NON_TRANSPLANT_WINDOW;
    "crates/kithara-audio/src/elastic/reader/rendering.rs", "forward_copy_indexes_from_the_actual_window_start" => NonTransplant, AP, "peek_is_pure_and_render_commit_advances_only_the_rendered_frontier", NON_TRANSPLANT_WINDOW;
    "crates/kithara-audio/src/elastic/reader/rendering.rs", "reverse_copy_consumes_an_ascending_window_in_descending_order" => NonTransplant, ES, "reverse_quantization_keeps_a_descending_cursor_inside_the_rate_envelope", NON_TRANSPLANT_DSP;
    "crates/kithara-audio/src/elastic/reader/rendering.rs", "reverse_copy_reaches_source_start_without_wrapping" => NonTransplant, ES, "reverse_phase_error_converges_without_a_source_jump", NON_TRANSPLANT_DSP;
    "crates/kithara-audio/src/renderer/node.rs", "decoder_node_retires_invalidated_overflow_in_the_worker_shell" => NonTransplant, AP, "cursor_from_another_plan_revision_is_stale", NON_TRANSPLANT_WINDOW;
    "crates/kithara-audio/src/source_range.rs", "source_range_requires_a_non_empty_ascending_interval" => NonTransplant, AP, "empty_retained_source_is_pending_without_progress", NON_TRANSPLANT_RANGE;
    "crates/kithara-events/src/event.rs", "transport_event_into_event" => ExactActive, "crates/kithara-events/src/event.rs", "transport_event_into_event", CURRENT_ACTIVE;
    "crates/kithara-events/src/event.rs", "sync_event_into_event" => EquivalentActive, SO, "sync_shape_exposes_each_intent_and_target", CURRENT_ACTIVE;
    "crates/kithara-ffi/src/core/convert.rs", "sync_event_to_ffi_uses_sync_vocabulary" => TransferIgnoredRed, "crates/kithara-ffi/tests/sync_contract.rs", "sync_event_to_ffi_uses_sync_vocabulary", TRANSFER_IGNORED_RED;
    "crates/kithara-ffi/src/core/convert.rs", "transport_event_to_ffi_uses_transport_vocabulary" => TransferIgnoredRed, "crates/kithara-ffi/tests/sync_contract.rs", "transport_event_to_ffi_uses_transport_vocabulary", TRANSFER_IGNORED_RED;
    "crates/kithara-ffi/src/core/convert.rs", "transport_seek_event_to_ffi_preserves_target_and_revision" => TransferIgnoredRed, "crates/kithara-ffi/tests/sync_contract.rs", "transport_seek_event_to_ffi_preserves_target_and_revision", TRANSFER_IGNORED_RED;
    "crates/kithara-platform/src/common/gate.rs", "thread_gate_signal_before_untimed_wait_is_not_lost" => NonTransplant, "crates/kithara-platform/src/common/gate.rs", "thread_gate_signal_before_wait_is_not_lost", NON_TRANSPLANT_GATE;
    "crates/kithara-platform/src/common/gate.rs", "thread_gate_untimed_wait_wakes_on_cross_thread_signal" => NonTransplant, "crates/kithara-platform/src/common/gate.rs", "thread_gate_wakes_on_cross_thread_signal", NON_TRANSPLANT_GATE;
    "crates/kithara-play/src/bridge/playback.rs", "session_seek_status_is_scoped_to_attempt_identity" => EquivalentActive, AP, "cursor_from_another_operation_with_the_same_plan_revision_is_stale", CURRENT_ACTIVE;
    "crates/kithara-play/src/engine/core.rs", "deferred_command_is_not_built_for_a_full_slot_lane" => TransferIgnoredRed, "crates/kithara-play/src/player/core.rs", "full_slot_lane_does_not_consume_queue_resource", TRANSFER_IGNORED_RED;
    "crates/kithara-play/src/engine/core.rs", "deferred_command_is_not_built_for_a_missing_slot" => EquivalentActive, "crates/kithara-play/src/player/flow/transport.rs", "select_item_on_consumed_slot_errors_without_bookkeeping", CURRENT_ACTIVE;
    "crates/kithara-play/src/player/core.rs", "rejected_load_restores_the_queue_resource" => TransferIgnoredRed, "crates/kithara-play/src/player/core.rs", "rejected_load_restores_the_queue_resource", TRANSFER_IGNORED_RED;
    "crates/kithara-play/src/player/flow/binding.rs", "cancelled_preparation_wait_returns_typed_error" => TransferIgnoredRed, "tests/tests/kithara_queue/sync_resident_plan.rs", "cancelled_preparation_wait_returns_typed_error", TRANSFER_IGNORED_RED;
    "crates/kithara-play/src/player/flow/notify.rs", "bound_track_notification_maps_to_sync_binding_event" => EquivalentActive, SO, "sync_shape_exposes_each_intent_and_target", CURRENT_ACTIVE;
    "crates/kithara-play/src/player/flow/transport.rs", "rejected_session_start_does_not_mutate_player_state" => EquivalentActive, ST, "a_failing_block_rejects_the_pending_stamp_and_still_publishes", CURRENT_ACTIVE;
    "crates/kithara-play/src/player/multi/member.rs", "join_is_available_for_canonical_player_ownership_forms" => EquivalentActive, SG, "asset_host_and_group_fake_satisfy_one_object_safe_contract", CURRENT_ACTIVE;
    "crates/kithara-play/src/player/multi/session.rs", "active_participants_exclude_registered_but_unstarted_decks" => EquivalentActive, SG, "unavailable_members_join_without_a_fabricated_alignment", CURRENT_ACTIVE;
    "crates/kithara-play/src/player/multi/session.rs", "session_seek_cancellation_is_not_blocked_by_saturated_lanes" => TransferIgnoredRed, "tests/tests/kithara_queue/sync_resident_plan.rs", "session_seek_cancellation_is_not_blocked_by_saturated_lanes", TRANSFER_IGNORED_RED;
    "crates/kithara-play/src/player/multi/tests.rs", "active_transaction_owns_the_composition_topology" => EquivalentActive, SS, "unknown_member_detach_leaves_the_published_snapshot_unchanged", CURRENT_ACTIVE;
    "crates/kithara-play/src/player/multi/tests.rs", "component_with_multiple_nested_roots_is_rejected" => EquivalentActive, SG, "topology_rejects_a_nested_group_with_two_parent_edges", CURRENT_ACTIVE;
    "crates/kithara-play/src/player/multi/tests.rs", "erased_nested_component_keeps_one_root_transaction_owner" => EquivalentActive, SG, "nested_groups_and_maps_share_one_object_safe_contract", CURRENT_ACTIVE;
    "crates/kithara-play/src/player/multi/tests.rs", "group_seek_preflights_the_shortest_member" => EquivalentIgnoredRed, QBM, "synthetic_behavioral_matrix_uses_final_pcm_and_cochlea", CURRENT_IGNORED_RED;
    "crates/kithara-play/src/player/multi/tests.rs", "member_routes_through_nested_root" => EquivalentActive, SG, "group_promotes_map_geometry_without_self_membership", CURRENT_ACTIVE;
    "crates/kithara-play/src/player/multi/tests.rs", "registration_advances_typed_topology_revision" => EquivalentActive, SS, "topology_batch_publishes_one_revision_with_only_the_replacement", CURRENT_ACTIVE;
    "crates/kithara-play/src/player/multi/tests.rs", "registration_calls_component_code_without_holding_the_state_lock" => EquivalentActive, SS, "topology_batch_publishes_one_revision_with_only_the_replacement", CURRENT_ACTIVE;
    "crates/kithara-play/src/player/multi/tests.rs", "registration_freezes_the_validated_canonical_player_leaves" => EquivalentActive, SG, "topology_rejects_a_stale_parent_alignment", CURRENT_ACTIVE;
    "crates/kithara-play/src/player/multi/tests.rs", "registration_rejects_component_without_a_canonical_player" => EquivalentActive, SO, "invalid_member_kind_preserves_group_member_and_policy", CURRENT_ACTIVE;
    "crates/kithara-play/src/player/multi/tests.rs", "registration_rejects_duplicate_canonical_player" => EquivalentActive, SG, "topology_rejects_one_leaf_repeated_across_nested_groups", CURRENT_ACTIVE;
    "crates/kithara-play/src/player/multi/tests.rs", "registration_rejects_mixed_sessions_inside_first_component" => EquivalentActive, SG, "live_member_rejects_an_alignment_from_another_target_identity", CURRENT_ACTIVE;
    "crates/kithara-play/src/player/multi/tests.rs", "registration_rejects_players_from_different_sessions" => EquivalentActive, SG, "live_member_rejects_an_alignment_from_another_target_identity", CURRENT_ACTIVE;
    "crates/kithara-play/src/player/multi/tests.rs", "routed_control_owns_the_root_topology" => EquivalentActive, SS, "host_routes_transport_to_the_canonical_deck_without_changing_root_topology", CURRENT_ACTIVE;
    "crates/kithara-play/src/player/node/processor.rs", "saturated_retirement_lane_preserves_the_pending_track_and_command_order" => TransferIgnoredRed, "tests/tests/kithara_queue/sync_resident_plan.rs", "saturated_retirement_lane_preserves_pending_plan_order", TRANSFER_IGNORED_RED;
    "crates/kithara-play/src/player/node/processor.rs", "second_session_seek_prepare_releases_the_stale_owner" => EquivalentActive, AP, "cursor_from_another_operation_with_the_same_plan_revision_is_stale", CURRENT_ACTIVE;
    "crates/kithara-play/src/player/node/processor.rs", "session_seek_cancel_clears_the_matching_preparation" => EquivalentActive, AP, "cursor_from_another_operation_with_the_same_plan_revision_is_stale", CURRENT_ACTIVE;
    "crates/kithara-play/src/player/node/processor.rs", "track_mutations_abort_a_pending_session_seek" => EquivalentActive, ST, "route_reset_rejects_pending_commit_and_reanchors_the_active_beat", CURRENT_ACTIVE;
    "crates/kithara-play/src/player/platform/native.rs", "bound_resource_type_requires_an_active_reader" => EquivalentActive, AP, "missing_future_source_is_pending_without_progress", CURRENT_ACTIVE;
    "crates/kithara-play/src/player/platform/native.rs", "first_load_transaction_removes_inserted_item_on_rejection" => TransferIgnoredRed, "tests/tests/kithara_play/resource_regressions.rs", "first_load_transaction_restores_inserted_item_on_rejection", TRANSFER_IGNORED_RED;
    "crates/kithara-play/src/player/platform/native.rs", "join_rejects_stream_shape_change_after_preparation" => EquivalentActive, BM, "external_segment_snapshots_reject_incompatible_states", CURRENT_ACTIVE;
    "crates/kithara-play/src/player/platform/native.rs", "tempo_preflight_rejects_insufficient_source_lookahead" => EquivalentActive, AP, "missing_future_source_is_pending_without_progress", CURRENT_ACTIVE;
    "crates/kithara-play/src/player/platform/native.rs", "tempo_preflight_rejects_one_unsupported_marker_segment" => EquivalentActive, BM, "external_segment_snapshots_reject_incompatible_states", CURRENT_ACTIVE;
    "crates/kithara-play/src/player/state/items.rs", "load_transaction_serializes_queue_mutation_and_restores_rejection" => TransferIgnoredRed, "crates/kithara-play/src/player/state/items.rs", "load_transaction_serializes_queue_mutation_and_restores_rejection", TRANSFER_IGNORED_RED;
    "crates/kithara-play/src/player/state/items.rs", "replacement_and_removal_invalidate_queued_binding_demand" => EquivalentActive, AP, "cursor_from_another_operation_with_the_same_plan_revision_is_stale", CURRENT_ACTIVE;
    "crates/kithara-play/src/player/state/items.rs", "taking_a_bound_resource_retains_its_queue_coordinate_metadata" => EquivalentActive, SO, "accepted_transport_preserves_every_dispatch_stamp", CURRENT_ACTIVE;
    "crates/kithara-play/src/player/track/bound_plan.rs", "accepts_a_marker_at_the_request_endpoint" => EquivalentActive, BM, "touching_segment_seams_belong_to_the_following_segment", CURRENT_ACTIVE;
    "crates/kithara-play/src/player/track/bound_plan.rs", "accepts_the_exact_upper_renderer_rate_after_float_mapping" => EquivalentActive, EL, "renders_exact_spans_at_both_declared_rate_edges", CURRENT_ACTIVE;
    "crates/kithara-play/src/player/track/bound_plan.rs", "finds_an_internal_marker_after_a_sub_frame_edge_marker" => EquivalentActive, BM, "sparse_snapshot_is_honest_and_invertible", CURRENT_ACTIVE;
    "crates/kithara-play/src/player/track/bound_plan.rs", "keeps_four_marker_segments_inline" => NonTransplant, BM, "external_segment_snapshots_reject_incompatible_states", NON_TRANSPLANT_LAYOUT;
    "crates/kithara-play/src/player/track/bound_plan.rs", "maps_only_the_requested_render_subrange" => EquivalentActive, AP, "peek_is_pure_and_render_commit_advances_only_the_rendered_frontier", CURRENT_ACTIVE;
    "crates/kithara-play/src/player/track/bound_plan.rs", "maps_track_tempo_to_a_numeric_source_span" => EquivalentActive, AP, "public_consumers_can_build_identity_plan_without_backend_capabilities", CURRENT_ACTIVE;
    "crates/kithara-play/src/player/track/bound_plan.rs", "rejects_a_rate_outside_the_renderer_envelope" => EquivalentActive, EL, "rejects_requests_outside_the_declared_rate_envelope", CURRENT_ACTIVE;
    "crates/kithara-play/src/player/track/bound_plan.rs", "rejects_an_internal_marker_boundary" => EquivalentActive, BM, "overlapping_segments_are_rejected", CURRENT_ACTIVE;
    "crates/kithara-play/src/player/track/bound_plan.rs", "reverse_source_start_keeps_the_valid_output_prefix" => EquivalentActive, ES, "reverse_quantization_keeps_a_descending_cursor_inside_the_rate_envelope", CURRENT_ACTIVE;
    "crates/kithara-play/src/player/track/bound_plan.rs", "rounds_a_marker_split_to_the_nearest_render_frame" => EquivalentActive, BM, "host_inverse_reports_frame_rounding_uncertainty", CURRENT_ACTIVE;
    "crates/kithara-play/src/player/track/bound_plan.rs", "splits_one_marker_into_ordered_continuous_segments" => EquivalentActive, BM, "sparse_snapshot_is_honest_and_invertible", CURRENT_ACTIVE;
    "crates/kithara-play/src/player/track/core.rs", "explicit_stop_clears_natural_end_retention" => TransferIgnoredRed, "crates/kithara-play/src/rt/track/core.rs", "explicit_stop_clears_natural_end_retention", TRANSFER_IGNORED_RED;
    "crates/kithara-play/src/resource/reader.rs", "into_reader_does_not_cancel_the_opened_resource_subtree" => TransferActive, "tests/tests/kithara_play/resource_regressions.rs", "into_reader_does_not_cancel_the_opened_resource_subtree", TRANSFER_ACTIVE;
    "crates/kithara-play/src/session/dispatch.rs", "preparation_context_captures_transport_stream_and_roster_revisions" => EquivalentActive, SO, "accepted_transport_preserves_every_dispatch_stamp", CURRENT_ACTIVE;
    "crates/kithara-play/src/session/dispatch.rs", "unregister_is_atomic_when_roster_revision_is_exhausted" => EquivalentActive, SS, "stale_topology_base_leaves_the_published_snapshot_unchanged", CURRENT_ACTIVE;
    "crates/kithara-play/src/session/protocol.rs", "unit_command_rejects_a_non_unit_reply" => TransferIgnoredRed, "crates/kithara-play/src/session/protocol.rs", "unit_command_rejects_a_non_unit_reply", TRANSFER_IGNORED_RED;
    "crates/kithara-play/src/session/render/tests.rs", "inactive_transport_is_a_valid_render_context" => EquivalentActive, ST, "inactive_transport_publishes_a_frozen_position", CURRENT_ACTIVE;
    "crates/kithara-play/src/session/render/tests.rs", "invalid_render_context_replaces_the_previous_snapshot" => EquivalentActive, ST, "route_reset_withdraws_snapshot_until_new_axis_is_reanchored", CURRENT_ACTIVE;
    "crates/kithara-play/src/session/render/tests.rs", "late_transport_commit_is_rejected_without_changing_the_active_commit" => ExactActive, ST, "late_transport_commit_is_rejected_without_changing_the_active_commit", CURRENT_ACTIVE;
    "crates/kithara-play/src/session/render/tests.rs", "node_local_subblock_cannot_reuse_a_stale_render_context" => EquivalentActive, ST, "discontinuous_block_start_is_rejected", CURRENT_ACTIVE;
    "crates/kithara-play/src/session/render/tests.rs", "relocation_commit_reanchors_the_exact_target_beat" => ExactActive, ST, "relocation_commit_reanchors_the_exact_target_beat", CURRENT_ACTIVE;
    "crates/kithara-play/src/session/render/tests.rs", "render_context_derives_exact_handover_subrange" => EquivalentActive, AP, "identity_plan_requires_its_exact_activation_as_the_first_output_frame", CURRENT_ACTIVE;
    "crates/kithara-play/src/session/render/tests.rs", "render_context_rejects_beats_without_a_playing_commit" => EquivalentActive, ST, "inactive_transport_publishes_a_frozen_position", CURRENT_ACTIVE;
    "crates/kithara-play/src/session/render/tests.rs", "render_context_rejects_output_range_outside_callback" => EquivalentActive, AP, "finite_coverage_distinguishes_exhaustion_from_completion", CURRENT_ACTIVE;
    "crates/kithara-play/src/session/render/tests.rs", "route_reset_rejects_pending_commit_and_reanchors_the_active_beat" => ExactActive, ST, "route_reset_rejects_pending_commit_and_reanchors_the_active_beat", CURRENT_ACTIVE;
    "crates/kithara-play/src/session/render/tests.rs", "stale_transport_commit_is_rejected_without_invalidating_the_context" => EquivalentActive, ST, "stale_transport_commit_is_rejected_without_breaking_the_clock", CURRENT_ACTIVE;
    "crates/kithara-play/src/session/render/tests.rs", "tempo_commit_waits_for_the_matching_render_boundary" => ExactActive, ST, "tempo_commit_waits_for_the_matching_render_boundary", CURRENT_ACTIVE;
    "crates/kithara-play/src/session/render/tests.rs", "transport_abort_is_idempotent" => ExactActive, ST, "transport_abort_is_idempotent", CURRENT_ACTIVE;
    "crates/kithara-play/src/session/render/tests.rs", "two_player_nodes_receive_the_same_committed_revision" => EquivalentActive, IT, "transport_commit_is_published_to_every_registered_player_bus", CURRENT_ACTIVE;
    "crates/kithara-play/src/session/render/tests.rs", "two_player_nodes_receive_the_same_render_context" => EquivalentActive, IT, "transport_position_is_independent_of_render_partitioning", CURRENT_ACTIVE;
    "crates/kithara-play/src/session/transport.rs", "preparation_rejects_while_a_future_revision_is_pending" => EquivalentActive, ST, "late_transport_commit_is_rejected_without_changing_the_active_commit", CURRENT_ACTIVE;
    "crates/kithara-play/src/session/transport.rs", "preparation_requires_a_configured_transport" => EquivalentActive, SO, "transport_shape_exposes_each_operation_and_target", CURRENT_ACTIVE;
    "crates/kithara-play/src/session/transport.rs", "preparation_uses_initial_accepted_commit_before_first_render" => EquivalentActive, ST, "transport_commit_publishes_anchor_and_map_stamp_atomically", CURRENT_ACTIVE;
    "crates/kithara-play/src/session/transport.rs", "preparation_uses_observed_active_commit" => EquivalentActive, ST, "transport_commit_publishes_anchor_and_map_stamp_atomically", CURRENT_ACTIVE;
    "crates/kithara-play/src/session/transport.rs", "relocation_commit_publishes_the_exact_seek_target" => EquivalentActive, IT, "session_seek_relocates_to_the_exact_target_beat", CURRENT_ACTIVE;
    "crates/kithara-queue/src/queue/component.rs", "queue_registers_as_a_routed_player_component" => EquivalentActive, SS, "host_routes_transport_to_the_canonical_deck_without_changing_root_topology", CURRENT_ACTIVE;
    "crates/kithara-queue/src/queue/state.rs", "from_with_params_wraps_the_supplied_player" => EquivalentActive, "tests/tests/kithara_queue/architecture_flow.rs", "queue_playback_architecture", CURRENT_ACTIVE;
    "crates/kithara-stream/src/seek_state.rs", "reposition_supersedes_application_event_delivery" => EquivalentActive, "crates/kithara-stream/src/seek_state.rs", "commit_if_epoch_runs_only_for_the_current_epoch", CURRENT_ACTIVE;
    "crates/kithara-stretch/src/elastic/config.rs", "span_config_preserves_valid_policy_values" => ExactActive, "crates/kithara-stretch/src/elastic/config.rs", "span_config_preserves_valid_policy_values", CURRENT_ACTIVE;
    "crates/kithara-stretch/src/elastic/config.rs", "span_config_requires_finite_positive_values" => ExactActive, "crates/kithara-stretch/src/elastic/config.rs", "span_config_requires_finite_positive_values", CURRENT_ACTIVE;
    "crates/kithara-stretch/src/elastic/rate.rs", "accepts_one_rounding_step_at_the_declared_rate_boundary" => ExactActive, "crates/kithara-stretch/src/elastic/rate.rs", "accepts_one_rounding_step_at_the_declared_rate_boundary", CURRENT_ACTIVE;
    "crates/kithara-stretch/src/elastic/span_tests.rs", "a_fifth_span_is_rejected_before_inline_storage_can_spill" => ExactActive, ES, "a_fifth_span_is_rejected_before_inline_storage_can_spill", CURRENT_ACTIVE;
    "crates/kithara-stretch/src/elastic/span_tests.rs", "configured_phase_limit_rejects_a_larger_cursor_error" => ExactActive, ES, "configured_phase_limit_rejects_a_larger_cursor_error", CURRENT_ACTIVE;
    "crates/kithara-stretch/src/elastic/span_tests.rs", "correction_respects_backend_rate_headroom" => ExactActive, ES, "correction_respects_backend_rate_headroom", CURRENT_ACTIVE;
    "crates/kithara-stretch/src/elastic/span_tests.rs", "negative_phase_error_converges_without_overshoot" => ExactActive, ES, "negative_phase_error_converges_without_overshoot", CURRENT_ACTIVE;
    "crates/kithara-stretch/src/elastic/span_tests.rs", "one_frame_error_is_continuous_but_larger_error_requires_relocation" => ExactActive, ES, "one_frame_error_is_continuous_but_larger_error_requires_relocation", CURRENT_ACTIVE;
    "crates/kithara-stretch/src/elastic/span_tests.rs", "one_plan_cannot_change_source_direction" => ExactActive, ES, "one_plan_cannot_change_source_direction", CURRENT_ACTIVE;
    "crates/kithara-stretch/src/elastic/span_tests.rs", "plan_limits_apply_to_the_complete_backend_block" => ExactActive, ES, "plan_limits_apply_to_the_complete_backend_block", CURRENT_ACTIVE;
    "crates/kithara-stretch/src/elastic/span_tests.rs", "planned_segment_gap_is_not_treated_as_phase_error" => ExactActive, ES, "planned_segment_gap_is_not_treated_as_phase_error", CURRENT_ACTIVE;
    "crates/kithara-stretch/src/elastic/span_tests.rs", "reverse_phase_error_converges_without_a_source_jump" => ExactActive, ES, "reverse_phase_error_converges_without_a_source_jump", CURRENT_ACTIVE;
    "crates/kithara-stretch/src/elastic/span_tests.rs", "reverse_quantization_keeps_a_descending_cursor_inside_the_rate_envelope" => ExactActive, ES, "reverse_quantization_keeps_a_descending_cursor_inside_the_rate_envelope", CURRENT_ACTIVE;
    "crates/kithara-stretch/src/elastic/span_tests.rs", "small_phase_error_converges_without_a_source_jump_and_is_partition_independent" => ExactActive, ES, "small_phase_error_converges_without_a_source_jump_and_is_partition_independent", CURRENT_ACTIVE;
    "crates/kithara-stretch/src/elastic/tests.rs", "history_and_output_warmup_remove_the_initial_gap" => ExactActive, EL, "history_and_output_warmup_remove_the_initial_gap", CURRENT_ACTIVE;
    "crates/kithara-stretch/src/elastic/tests.rs", "keeps_the_same_latency_through_unity_and_rate_changes" => EquivalentActive, EL, "keeps_capabilities_stable_through_rate_changes", CURRENT_ACTIVE;
    "crates/kithara-stretch/src/elastic/tests.rs", "non_unity_warmup_aligns_the_first_audible_frame" => ExactActive, EL, "non_unity_warmup_aligns_the_first_audible_frame", CURRENT_ACTIVE;
    "crates/kithara-stretch/src/elastic/tests.rs", "prime_discards_previous_stream_state" => ExactActive, EL, "prime_discards_previous_stream_state", CURRENT_ACTIVE;
    "crates/kithara-stretch/src/elastic/tests.rs", "prime_rejects_every_ambiguous_buffer_count" => ExactActive, EL, "prime_rejects_every_ambiguous_buffer_count", CURRENT_ACTIVE;
    "crates/kithara-stretch/src/elastic/tests.rs", "primed_output_is_independent_of_unity_request_partitioning" => TransferActive, EL, "primed_output_is_independent_of_unity_request_partitioning", TRANSFER_ACTIVE;
    "crates/kithara-stretch/src/elastic/tests.rs", "preserves_tone_pitch_when_source_advance_changes" => ExactActive, EL, "preserves_tone_pitch_when_source_advance_changes", CURRENT_ACTIVE;
    "crates/kithara-stretch/src/elastic/tests.rs", "rejects_requests_outside_the_prepared_contract" => EquivalentActive, EL, "rejects_requests_outside_the_declared_rate_envelope", CURRENT_ACTIVE;
    "crates/kithara-stretch/src/elastic/tests.rs", "reset_clears_stream_history_without_changing_capabilities" => ExactActive, EL, "reset_clears_stream_history_without_changing_capabilities", CURRENT_ACTIVE;
    "crates/kithara-stretch/src/elastic/tests.rs", "reset_reprime_keeps_the_first_frame_aligned" => ExactActive, EL, "reset_reprime_keeps_the_first_frame_aligned", CURRENT_ACTIVE;
    "crates/kithara-stretch/src/elastic/tests.rs", "signalsmith_declares_reverse_input_support" => EquivalentActive, EL, "signalsmith_declares_its_rate_window_and_latency", CURRENT_ACTIVE;
    "crates/kithara-stretch/src/elastic/tests.rs", "unity_render_exposes_the_declared_source_and_output_latency" => EquivalentActive, EL, "signalsmith_unity_render_exposes_the_declared_latency", CURRENT_ACTIVE;
    "crates/kithara-stretch/src/elastic/tests.rs", "warmup_request_has_exact_latency_spans" => EquivalentActive, EL, "history_and_output_warmup_remove_the_initial_gap", CURRENT_ACTIVE;
    "crates/kithara-stretch/src/elastic/tests.rs", "renders_the_requested_output_frame_count" => ExactActive, EL, "renders_the_requested_output_frame_count", CURRENT_ACTIVE;
    "tests/tests/kithara_play/elastic_offline.rs", "bound_insert_reuses_an_already_opened_audio_reader" => TransferIgnoredRed, "tests/tests/kithara_play/analysis_decode_reuse.rs", "playback_and_analysis_share_one_opened_reader", TRANSFER_IGNORED_RED;
    "tests/tests/kithara_play/elastic_offline.rs", "bound_track_renders_elastic_audio_to_an_offline_wav" => EquivalentIgnoredRed, QRT, "bound_sync_render_is_rtsan_clean", CURRENT_IGNORED_RED;
    "tests/tests/kithara_play/elastic_offline.rs", "failed_session_seek_leaves_the_transport_unchanged" => EquivalentActive, ST, "late_transport_commit_is_rejected_without_changing_the_active_commit", CURRENT_ACTIVE;
    "tests/tests/kithara_play/elastic_offline.rs", "four_bound_players_write_one_tempo_revision" => EquivalentIgnoredRed, QBM, "synthetic_behavioral_matrix_uses_final_pcm_and_cochlea", CURRENT_IGNORED_RED;
    "tests/tests/kithara_play/elastic_offline.rs", "local_handover_rejects_tempo_before_session_mutation" => EquivalentActive, ST, "a_failing_block_rejects_the_pending_stamp_and_still_publishes", CURRENT_ACTIVE;
    "tests/tests/kithara_play/elastic_offline.rs", "omitted_peer_rejects_tempo_before_session_mutation" => EquivalentActive, SS, "unknown_routed_target_leaves_root_and_operation_counters_unchanged", CURRENT_ACTIVE;
    "tests/tests/kithara_play/elastic_offline.rs", "paused_unsupported_peer_rejects_tempo_before_session_mutation" => EquivalentIgnoredRed, QBM, "synthetic_behavioral_matrix_uses_final_pcm_and_cochlea", CURRENT_IGNORED_RED;
    "tests/tests/kithara_play/elastic_offline.rs", "queued_bound_successor_rejects_unsupported_tempo_before_commit" => EquivalentActive, AP, "identity_plan_rejects_alignment_transitions", CURRENT_ACTIVE;
    "tests/tests/kithara_play/elastic_offline.rs", "queued_bound_successor_retargets_after_tempo_commit_without_reload" => EquivalentIgnoredRed, QL, "latest_sync_target_wins_in_pcm", CURRENT_IGNORED_RED;
    "tests/tests/kithara_play/elastic_offline.rs", "reverse_binding_rejects_a_source_without_range_capability" => EquivalentActive, AP, "identity_plan_rejects_alignment_transitions", CURRENT_ACTIVE;
    "tests/tests/kithara_play/elastic_offline.rs", "reverse_binding_rejects_adaptive_hls_before_publication" => EquivalentActive, BM, "external_segment_snapshots_reject_incompatible_states", CURRENT_ACTIVE;
    "tests/tests/kithara_play/elastic_offline.rs", "reverse_bound_file_prepares_before_activation_and_renders_markers_to_wav" => EquivalentIgnoredRed, QRT, "bound_sync_render_is_rtsan_clean", CURRENT_IGNORED_RED;
    "tests/tests/kithara_play/elastic_offline.rs", "reverse_bound_hls_crosses_segment_boundaries_without_stale_replay" => EquivalentIgnoredRed, QM, "media_source_axis_runs_the_full_behavioral_row", CURRENT_IGNORED_RED;
    "tests/tests/kithara_play/elastic_offline.rs", "routed_join_is_silent_before_its_exact_session_beat" => EquivalentIgnoredRed, QBM, "synthetic_behavioral_matrix_uses_final_pcm_and_cochlea", CURRENT_IGNORED_RED;
    "tests/tests/kithara_play/elastic_offline.rs", "stale_session_seek_is_cancelled_before_a_retry" => EquivalentActive, AP, "cursor_from_another_operation_with_the_same_plan_revision_is_stale", CURRENT_ACTIVE;
    "tests/tests/kithara_play/elastic_offline.rs", "two_bound_players_commit_one_session_seek_revision" => EquivalentIgnoredRed, QBM, "synthetic_behavioral_matrix_uses_final_pcm_and_cochlea", CURRENT_IGNORED_RED;
    "tests/tests/kithara_play/elastic_offline.rs", "two_bound_players_commit_one_tempo_revision" => EquivalentIgnoredRed, QBM, "synthetic_behavioral_matrix_uses_final_pcm_and_cochlea", CURRENT_IGNORED_RED;
    "tests/tests/kithara_play/elastic_offline.rs", "unsupported_peer_rejects_tempo_before_session_mutation" => EquivalentActive, SO, "invalid_member_kind_preserves_group_member_and_policy", CURRENT_ACTIVE;
    "tests/tests/kithara_play/pitch_bend_transport.rs", "pitch_bend_pitches_up_transport_output" => EquivalentIgnoredRed, QL, "running_sync_command_changes_audible_pcm_within_one_block", CURRENT_IGNORED_RED;
    "tests/tests/kithara_play/player_processor_internal.rs", "load_track_preserves_the_musical_binding" => EquivalentActive, AP, "public_consumers_can_build_identity_plan_without_backend_capabilities", CURRENT_ACTIVE;
    "tests/tests/kithara_play/player_track_internal.rs", "active_track_owns_its_binding" => EquivalentActive, AP, "public_consumers_can_build_identity_plan_without_backend_capabilities", CURRENT_ACTIVE;
    "tests/tests/kithara_play/player_track_internal.rs", "host_rate_change_does_not_fall_back_to_an_unbound_axis" => EquivalentActive, BM, "asset_axis_is_independent_of_host_sample_rate", CURRENT_ACTIVE;
    "tests/tests/kithara_play/session_transport.rs", "checked_tempo_rejects_a_stale_physical_roster_context" => EquivalentActive, SS, "stale_topology_base_leaves_the_published_snapshot_unchanged", CURRENT_ACTIVE;
    "tests/tests/kithara_play/session_transport.rs", "failed_abort_delivery_is_retried_before_the_next_transport_commit" => EquivalentActive, ST, "a_failing_block_rejects_the_pending_stamp_and_still_publishes", CURRENT_ACTIVE;
    "tests/tests/kithara_play/session_transport.rs", "manual_session_transport_is_shared_and_render_driven" => EquivalentActive, IT, "session_transport_advances_with_rendered_frames", CURRENT_ACTIVE;
    "tests/tests/kithara_play/session_transport.rs", "scheduled_tempo_change_is_exact_and_offline_partition_independent" => EquivalentActive, ST, "tempo_commit_waits_for_the_matching_render_boundary", CURRENT_ACTIVE;
    "tests/tests/kithara_play/session_transport.rs", "tempo_change_preserves_beat_and_changes_slope_at_scheduled_boundary" => EquivalentActive, IT, "tempo_change_preserves_beat_and_changes_slope_at_the_scheduled_boundary", CURRENT_ACTIVE;
    "tests/tests/kithara_play/session_transport.rs", "tempo_rejects_non_finite_and_non_positive_values" => EquivalentActive, IT, "tempo_rejects_values_outside_the_representable_range", CURRENT_ACTIVE;
    "tests/tests/kithara_play/session_transport.rs", "tempo_revision_is_not_observed_before_render_commit" => EquivalentActive, IT, "tempo_revision_is_not_observed_before_the_render_commit", CURRENT_ACTIVE;
    "tests/tests/kithara_play/session_transport.rs", "setting_the_same_tempo_does_not_create_a_new_revision" => ExactActive, IT, "setting_the_same_tempo_does_not_create_a_new_revision", CURRENT_ACTIVE;
    "tests/tests/kithara_play/session_transport.rs", "transport_commit_is_published_to_every_registered_player_bus" => ExactActive, IT, "transport_commit_is_published_to_every_registered_player_bus", CURRENT_ACTIVE;
    "tests/tests/kithara_play/session_transport.rs", "transport_position_is_independent_of_render_partitioning" => ExactActive, IT, "transport_position_is_independent_of_render_partitioning", CURRENT_ACTIVE;
    "tests/tests/kithara_play/source_range.rs", "canonical_source_range_matches_the_unity_linear_path" => EquivalentActive, AP, "public_consumers_can_build_identity_plan_without_backend_capabilities", CURRENT_ACTIVE;
    "tests/tests/kithara_play/track_binding.rs", "binding_keeps_signed_phase_before_non_zero_anchor" => EquivalentActive, BM, "asset_axis_is_independent_of_host_sample_rate", CURRENT_ACTIVE;
    "tests/tests/kithara_play/track_binding.rs", "first_downbeat_defines_zero_and_preserves_pickup_beats" => EquivalentActive, BM, "pickup_ordinals_keep_canonical_downbeat_zero", CURRENT_ACTIVE;
    "tests/tests/kithara_play/track_binding.rs", "marker_past_decoded_source_end_is_rejected" => EquivalentActive, BM, "analyzer_snapshot_requires_marker_span_evidence", CURRENT_ACTIVE;
    "tests/tests/kithara_play/track_binding.rs", "missing_invalid_and_tempo_only_analysis_are_sync_unavailable" => EquivalentActive, BM, "tempo_only_geometry_does_not_fabricate_meter", CURRENT_ACTIVE;
    "tests/tests/kithara_play/track_binding.rs", "track_beat_map_converts_source_rate_to_host_rate" => EquivalentActive, BM, "asset_axis_is_independent_of_host_sample_rate", CURRENT_ACTIVE;
    "tests/tests/kithara_play/track_binding.rs", "track_beat_map_does_not_extrapolate_outside_markers" => EquivalentActive, BM, "building_gap_is_uncovered_but_complete_extent_is_outside_domain", CURRENT_ACTIVE;
    "tests/tests/kithara_play/track_binding.rs", "track_beat_map_interpolates_non_uniform_markers_and_inverts" => EquivalentActive, BM, "sparse_snapshot_is_honest_and_invertible", CURRENT_ACTIVE;
    "tests/tests/kithara_play/track_binding.rs", "track_beat_map_preserves_non_zero_source_anchor_and_meter" => EquivalentActive, BM, "scalar_tempo_and_segments_share_declared_topology", CURRENT_ACTIVE;
    "tests/tests/kithara_play/track_binding.rs", "track_beat_map_rejects_host_rate_source_extent_overflow" => EquivalentActive, BM, "large_asset_extent_uses_exact_integer_boundary_semantics", CURRENT_ACTIVE;
    "tests/tests/kithara_play/track_binding.rs", "track_beat_map_source_extent_uses_decoder_rounding" => EquivalentActive, BM, "progressive_extrapolation_is_immediately_queryable", CURRENT_ACTIVE;
    "tests/tests/kithara_play/transport_artifact.rs", "multi_track_transport_writes_deterministic_tempo_reverse_wav" => EquivalentIgnoredRed, QRT, "bound_sync_render_is_rtsan_clean", CURRENT_IGNORED_RED;
    "tests/tests/kithara_play/wasm_transport.rs", "browser_player_uses_shared_transport_contract" => TransferIgnoredRed, "tests/tests/kithara_play/wasm_sync.rs", "browser_player_uses_shared_sync_contract", TRANSFER_IGNORED_RED;
};

/// Returns the complete frozen PR #118 provenance ledger.
#[must_use]
pub fn pr_118_provenance() -> Vec<Pr118Provenance> {
    PR_118_ROWS.to_vec()
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
            "progressive_extrapolation_is_immediately_queryable",
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

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};

    use super::*;
    use crate::kithara;

    const PR_118_FROZEN_FINGERPRINT: u64 = 0x374e_2535_68bf_c490;
    const PR_118_PATH_COUNTS: &[(&str, usize)] = &[
        ("crates/kithara-audio/src/analysis/analyzer/set.rs", 2),
        ("crates/kithara-audio/src/audio/core.rs", 6),
        ("crates/kithara-audio/src/audio/cursor.rs", 6),
        ("crates/kithara-audio/src/audio/ring.rs", 1),
        ("crates/kithara-audio/src/effects/transport.rs", 2),
        ("crates/kithara-audio/src/elastic/reader/preparation.rs", 6),
        ("crates/kithara-audio/src/elastic/reader/rendering.rs", 3),
        ("crates/kithara-audio/src/renderer/node.rs", 1),
        ("crates/kithara-audio/src/source_range.rs", 1),
        ("crates/kithara-events/src/event.rs", 2),
        ("crates/kithara-ffi/src/core/convert.rs", 3),
        ("crates/kithara-platform/src/common/gate.rs", 2),
        ("crates/kithara-play/src/bridge/playback.rs", 1),
        ("crates/kithara-play/src/engine/core.rs", 2),
        ("crates/kithara-play/src/player/core.rs", 1),
        ("crates/kithara-play/src/player/flow/binding.rs", 1),
        ("crates/kithara-play/src/player/flow/notify.rs", 1),
        ("crates/kithara-play/src/player/flow/transport.rs", 1),
        ("crates/kithara-play/src/player/multi/member.rs", 1),
        ("crates/kithara-play/src/player/multi/session.rs", 2),
        ("crates/kithara-play/src/player/multi/tests.rs", 13),
        ("crates/kithara-play/src/player/node/processor.rs", 4),
        ("crates/kithara-play/src/player/platform/native.rs", 5),
        ("crates/kithara-play/src/player/state/items.rs", 3),
        ("crates/kithara-play/src/player/track/bound_plan.rs", 11),
        ("crates/kithara-play/src/player/track/core.rs", 1),
        ("crates/kithara-play/src/resource/reader.rs", 1),
        ("crates/kithara-play/src/session/dispatch.rs", 2),
        ("crates/kithara-play/src/session/protocol.rs", 1),
        ("crates/kithara-play/src/session/render/tests.rs", 14),
        ("crates/kithara-play/src/session/transport.rs", 5),
        ("crates/kithara-queue/src/queue/component.rs", 1),
        ("crates/kithara-queue/src/queue/state.rs", 1),
        ("crates/kithara-stream/src/seek_state.rs", 1),
        ("crates/kithara-stretch/src/elastic/config.rs", 2),
        ("crates/kithara-stretch/src/elastic/rate.rs", 1),
        ("crates/kithara-stretch/src/elastic/span_tests.rs", 11),
        ("crates/kithara-stretch/src/elastic/tests.rs", 14),
        ("tests/tests/kithara_play/elastic_offline.rs", 18),
        ("tests/tests/kithara_play/pitch_bend_transport.rs", 1),
        ("tests/tests/kithara_play/player_processor_internal.rs", 1),
        ("tests/tests/kithara_play/player_track_internal.rs", 2),
        ("tests/tests/kithara_play/session_transport.rs", 10),
        ("tests/tests/kithara_play/source_range.rs", 1),
        ("tests/tests/kithara_play/track_binding.rs", 10),
        ("tests/tests/kithara_play/transport_artifact.rs", 1),
        ("tests/tests/kithara_play/wasm_transport.rs", 1),
    ];

    #[kithara::test]
    fn pr_118_provenance_is_exact_complete_and_unique() {
        let rows = pr_118_provenance();
        assert_eq!(rows.len(), PR_118_TEST_COUNT);
        assert!(rows.iter().all(Pr118Provenance::has_complete_provenance));

        let keys = rows
            .iter()
            .map(|row| (row.source_path(), row.source_test()))
            .collect::<BTreeSet<_>>();
        assert_eq!(keys.len(), rows.len(), "every frozen source test is unique");
        assert_eq!(
            frozen_fingerprint(&keys),
            PR_118_FROZEN_FINGERPRINT,
            "the frozen PR #118 source set changed"
        );

        let path_counts = rows.iter().fold(BTreeMap::new(), |mut counts, row| {
            *counts.entry(row.source_path()).or_insert(0) += 1;
            counts
        });
        let expected_path_counts = PR_118_PATH_COUNTS
            .iter()
            .copied()
            .collect::<BTreeMap<_, _>>();
        assert_eq!(path_counts, expected_path_counts);

        let dispositions = rows.iter().fold(BTreeMap::new(), |mut counts, row| {
            *counts.entry(row.disposition()).or_insert(0) += 1;
            counts
        });
        assert_eq!(
            dispositions,
            BTreeMap::from([
                (Pr118Disposition::ExactActive, 31),
                (Pr118Disposition::EquivalentActive, 94),
                (Pr118Disposition::EquivalentIgnoredRed, 16),
                (Pr118Disposition::TransferActive, 2),
                (Pr118Disposition::TransferIgnoredRed, 16),
                (Pr118Disposition::NonTransplant, 22),
            ])
        );
    }

    #[kithara::test]
    fn pr_118_transfer_rows_do_not_claim_executable_oracle_coverage() {
        let transfers = pr_118_provenance()
            .into_iter()
            .filter(|row| {
                matches!(
                    row.disposition(),
                    Pr118Disposition::TransferActive | Pr118Disposition::TransferIgnoredRed
                )
            })
            .collect::<Vec<_>>();

        assert_eq!(transfers.len(), 18);
        assert!(
            transfers
                .iter()
                .all(|row| row.note().contains("ledger is not the executable test"))
        );
    }

    #[kithara::test]
    fn current_map_provenance_names_the_real_progressive_test() {
        let row = registrations()
            .into_iter()
            .find(|row| {
                matches!(
                    row.source(),
                    OracleSource::Frozen { pr: Some(118), test, .. }
                        if test == "track_beat_map_source_extent_uses_decoder_rounding"
                )
            })
            .expect("the PR #118 progressive-map provenance row is registered");

        assert_eq!(
            row.destination_test,
            "progressive_extrapolation_is_immediately_queryable"
        );
    }

    fn frozen_fingerprint(keys: &BTreeSet<(&str, &str)>) -> u64 {
        const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
        const PRIME: u64 = 0x0000_0100_0000_01b3;

        keys.iter().fold(OFFSET, |mut hash, (path, test)| {
            for byte in path.bytes().chain([0]).chain(test.bytes()).chain([u8::MAX]) {
                hash ^= u64::from(byte);
                hash = hash.wrapping_mul(PRIME);
            }
            hash
        })
    }
}
