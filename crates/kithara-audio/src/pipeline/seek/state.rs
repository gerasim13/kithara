use kithara_decode::DecodeResult;
use kithara_platform::time::Duration;
use kithara_signal::AudioSpec;

#[derive(Clone, Copy, Debug)]
pub(crate) enum ResumeTarget {
    Position(Duration),
    Source(crate::SourceEnd),
}

impl ResumeTarget {
    pub(crate) fn position(self) -> DecodeResult<Duration> {
        match self {
            Self::Position(position) => Ok(position),
            Self::Source(end) => {
                Ok(AudioSpec::new(1, end.sample_rate()).duration_for(end.frame())?)
            }
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct ResumeState {
    pub(crate) target: ResumeTarget,
    pub(crate) trim_head: bool,
}
