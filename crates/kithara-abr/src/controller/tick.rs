use std::sync::atomic::Ordering;

use kithara_platform::{sync::Arc, time::Instant};
use kithara_test_utils::kithara;
use tracing::debug;

use super::{
    core::{AbrController, AbrPeerId},
    peer::PeerEntry,
};
use crate::{
    AbrEvent, AbrReason, VariantIndex,
    state::{AbrDecision, AbrView},
};

impl AbrController {
    #[kithara::probe(peer_id)]
    pub(crate) fn run_tick(self: &Arc<Self>, peer_id: AbrPeerId, now: Instant) {
        let Some(ctx) = TickContext::resolve(self, peer_id) else {
            return;
        };

        let bus = ctx.entry.bus();
        let variants = ctx.peer.variants();
        let progress = ctx.peer.progress();
        let buffer_ahead = progress.map(|p| {
            p.download_head_playback_time
                .saturating_sub(p.reader_playback_time)
        });
        let bytes_downloaded = ctx.entry.bytes_downloaded.load(Ordering::Acquire);
        let estimate_bps = self.estimator.estimate_bps();

        if !ctx
            .entry
            .variants_registered_published
            .load(Ordering::Acquire)
            && let Some(ref bus) = bus
        {
            let initial = ctx
                .entry
                .state
                .as_ref()
                .map_or(VariantIndex::new(0), |s| s.current_variant_index());
            bus.publish(AbrEvent::VariantsRegistered {
                initial,
                variants: variants.clone(),
            });
            ctx.entry
                .variants_registered_published
                .store(true, Ordering::Release);
        }

        let view = AbrView {
            buffer_ahead,
            estimate_bps,
            bytes_downloaded,
            settings: &self.settings,
            variants: &variants,
        };
        self.emit_throttled(&ctx.entry, &bus, now, &view);

        let Some(state) = ctx.entry.state.as_ref() else {
            return;
        };
        let decision = state.decide(&view, now);

        match decision {
            AbrDecision::Stay {
                reason: AbrReason::AlreadyOptimal,
                current,
            } => {
                ctx.entry.clear_tick_deadline();
                state.retract_throughput_pending(current);
            }
            AbrDecision::Stay {
                reason: AbrReason::MinInterval,
                ..
            } => {
                let deadline =
                    now + state.switch_interval_remaining(now, self.settings.min_switch_interval);
                ctx.entry.set_tick_deadline(deadline);
                if let Some(ref bus) = bus {
                    bus.publish(AbrEvent::DecisionSkipped {
                        reason: AbrReason::MinInterval,
                    });
                }
            }
            AbrDecision::Stay { reason, .. } => {
                ctx.entry.clear_tick_deadline();
                if let Some(ref bus) = bus {
                    bus.publish(AbrEvent::DecisionSkipped { reason });
                }
            }
            change => {
                ctx.entry.clear_tick_deadline();
                state.request_target(change.target(), change.reason());
                ctx.peer.wake();
            }
        }

        debug!(
            ?peer_id,
            reason = ?decision.reason(),
            did_change = decision.changed(),
            target = decision.target().get(),
            estimate_bps,
            mode = ?state.mode(),
            pending_target_after = ?state.pending_target(),
            is_locked = state.is_locked(),
            "ABR: tick"
        );
    }
}

/// Live peer and registration resolved for one controller tick.
struct TickContext {
    entry: Arc<PeerEntry>,
    peer: Arc<dyn crate::abr::Abr>,
}

impl TickContext {
    fn resolve(controller: &AbrController, peer_id: AbrPeerId) -> Option<Self> {
        let entry = controller.peer_entry(peer_id)?;
        let peer = entry.peer_weak.upgrade()?;
        Some(Self { entry, peer })
    }
}
