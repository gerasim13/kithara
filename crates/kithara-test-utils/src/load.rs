//! CPU contention started next to a test, so repeating one test reproduces the
//! scheduling pressure a full parallel run puts on it.

use std::{
    env, hint,
    sync::mpsc::{self, Sender, TryRecvError},
    thread::{self, JoinHandle},
};

/// Spinning threads per available CPU; unset or `0` starts none.
const ENV_LOAD: &str = "KITHARA_TEST_LOAD";

/// Keeps every CPU busy while the test runs. Dropping it stops the spinners.
pub struct LoadGuard {
    stops: Vec<Sender<()>>,
    spinners: Vec<JoinHandle<()>>,
}

impl LoadGuard {
    /// Start `KITHARA_TEST_LOAD` spinners per available CPU.
    ///
    /// # Panics
    ///
    /// When the variable is set to something other than a count: a stress run
    /// that silently ran without its load would report a clean result for a
    /// condition it never created.
    #[must_use]
    pub fn start() -> Option<Self> {
        let value = env::var(ENV_LOAD).ok()?;
        let per_cpu: usize = match value.parse() {
            Ok(count) => count,
            Err(error) => panic!("{ENV_LOAD}={value} is not a count of spinners per CPU: {error}"),
        };
        if per_cpu == 0 {
            return None;
        }
        let count = per_cpu * thread::available_parallelism().map_or(1, usize::from);
        let (stops, spinners) = (0..count)
            .map(|_| {
                let (stop, stopped) = mpsc::channel::<()>();
                let spinner = thread::Builder::new()
                    .name("kithara-test-load".into())
                    .spawn(move || {
                        while matches!(stopped.try_recv(), Err(TryRecvError::Empty)) {
                            hint::spin_loop();
                        }
                    })
                    .unwrap_or_else(|error| panic!("spawn a {ENV_LOAD} spinner: {error}"));
                (stop, spinner)
            })
            .unzip();
        tracing::info!(spinners = count, "{ENV_LOAD}: CPU load started");
        Some(Self { stops, spinners })
    }
}

impl Drop for LoadGuard {
    fn drop(&mut self) {
        self.stops.clear();
        for spinner in self.spinners.drain(..) {
            let _ = spinner.join();
        }
    }
}
