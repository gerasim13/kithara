use kithara_platform::time::Duration;
use serde::Serialize;

use super::{ServiceClass, SlotId, TickResult};

/// Best result from a single round-robin pass over all nodes.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum PassOutcome {
    Produced,
    Waiting,
    UpstreamPending,
    Backpressured,
    Idle,
}

/// Allocation-free summary of one scheduler pass.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct PassReport {
    pub(super) first_backpressured_slot: Option<SlotId>,
    pub(super) first_progress_slot: Option<SlotId>,
    pub(super) first_upstream_pending_slot: Option<SlotId>,
    pub(super) first_waiting_service_class: Option<ServiceClass>,
    pub(super) first_waiting_slot: Option<SlotId>,
    pub(super) outcome: PassOutcome,
    pub(super) active_slots: usize,
    pub(super) backpressured_slots: usize,
    pub(super) done_slots: usize,
    pub(super) progress_slots: usize,
    pub(super) upstream_pending_slots: usize,
    pub(super) waiting_slots: usize,
}

impl PassReport {
    pub(super) const fn new(active_slots: usize) -> Self {
        Self {
            first_backpressured_slot: None,
            first_progress_slot: None,
            first_upstream_pending_slot: None,
            first_waiting_service_class: None,
            first_waiting_slot: None,
            outcome: PassOutcome::Idle,
            active_slots,
            backpressured_slots: 0,
            done_slots: 0,
            progress_slots: 0,
            upstream_pending_slots: 0,
            waiting_slots: 0,
        }
    }

    pub(super) fn record(&mut self, slot: SlotId, service_class: ServiceClass, result: TickResult) {
        match result {
            TickResult::Progress => {
                self.progress_slots += 1;
                self.first_progress_slot.get_or_insert(slot);
            }
            TickResult::Waiting => {
                self.waiting_slots += 1;
                self.first_waiting_slot.get_or_insert(slot);
                self.first_waiting_service_class
                    .get_or_insert(service_class);
            }
            TickResult::UpstreamPending => {
                self.upstream_pending_slots += 1;
                self.first_upstream_pending_slot.get_or_insert(slot);
            }
            TickResult::Backpressured => {
                self.backpressured_slots += 1;
                self.first_backpressured_slot.get_or_insert(slot);
            }
            TickResult::Done => self.done_slots += 1,
        }
    }
}

pub(crate) enum SchedulerEvent {
    PassStart,
    PassEnd,
    Progress(PassReport),
    Idle(PassReport),
    Waiting(PassReport),
    UpstreamPending(PassReport),
    Backpressured(PassReport),
    SlowTick { slot: SlotId, elapsed: Duration },
}

pub(crate) trait SchedulerObserver: Send + 'static {
    fn on_event(&mut self, event: SchedulerEvent);
}

pub(crate) struct PlaybackObserver;

impl PlaybackObserver {
    pub(super) const fn new() -> Self {
        Self
    }
}

impl SchedulerObserver for PlaybackObserver {
    fn on_event(&mut self, event: SchedulerEvent) {
        match event {
            SchedulerEvent::SlowTick { slot, elapsed } => {
                tracing::debug!(
                    track_id = slot,
                    elapsed_ms = elapsed.as_millis(),
                    "step_track took too long — starving other tracks"
                );
            }
            SchedulerEvent::Progress(report)
            | SchedulerEvent::Idle(report)
            | SchedulerEvent::Waiting(report)
            | SchedulerEvent::UpstreamPending(report)
            | SchedulerEvent::Backpressured(report) => trace_report(report),
            SchedulerEvent::PassStart | SchedulerEvent::PassEnd => {}
        }
    }
}

fn trace_report(report: PassReport) {
    tracing::trace!(
        ?report.outcome,
        active_slots = report.active_slots,
        progress_slots = report.progress_slots,
        waiting_slots = report.waiting_slots,
        upstream_pending_slots = report.upstream_pending_slots,
        backpressured_slots = report.backpressured_slots,
        done_slots = report.done_slots,
        "playback scheduler pass"
    );
}
