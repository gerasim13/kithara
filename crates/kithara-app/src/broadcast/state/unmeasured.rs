use super::{BroadcastResult, Packager, fixture::Stream};
use crate::pools::AppHost;

pub(super) struct Unmeasured;

impl Packager for Unmeasured {
    type Config = ();
    type Live = Stream;

    const IS_AVAILABLE: bool = true;

    fn is_live(_live: &Stream) -> bool {
        true
    }

    fn release(_host: &AppHost) -> BroadcastResult<()> {
        Ok(())
    }

    fn start(_host: &AppHost, _config: &()) -> BroadcastResult<Option<Stream>> {
        Ok(None)
    }

    fn stop(_live: Stream) {}

    fn url(live: &Stream) -> &str {
        &live.0
    }
}
