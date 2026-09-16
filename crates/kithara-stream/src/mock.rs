use kithara_test_utils::mock::CallCounter;

use crate::WorkerWake;
pub use crate::source::SourceMock;

#[derive(Default)]
pub struct NoopWorkerWake;

impl WorkerWake for NoopWorkerWake {
    fn defer(&self) {}

    fn wake(&self) {}
}

pub struct CountingWorkerWake(CallCounter);

impl CountingWorkerWake {
    #[must_use]
    pub fn new(counter: CallCounter) -> Self {
        Self(counter)
    }
}

impl WorkerWake for CountingWorkerWake {
    delegate::delegate! {
        to self.0 {
            #[call(record)]
            fn defer(&self);
            #[call(record)]
            fn wake(&self);
        }
    }
}
