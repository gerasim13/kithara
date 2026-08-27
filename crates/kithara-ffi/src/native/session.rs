use std::sync::OnceLock;

use kithara::{
    host::{Host, HostConfig, HostOwned},
    play::{PlayError, player::PlayerControlSource},
};
use kithara_platform::sync::Mutex;

static HOST: OnceLock<Mutex<Host>> = OnceLock::new();

fn host() -> &'static Mutex<Host> {
    HOST.get_or_init(|| {
        let host = Host::new(HostConfig::builder().build())
            .expect("INVARIANT: the process audio Host must allocate its root identity");
        Mutex::new(host)
    })
}

pub(crate) fn insert<P>(player: P) -> Result<HostOwned<P>, PlayError>
where
    P: PlayerControlSource,
{
    host().lock().insert(player)
}

pub(crate) fn remove<P>(player: &HostOwned<P>) -> Result<(), PlayError>
where
    P: PlayerControlSource,
{
    host().lock().remove(player)
}
