use kithara_abr::{AbrDecision, AbrSettings, AbrState, VariantDuration, VariantIndex, VariantInfo};
use kithara_platform::{
    sync::Arc,
    time::{Duration, Instant},
};

pub(super) fn fast_settings() -> AbrSettings {
    AbrSettings::builder()
        .initial_throughput_bps(Some(2_000_000))
        .min_switch_interval(Duration::ZERO)
        .min_buffer_for_up_switch(Duration::ZERO)
        .build()
}

pub(super) fn variants(bitrates: &[u64]) -> Vec<VariantInfo> {
    bitrates
        .iter()
        .enumerate()
        .map(|(index, bitrate)| VariantInfo {
            variant_index: VariantIndex::new(index),
            bandwidth_bps: Some(*bitrate),
            duration: VariantDuration::Unknown,
            name: None,
            codecs: None,
            container: None,
        })
        .collect()
}

pub(super) fn publish(state: &Arc<AbrState>, decision: AbrDecision, now: Instant) {
    if decision.changed() {
        state.request_target(decision.target(), decision.reason());
    }
    if let Some(claim) = state.claim_pending_decision(state.current_variant_index()) {
        assert!(state.commit_pending(claim, now));
    }
}
