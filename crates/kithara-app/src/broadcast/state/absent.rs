use kithara::host::Host;
use kithara_platform::{CancelToken, time::Duration};

use super::{BroadcastResult, Packager, fixture::Stream};

pub(super) struct Absent;

impl Packager for Absent {
    type Live = Stream;

    const IS_AVAILABLE: bool = false;

    fn is_live(_live: &Stream) -> bool {
        true
    }

    fn release(_host: &Host) -> BroadcastResult<()> {
        Ok(())
    }

    fn start(
        _host: &Host,
        _shutdown: &CancelToken,
        _tap_lead: Duration,
    ) -> BroadcastResult<Option<Stream>> {
        Err("no packager in this build".into())
    }

    fn stop(_live: Stream) {}

    fn url(live: &Stream) -> &str {
        &live.0
    }
}
