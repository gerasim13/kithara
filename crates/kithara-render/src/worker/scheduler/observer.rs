use kithara_test_utils::hang::{HangDetector, default_timeout};
use kithara_worker::{Event, Observer, PassOutcome, PassReport, TaskId};
use serde::Serialize;

#[derive(Clone, Copy, Default, Serialize)]
struct PlaybackHangContext {
    first_waiting_task: Option<u64>,
    active_tasks: usize,
    waiting_tasks: usize,
}

impl From<PassReport> for PlaybackHangContext {
    fn from(report: PassReport) -> Self {
        Self {
            active_tasks: report.active_tasks,
            first_waiting_task: report.first_waiting_task.map(TaskId::get),
            waiting_tasks: report.waiting_tasks,
        }
    }
}

pub(crate) struct PlaybackObserver {
    detector: HangDetector<PlaybackHangContext>,
    pass_start: Option<std::time::Instant>,
    pass_end: Option<std::time::Instant>,
    last_outcome: Option<PassOutcome>,
}

impl PlaybackObserver {
    fn observe(&mut self, report: PassReport) {
        self.last_outcome = Some(report.outcome);
        let context = PlaybackHangContext::from(report);
        if report.outcome == PassOutcome::Waiting {
            self.detector.tick_with(|| context);
        } else {
            self.detector.reset_with(|| context);
        }
        tracing::trace!(
            ?report.outcome,
            active_tasks = report.active_tasks,
            progress_tasks = report.progress_tasks,
            waiting_tasks = report.waiting_tasks,
            upstream_pending_tasks = report.upstream_pending_tasks,
            backpressured_tasks = report.backpressured_tasks,
            done_tasks = report.done_tasks,
            "playback scheduler pass"
        );
    }
}

impl Default for PlaybackObserver {
    fn default() -> Self {
        Self {
            detector: HangDetector::new("audio_worker_loop", default_timeout()),
            pass_start: None,
            pass_end: None,
            last_outcome: None,
        }
    }
}

impl Observer for PlaybackObserver {
    fn on_event(&mut self, event: Event) {
        match event {
            Event::PassStart => {
                let now = std::time::Instant::now();
                if let Some(end) = self.pass_end
                    && now.duration_since(end).as_micros() > 15_000
                {
                    tracing::debug!(
                        thread = ?std::thread::current().id(),
                        parked_us = now.duration_since(end).as_micros(),
                        outcome = ?self.last_outcome,
                        "diag: worker parked long"
                    );
                }
                self.pass_start = Some(now);
            }
            Event::PassEnd => {
                let now = std::time::Instant::now();
                if let Some(start) = self.pass_start
                    && now.duration_since(start).as_micros() > 5_000
                {
                    tracing::debug!(
                        thread = ?std::thread::current().id(),
                        pass_us = now.duration_since(start).as_micros(),
                        "diag: worker pass long"
                    );
                }
                self.pass_end = Some(now);
            }
            Event::SlowTick { task, elapsed } => {
                tracing::debug!(
                    track_id = task.get(),
                    elapsed_ms = elapsed.as_millis(),
                    "step_track took too long - starving other tracks"
                );
            }
            Event::TaskPanicked { task } => {
                tracing::warn!(track_id = task.get(), "playback task panicked");
            }
            Event::ThreadClassRefused { class, error } => {
                tracing::warn!(
                    ?class,
                    ?error,
                    "the OS refused the playback worker's thread class; \
                     a busy machine can starve the decks' rings"
                );
            }
            Event::Progress(report)
            | Event::Idle(report)
            | Event::Waiting(report)
            | Event::UpstreamPending(report)
            | Event::Backpressured(report) => self.observe(report),
            _ => {}
        }
    }
}
