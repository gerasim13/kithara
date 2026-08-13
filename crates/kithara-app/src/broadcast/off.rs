use kithara::play::SessionHandle;
use kithara_platform::CancelToken;

use super::state::{BroadcastResult, Packager};

pub(crate) struct Backend;

/// No values: the packager is not compiled into this build. That is what makes
/// the on-air phase unconstructable and every arm below unreachable — the three
/// `match` expressions have no arms because there is nothing to match.
pub(crate) enum Stream {}

impl Packager for Backend {
    type Live = Stream;

    fn start(_session: &SessionHandle, _shutdown: &CancelToken) -> BroadcastResult<Option<Stream>> {
        Err("this build carries no broadcaster; rebuild with `--features broadcast`".into())
    }

    fn is_live(live: &Stream) -> bool {
        match *live {}
    }

    fn url(live: &Stream) -> &str {
        match *live {}
    }

    fn stop(live: Stream) {
        match live {}
    }
}
