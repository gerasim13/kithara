/// Playback adapter for the base worker wake capability.
#[derive(Clone)]
pub(crate) struct Wake(kithara_worker::Wake);

impl Wake {
    pub(crate) const fn new(wake: kithara_worker::Wake) -> Self {
        Self(wake)
    }
}

impl kithara_stream::WorkerWake for Wake {
    delegate::delegate! {
        to self.0 {
            fn wake(&self);
            fn defer(&self);
        }
    }
}

impl std::task::Wake for Wake {
    fn wake(self: std::sync::Arc<Self>) {
        kithara_stream::WorkerWake::wake(&*self);
    }

    fn wake_by_ref(self: &std::sync::Arc<Self>) {
        kithara_stream::WorkerWake::wake(&**self);
    }
}
