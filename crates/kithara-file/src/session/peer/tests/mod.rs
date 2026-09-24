mod fixtures;
#[cfg(not(target_arch = "wasm32"))]
use std::{sync::Barrier, thread};
use std::{sync::atomic::AtomicU64, task::Waker};

use kithara_assets::{
    AcquisitionResult, AssetResourceState, AssetStore, ResourceLease, StorageBackend, WriterOutcome,
};
use kithara_download::Peer;
use kithara_events::{Envelope, EventBus};
use kithara_platform::{CancelScope, CancelToken, sync::Arc, time::Duration};
use kithara_stream::WorkerWake;
use kithara_test_utils::kithara;

use super::*;
use crate::{FileEvent, session::FileSource, test_pools::pools};

mod completion;
mod metadata;
mod ownership;
mod seek;

#[cfg(not(target_arch = "wasm32"))]
use fixtures::BlockingWake;
use fixtures::{
    CountingWake, assert_ready_bytes, attach_pending, completion, fresh_session, make_coord,
    make_inner, make_inner_with_cancel, make_peer, test_key,
};
