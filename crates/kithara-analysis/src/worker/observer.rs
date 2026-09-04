use kithara_platform::time::Duration;
use kithara_test_utils::hang::{HangDetector, default_timeout};
use kithara_worker::{Event, Observer, TickResult};
use serde::Serialize;
use tracing::{debug, warn};

#[derive(Clone, Copy, Default, Serialize)]
struct AnalysisHangContext {
    last_step: Option<&'static str>,
    waiting_streak: u32,
}

pub(crate) struct AnalysisObserver {
    context: AnalysisHangContext,
    detector: HangDetector<AnalysisHangContext>,
}

impl AnalysisObserver {
    const HEAVY_TICK_BUDGET: Duration = Duration::from_secs(120);

    fn observe(&mut self, step: TickResult) {
        self.context.last_step = Some(step_name(step));
        if step == TickResult::Waiting {
            self.context.waiting_streak = self.context.waiting_streak.saturating_add(1);
            let context = self.context;
            self.detector.tick_with(|| context);
        } else {
            self.context.waiting_streak = 0;
            let context = self.context;
            self.detector.reset_with(|| context);
        }
    }

    fn observe_slow_tick(elapsed: Duration) {
        if elapsed > Self::HEAVY_TICK_BUDGET {
            warn!(
                elapsed_ms = elapsed.as_millis(),
                budget_ms = Self::HEAVY_TICK_BUDGET.as_millis(),
                "analysis heavy tick exceeded hang budget"
            );
        } else {
            debug!(
                elapsed_ms = elapsed.as_millis(),
                budget_ms = Self::HEAVY_TICK_BUDGET.as_millis(),
                "analysis heavy tick completed within its budget"
            );
        }
    }
}

impl Observer for AnalysisObserver {
    fn on_event(&mut self, event: Event) {
        match event {
            Event::Progress(_) => self.observe(TickResult::Progress),
            Event::Waiting(_) => self.observe(TickResult::Waiting),
            Event::UpstreamPending(_) => self.observe(TickResult::UpstreamPending),
            Event::Backpressured(_) => self.observe(TickResult::Backpressured),
            Event::Idle(_) => self.observe(TickResult::Done),
            Event::SlowTick { elapsed, .. } => Self::observe_slow_tick(elapsed),
            Event::TaskPanicked { task } => {
                warn!(task_id = task.get(), "analysis worker node panicked");
            }
            _ => {}
        }
    }
}

fn step_name(step: TickResult) -> &'static str {
    match step {
        TickResult::Progress => "progress",
        TickResult::Waiting => "waiting",
        TickResult::Backpressured => "backpressured",
        TickResult::UpstreamPending => "upstream_pending",
        TickResult::Done => "done",
        _ => "unknown",
    }
}

impl Default for AnalysisObserver {
    fn default() -> Self {
        Self {
            context: AnalysisHangContext::default(),
            detector: HangDetector::new("analysis_worker_loop", default_timeout()),
        }
    }
}
