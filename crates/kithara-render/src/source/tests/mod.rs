use std::{
    collections::VecDeque,
    num::{NonZeroU32, NonZeroUsize},
};

use kithara_audio::{
    AudioSource, Fetch, SourceDiscontinuity, SourceEnd, TrackFailureKind, TrackStep, WaitingReason,
};
use kithara_bufpool::PoolRegion;
use kithara_command::{Batch, ChannelConfig, Inbox, Outcome, Rejection, Sender, When, channel};
use kithara_effects::{AudioEffect, EffectDrain, held_source_frames};
use kithara_platform::sync::{
    Arc, Mutex,
    atomic::{AtomicU64, Ordering},
};
use kithara_signal::{AudioChunk, AudioChunkInfo, AudioSpec};
use kithara_stream::{SeekControl, SeekObserve, SeekState};
#[cfg(any(feature = "stretch-signalsmith", feature = "stretch-bungee"))]
use kithara_test_fixtures::play_fixtures::half;
#[cfg(any(
    feature = "stretch-signalsmith",
    feature = "stretch-bungee",
    feature = "stretch-glide"
))]
use kithara_test_fixtures::play_fixtures::{negative_half, three_quarter};
use kithara_test_fixtures::play_fixtures::{negative_quarter, quarter};
#[cfg(any(
    feature = "stretch-signalsmith",
    feature = "stretch-bungee",
    feature = "stretch-glide"
))]
use kithara_test_utils::bufpool::pools_with_budget;
use kithara_test_utils::{
    bufpool::{TestPools, pools},
    kithara,
};
#[cfg(any(
    feature = "stretch-signalsmith",
    feature = "stretch-bungee",
    feature = "stretch-glide"
))]
use kithara_warp::WarpCapabilities;
use kithara_warp::{SpeedCurve, StretchKind};
#[cfg(any(
    feature = "stretch-signalsmith",
    feature = "stretch-bungee",
    feature = "stretch-glide"
))]
use num_traits::AsPrimitive;

use super::core::*;
use crate::{LaneCommand, LaneFrame, LaneProtocol, consts};

mod doubles;
mod flow;
mod lane;

use doubles::*;
