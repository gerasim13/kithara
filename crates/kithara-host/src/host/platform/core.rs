use kithara_play::PlayError;

pub(in crate::host) trait PlatformResult<T> {
    fn resolve(self) -> Result<T, PlayError>;
}

impl<T> PlatformResult<T> for Result<T, PlayError> {
    fn resolve(self) -> Self {
        self
    }
}
