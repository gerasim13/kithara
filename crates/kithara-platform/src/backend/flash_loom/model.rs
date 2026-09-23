struct Reset;

impl Drop for Reset {
    fn drop(&mut self) {
        crate::flash::reset();
    }
}

pub fn model<F>(f: F)
where
    F: Fn() + Send + Sync + 'static,
{
    crate::loom::model(move || {
        crate::flash::reset();
        let _reset = Reset;
        f();
    });
}
