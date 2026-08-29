use kithara::host::Host;
use kithara_platform::{CancelToken, time::Duration};

use super::{BroadcastResult, Packager, fixture::Stream};

pub(super) struct Unmeasured;

impl Packager for Unmeasured {
    type Live = Stream;

    const IS_AVAILABLE: bool = true;

    fn is_live(_live: &Stream) -> bool {
        true
    }

    fn start(
        _host: &Host,
        _shutdown: &CancelToken,
        _tap_lead: Duration,
    ) -> BroadcastResult<Option<Stream>> {
        Ok(None)
    }

    fn release(_host: &Host) -> BroadcastResult<()> {
        Ok(())
    }

    fn stop(_live: Stream) {}

    fn url(live: &Stream) -> &str {
        &live.0
    }
}
