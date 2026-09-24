pub fn model<F>(f: F)
where
    F: Fn() + Send + Sync + 'static,
{
    crate::loom::model(f);
}
