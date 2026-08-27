mod observer;
mod outcome;
mod reader;
mod source;

pub use kithara_decode::{DecodeError, DecodeResult};
#[cfg(any(test, feature = "mock"))]
pub use observer::PcmObserverMock;
pub use observer::{PcmObserveError, PcmObserver};
pub use outcome::{ChunkOutcome, PendingReason, ReadOutcome, SeekOutcome};
pub use reader::{PcmControl, PcmRead, PcmReader, PcmSession, SeekBegin};
#[cfg(any(test, feature = "mock"))]
pub use reader::{PcmControlMock, PcmReadMock, PcmSessionMock};
#[cfg(test)]
pub(crate) use source::PcmSourceExt;
#[cfg(any(test, feature = "mock"))]
pub use source::PcmSourceMock;
pub use source::{PcmSource, SourceDiscontinuity};
