use kithara_platform::time::Duration;

#[derive(Clone, Copy, Debug)]
pub(crate) struct ResumeState {
    pub(crate) target: Duration,
    pub(crate) trim_head: bool,
}
