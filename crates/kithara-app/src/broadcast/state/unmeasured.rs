use kithara::platform::{CancelToken, time::Duration};

use super::{BroadcastResult, Packager, fixture::Stream};
use crate::pools::AppHost;

pub(super) struct Unmeasured;

impl Packager for Unmeasured {
    type Live = Stream;

    const IS_AVAILABLE: bool = true;

    fn is_live(_live: &Stream) -> bool {
        true
    }

    fn start(
        _host: &AppHost,
        _shutdown: &CancelToken,
        _tap_lead: Duration,
    ) -> BroadcastResult<Option<Stream>> {
        Ok(None)
    }

    fn release(_host: &AppHost) -> BroadcastResult<()> {
        Ok(())
    }

    fn stop(_live: Stream) {}

    fn url(live: &Stream) -> &str {
        &live.0
    }
}
