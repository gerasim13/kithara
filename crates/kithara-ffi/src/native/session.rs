use std::{num::NonZeroU32, sync::OnceLock};

use kithara::{
    host::{HostConfig, HostOwned},
    platform::sync::Mutex,
    play::{PlayError, player::PlayerControlSource},
};

use crate::pools::{FfiHost, FfiPools};

static HOST: OnceLock<Mutex<FfiHost>> = OnceLock::new();

fn host() -> &'static Mutex<FfiHost> {
    HOST.get_or_init(|| {
        let host = FfiHost::new(HostConfig::builder().build())
            .expect("INVARIANT: the process audio Host must allocate its root identity");
        Mutex::new(host)
    })
}

pub(crate) fn insert<P>(player: P) -> Result<HostOwned<P>, PlayError>
where
    P: PlayerControlSource<Schema = FfiPools>,
{
    host().lock().insert(player)
}

pub(crate) fn requested_sample_rate() -> NonZeroU32 {
    host().lock().requested_sample_rate()
}

pub(crate) fn remove<P>(player: &HostOwned<P>) -> Result<(), PlayError>
where
    P: PlayerControlSource<Schema = FfiPools>,
{
    host().lock().remove(player)
}
