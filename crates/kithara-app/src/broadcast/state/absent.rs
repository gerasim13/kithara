use super::{BroadcastResult, Packager, fixture::Stream};
use crate::pools::AppHost;

pub(super) struct Absent;

impl Packager for Absent {
    type Config = ();
    type Live = Stream;

    const IS_AVAILABLE: bool = false;

    fn is_live(_live: &Stream) -> bool {
        true
    }

    fn release(_host: &AppHost) -> BroadcastResult<()> {
        Ok(())
    }

    fn start(_host: &AppHost, _config: &()) -> BroadcastResult<Option<Stream>> {
        Err("no packager in this build".into())
    }

    fn stop(_live: Stream) {}

    fn url(live: &Stream) -> &str {
        &live.0
    }
}
