use super::state::{BroadcastResult, Packager};
use crate::{config::AppBroadcastConfig, pools::AppHost};

pub(crate) struct Backend;

/// No values: the packager is not in this build, so the on-air phase cannot be
/// constructed and the matches below have nothing to match.
pub(crate) enum Stream {}

impl Packager for Backend {
    type Config = AppBroadcastConfig;
    type Live = Stream;

    const IS_AVAILABLE: bool = false;

    fn is_live(live: &Stream) -> bool {
        match *live {}
    }

    fn release(_host: &AppHost) -> BroadcastResult<()> {
        Ok(())
    }

    fn start(_host: &AppHost, _config: &AppBroadcastConfig) -> BroadcastResult<Option<Stream>> {
        Err("this build carries no broadcaster; rebuild with `--features broadcast`".into())
    }

    fn stop(live: Stream) {
        match live {}
    }

    fn url(live: &Stream) -> &str {
        match *live {}
    }
}
