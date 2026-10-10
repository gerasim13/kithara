use kithara_platform::{
    CancelToken,
    maybe_send::MaybeSend,
    tokio::{
        self,
        runtime::Handle,
        sync::mpsc::{self, UnboundedReceiver},
        task,
    },
};

/// Work on a runtime that ends when its handle drops.
pub(crate) struct Job<T> {
    cancel: CancelToken,
    output: UnboundedReceiver<T>,
}

impl<T: MaybeSend + 'static> Job<T> {
    /// Runs `work` until it finishes or `parent` is cancelled.
    pub(crate) fn spawn<F>(runtime: &Handle, parent: &CancelToken, work: F) -> Self
    where
        F: Future<Output = T> + MaybeSend + 'static,
    {
        let cancel = parent.child();
        let (sender, output) = mpsc::unbounded_channel();
        let cancelled = cancel.clone();
        drop(task::spawn_on(runtime, async move {
            tokio::select! {
                biased;
                () = cancelled.cancelled() => {}
                output = work => {
                    let _ = sender.send(output);
                }
            }
        }));
        Self { cancel, output }
    }
}

impl<T> Job<T> {
    /// The output once the work has finished; a cancelled job has none.
    pub(crate) fn finished(&mut self) -> Option<T> {
        if self.cancel.is_cancelled() {
            return None;
        }
        self.output.try_recv().ok()
    }
}

impl<T> Drop for Job<T> {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}
