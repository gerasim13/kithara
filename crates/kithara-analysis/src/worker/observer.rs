use kithara_platform::time::Duration;
use kithara_test_utils::hang::{HangDetector, default_timeout};
use serde::Serialize;
use tracing::{debug, warn};

use super::AnalysisStep;

#[derive(Clone, Copy, Default, Serialize)]
struct AnalysisHangContext {
    last_step: Option<AnalysisStep>,
    waiting_streak: u32,
}

pub(crate) struct AnalysisObserver {
    context: AnalysisHangContext,
    detector: HangDetector<AnalysisHangContext>,
}

impl AnalysisObserver {
    const HEAVY_TICK_BUDGET: Duration = Duration::from_secs(120);

    pub(crate) fn observe(&mut self, step: AnalysisStep) {
        self.context.last_step = Some(step);
        if step == AnalysisStep::Waiting {
            self.context.waiting_streak = self.context.waiting_streak.saturating_add(1);
            let context = self.context;
            self.detector.tick_with(|| context);
        } else {
            self.context.waiting_streak = 0;
            let context = self.context;
            self.detector.reset_with(|| context);
        }
    }

    pub(crate) fn observe_slow_tick(elapsed: Duration) {
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

impl Default for AnalysisObserver {
    fn default() -> Self {
        Self {
            context: AnalysisHangContext::default(),
            detector: HangDetector::new("analysis_worker_loop", default_timeout()),
        }
    }
}
