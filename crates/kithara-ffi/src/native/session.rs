use std::{num::NonZeroU32, sync::OnceLock};

use kithara::{
    host::{HostConfig, HostOwned, HostSettingsControl},
    platform::sync::Mutex,
    play::{PlayError, SessionError, player::PlayerControlSource},
    sync::SyncGroup,
};

use crate::pools::{FfiHost, FfiPools};

static HOST: OnceLock<Mutex<Option<FfiHost>>> = OnceLock::new();

fn host() -> &'static Mutex<Option<FfiHost>> {
    HOST.get_or_init(|| Mutex::new(None))
}

fn active_host(slot: &mut Option<FfiHost>) -> &mut FfiHost {
    slot.get_or_insert_with(|| {
        FfiHost::new(HostConfig::builder().build())
            .expect("INVARIANT: the process audio Host must allocate its root identity")
    })
}

pub(crate) fn insert<P>(player: P) -> Result<HostOwned<P>, PlayError>
where
    P: PlayerControlSource<Schema = FfiPools>,
{
    active_host(&mut host().lock()).insert(player)
}

pub(crate) fn requested_sample_rate() -> NonZeroU32 {
    host().lock().as_ref().map_or_else(
        || HostConfig::<FfiPools>::builder().build().sample_rate(),
        HostSettingsControl::sample_rate,
    )
}

pub(crate) fn remove<P>(player: &HostOwned<P>) -> Result<(), PlayError>
where
    P: PlayerControlSource<Schema = FfiPools>,
{
    let mut slot = host().lock();
    let active = slot.as_mut().ok_or(PlayError::SessionGone {
        reason: "process audio Host is unavailable",
    })?;
    active.remove(player)?;
    let empty = active
        .topology()
        .map_err(|error| PlayError::from(SessionError::from(error)))?
        .members()
        .is_empty();
    if empty {
        drop(slot.take());
    }
    drop(slot);
    Ok(())
}
