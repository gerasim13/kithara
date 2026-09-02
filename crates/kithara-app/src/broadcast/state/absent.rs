use kithara::{platform::CancelToken, worker::Worker};

use super::{BroadcastResult, Packager, fixture::Stream};
use crate::pools::{AppHost, Pools};

pub(super) struct Absent;

impl Packager for Absent {
    type Config = ();
    type Live = Stream;

    const IS_AVAILABLE: bool = false;

    fn is_live(_live: &Stream) -> bool {
        true
    }

    fn start(
        _host: &AppHost,
        _worker: &Worker,
        _pools: &Pools,
        _shutdown: &CancelToken,
        _config: &(),
    ) -> BroadcastResult<Option<Stream>> {
        Err("no packager in this build".into())
    }

    fn release(_host: &AppHost) -> BroadcastResult<()> {
        Ok(())
    }

    fn stop(_live: Stream) {}

    fn url(live: &Stream) -> &str {
        &live.0
    }
}
